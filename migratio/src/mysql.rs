//!
//! # MySQL migration support
//!
//! This module provides MySQL migration support using the [`mysql`](https://crates.io/crates/mysql) crate.
//!
//! ## Important: DDL and Transaction Limitations
//!
//! **MySQL handles DDL (Data Definition Language) statements differently than SQLite.**
//! Understanding this is critical for writing reliable migrations.
//!
//! ### The Core Issue
//!
//! In MySQL, DDL statements like `CREATE TABLE`, `ALTER TABLE`, `DROP TABLE`, `CREATE INDEX`, etc.
//! cause an **implicit commit** and **cannot be rolled back**. This is a fundamental MySQL behavior,
//! not a limitation of this library.
//!
//! This means:
//! - If a migration fails partway through, any DDL that already executed will remain in the database
//! - The migration version will NOT be recorded (allowing you to fix and retry)
//! - You may need to manually clean up partial changes before retrying
//!
//! ### Comparison with SQLite
//!
//! | Behavior | SQLite | MySQL |
//! |----------|--------|-------|
//! | DDL in transactions | Fully supported | Causes implicit commit |
//! | Migration failure | Complete rollback | Partial DDL may persist |
//! | Method parameter | `&Transaction` | `&mut Conn` |
//! | Automatic cleanup | Yes | Manual intervention may be needed |
//!
//! ### Best Practices for MySQL Migrations
//!
//! 1. **Make migrations idempotent** - Use `IF EXISTS` / `IF NOT EXISTS`:
//!    ```ignore
//!    fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
//!        conn.query_drop("CREATE TABLE IF NOT EXISTS users (...)")?;
//!        Ok(())
//!    }
//!    ```
//!
//! 2. **Keep migrations small and focused** - One logical change per migration reduces the
//!    impact of partial failures.
//!
//! 3. **Order statements carefully** - Put riskier operations (like data transformations)
//!    before DDL when possible, since data changes may be easier to reverse manually.
//!
//! 4. **Test thoroughly in staging** - DDL failures in production require manual intervention.
//!
//! 5. **Consider `down()` carefully** - Even `down()` migrations cannot atomically reverse DDL.
//!
//! ### What Happens on Failure
//!
//! ```text
//! Migration 3 starts
//!   ├── CREATE TABLE orders (...)     ✓ Committed immediately
//!   ├── CREATE INDEX idx_orders (...)  ✓ Committed immediately
//!   └── ALTER TABLE users ADD (...)    ✗ Error!
//!
//! Result:
//!   - "orders" table EXISTS (cannot be rolled back)
//!   - "idx_orders" index EXISTS (cannot be rolled back)
//!   - Migration 3 is NOT recorded in version table
//!   - You must manually DROP the partial changes before retrying
//! ```
//!
//! ### Recovery from Partial Failures
//!
//! If a migration fails partway through:
//!
//! 1. Check the database state to see what was applied
//! 2. Manually reverse the partial changes (or make your migration idempotent)
//! 3. Fix the migration code
//! 4. Run migrations again
//!
//! Because the failed migration version was not recorded, the migrator will attempt to
//! run it again on the next `upgrade()` call.
//!

use crate::core::{AppliedMigrationRow, GenericMigrator, MigrationBackend};
use crate::error::Error;
use crate::AppliedMigration;
use crate::Migration;
use crate::MigrationType;
use crate::MigrationReport;
use crate::Precondition;
use chrono::Utc;
use mysql::prelude::Queryable;
use mysql::Conn;

// Re-export mysql types for use in migrations
pub use mysql::prelude::Queryable as MysqlQueryable;
pub use mysql::Conn as MysqlConn;

/// MySQL-specific backend implementing the MigrationBackend trait.
pub(crate) struct MysqlBackend;

impl MigrationBackend for MysqlBackend {
    type Conn = Conn;

    fn version_table_exists(conn: &mut Conn, table_name: &str) -> Result<bool, Error> {
        let exists: bool = conn
            .query_first(format!(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = '{}'",
                table_name
            ))?
            .map(|(count,): (i64,)| count > 0)
            .unwrap_or(false);
        Ok(exists)
    }

    fn create_version_table(conn: &mut Conn, table_name: &str) -> Result<(), Error> {
        conn.query_drop(format!(
            "CREATE TABLE IF NOT EXISTS {} (
                version INT UNSIGNED PRIMARY KEY NOT NULL,
                name VARCHAR(255) NOT NULL,
                applied_at VARCHAR(255) NOT NULL,
                checksum VARCHAR(64) NOT NULL,
                migration_type VARCHAR(20) NOT NULL DEFAULT 'migration'
            )",
            table_name
        ))?;
        Ok(())
    }

    fn column_exists(
        conn: &mut Conn,
        table_name: &str,
        column_name: &str,
    ) -> Result<bool, Error> {
        let exists: bool = conn
            .query_first(format!(
                "SELECT COUNT(*) FROM information_schema.columns
                 WHERE table_schema = DATABASE()
                 AND table_name = '{}'
                 AND column_name = '{}'",
                table_name, column_name
            ))?
            .map(|(count,): (i64,)| count > 0)
            .unwrap_or(false);
        Ok(exists)
    }

    fn add_column(
        conn: &mut Conn,
        table_name: &str,
        column_name: &str,
        column_def: &str,
    ) -> Result<(), Error> {
        conn.query_drop(format!(
            "ALTER TABLE {} ADD COLUMN {} {}",
            table_name, column_name, column_def
        ))?;
        Ok(())
    }

    fn get_applied_migration_rows(
        conn: &mut Conn,
        table_name: &str,
    ) -> Result<Vec<AppliedMigrationRow>, Error> {
        let rows: Vec<(u32, String, String)> = conn.query(format!(
            "SELECT version, name, checksum FROM {}",
            table_name
        ))?;
        Ok(rows
            .into_iter()
            .map(|(version, name, checksum)| AppliedMigrationRow {
                version,
                name,
                checksum,
            })
            .collect())
    }

    fn get_max_version(conn: &mut Conn, table_name: &str) -> Result<u32, Error> {
        let result: Option<(Option<u32>,)> =
            conn.query_first(format!("SELECT MAX(version) FROM {}", table_name))?;
        Ok(result.and_then(|(v,)| v).unwrap_or(0))
    }

    fn get_migration_history_rows(
        conn: &mut Conn,
        table_name: &str,
    ) -> Result<Vec<AppliedMigration>, Error> {
        // Check whether the migration_type column exists for backwards compatibility
        let has_migration_type = Self::column_exists(conn, table_name, "migration_type")?;

        let rows: Vec<AppliedMigration> = if has_migration_type {
            let raw: Vec<(u32, String, String, String, String)> = conn.query(format!(
                "SELECT version, name, applied_at, checksum, migration_type FROM {} ORDER BY version",
                table_name
            ))?;
            raw.into_iter()
                .map(|(version, name, applied_at_str, checksum, migration_type_str)| {
                    let applied_at = chrono::DateTime::parse_from_rfc3339(&applied_at_str)
                        .map_err(|e| Error::Generic(format!("Failed to parse datetime: {}", e)))?
                        .with_timezone(&Utc);
                    let migration_type = if migration_type_str == "baseline" {
                        MigrationType::Baseline
                    } else {
                        MigrationType::Migration
                    };
                    Ok(AppliedMigration {
                        version,
                        name,
                        applied_at,
                        checksum,
                        migration_type,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?
        } else {
            let raw: Vec<(u32, String, String, String)> = conn.query(format!(
                "SELECT version, name, applied_at, checksum FROM {} ORDER BY version",
                table_name
            ))?;
            raw.into_iter()
                .map(|(version, name, applied_at_str, checksum)| {
                    let applied_at = chrono::DateTime::parse_from_rfc3339(&applied_at_str)
                        .map_err(|e| Error::Generic(format!("Failed to parse datetime: {}", e)))?
                        .with_timezone(&Utc);
                    Ok(AppliedMigration {
                        version,
                        name,
                        applied_at,
                        checksum,
                        migration_type: MigrationType::Migration,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?
        };

        Ok(rows)
    }

    fn execute_migration_up(
        conn: &mut Conn,
        migration: &Box<dyn Migration>,
        table_name: &str,
        applied_at: &str,
        checksum: &str,
        migration_type: MigrationType,
    ) -> Result<bool, Error> {
        // Check precondition
        let precondition = migration.mysql_precondition(conn)?;

        match precondition {
            Precondition::AlreadySatisfied => {
                // Stamp without running up() — no transaction needed for MySQL
                conn.exec_drop(
                    format!(
                        "INSERT INTO {} (version, name, applied_at, checksum, migration_type) VALUES(?, ?, ?, ?, ?)",
                        table_name
                    ),
                    (migration.version(), migration.name(), applied_at, checksum, migration_type.to_string()),
                )?;
                Ok(false)
            }
            Precondition::NeedsApply => {
                // Run the migration directly — no transaction (MySQL DDL causes implicit commits)
                migration.mysql_up(conn)?;
                // Insert version row after migration succeeds
                conn.exec_drop(
                    format!(
                        "INSERT INTO {} (version, name, applied_at, checksum, migration_type) VALUES(?, ?, ?, ?, ?)",
                        table_name
                    ),
                    (migration.version(), migration.name(), applied_at, checksum, migration_type.to_string()),
                )?;
                Ok(true)
            }
        }
    }

    fn execute_migration_down(
        conn: &mut Conn,
        migration: &Box<dyn Migration>,
        table_name: &str,
    ) -> Result<(), Error> {
        // Run the rollback directly — no transaction (MySQL DDL causes implicit commits)
        migration.mysql_down(conn)?;
        // Delete version row after rollback succeeds
        conn.exec_drop(
            format!("DELETE FROM {} WHERE version = ?", table_name),
            (migration.version(),),
        )?;
        Ok(())
    }
}

/// The entrypoint for running a sequence of [Migration]s on a MySQL database.
/// Construct this struct with the list of all [Migration]s to be applied.
/// [Migration::version]s must be contiguous, greater than zero, and unique.
#[derive(Debug)]
pub struct MysqlMigrator {
    migrator: GenericMigrator,
}

impl MysqlMigrator {
    /// Create a new MysqlMigrator, validating migration invariants.
    /// Returns an error if migrations are invalid.
    pub fn try_new(migrations: Vec<Box<dyn Migration>>) -> Result<Self, String> {
        Ok(Self {
            migrator: GenericMigrator::try_new(migrations)?,
        })
    }

    /// Create a new MysqlMigrator, panicking if migration metadata is invalid.
    /// For a non-panicking version, use `try_new`.
    pub fn new(migrations: Vec<Box<dyn Migration>>) -> Self {
        match Self::try_new(migrations) {
            Ok(migrator) => migrator,
            Err(err) => panic!("{}", err),
        }
    }

    /// Set a custom name for the schema version tracking table.
    /// Defaults to "_migratio_version_".
    pub fn with_schema_version_table_name(mut self, name: impl Into<String>) -> Self {
        self.migrator.set_schema_version_table_name(name);
        self
    }

    /// Set a callback to be invoked when a migration starts.
    /// The callback receives the migration version and name.
    pub fn on_migration_start<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str) + Send + Sync + 'static,
    {
        self.migrator.set_on_migration_start(callback);
        self
    }

    /// Set a callback to be invoked when a migration completes successfully.
    /// The callback receives the migration version, name, and duration.
    pub fn on_migration_complete<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str, std::time::Duration) + Send + Sync + 'static,
    {
        self.migrator.set_on_migration_complete(callback);
        self
    }

    /// Set a callback to be invoked when a migration is skipped because its precondition
    /// returned [`Precondition::AlreadySatisfied`].
    /// The callback receives the migration version and name.
    pub fn on_migration_skipped<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str) + Send + Sync + 'static,
    {
        self.migrator.set_on_migration_skipped(callback);
        self
    }

    /// Set a callback to be invoked when a migration fails.
    /// The callback receives the migration version, name, and error.
    pub fn on_migration_error<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str, &Error) + Send + Sync + 'static,
    {
        self.migrator.set_on_migration_error(callback);
        self
    }

    /// Get a reference to all migrations in this migrator.
    pub fn migrations(&self) -> &[Box<dyn Migration>] {
        &self.migrator.migrations
    }

    pub fn schema_version_table_name(&self) -> &str {
        &self.migrator.schema_version_table_name
    }

    /// Get the current migration version from the database.
    /// Returns 0 if no migrations have been applied.
    pub fn get_current_version(&self, conn: &mut Conn) -> Result<u32, Error> {
        self.migrator.generic_get_current_version::<MysqlBackend>(conn)
    }

    /// Get the history of all migrations that have been applied to the database.
    /// Returns migrations in the order they were applied (by version number).
    /// Returns an empty vector if no migrations have been applied.
    pub fn get_migration_history(&self, conn: &mut Conn) -> Result<Vec<AppliedMigration>, Error> {
        self.migrator.generic_get_migration_history::<MysqlBackend>(conn)
    }

    /// Preview which migrations would be applied by `upgrade()` without actually running them.
    /// Returns a list of migrations that would be executed, in the order they would run.
    pub fn preview_upgrade(&self, conn: &mut Conn) -> Result<Vec<&Box<dyn Migration>>, Error> {
        self.migrator.generic_preview_upgrade::<MysqlBackend>(conn)
    }

    /// Preview which migrations would be rolled back by `downgrade(target_version)` without actually running them.
    /// Returns a list of migrations that would be executed, in the order they would run (reverse order).
    pub fn preview_downgrade(
        &self,
        conn: &mut Conn,
        target_version: u32,
    ) -> Result<Vec<&Box<dyn Migration>>, Error> {
        self.migrator
            .generic_preview_downgrade::<MysqlBackend>(conn, target_version)
    }

    /// Upgrade the database to a specific target version.
    ///
    /// This runs all pending migrations up to and including the target version.
    /// If the database is already at or beyond the target version, no migrations are run.
    pub fn upgrade_to(
        &self,
        conn: &mut Conn,
        target_version: u32,
    ) -> Result<MigrationReport<'_>, Error> {
        // Validate target version exists
        if target_version > 0
            && !self
                .migrations()
                .iter()
                .any(|m| m.version() == target_version)
        {
            return Err(Error::Generic(format!(
                "Target version {} does not exist in migration list",
                target_version
            )));
        }

        self.migrator
            .generic_upgrade::<MysqlBackend>(conn, Some(target_version))
    }

    /// Upgrade the database by running all pending migrations.
    pub fn upgrade(&self, conn: &mut Conn) -> Result<MigrationReport<'_>, Error> {
        self.migrator.generic_upgrade::<MysqlBackend>(conn, None)
    }

    /// Rollback migrations down to the specified target version.
    /// Pass `target_version = 0` to rollback all migrations.
    /// Note: In MySQL, DDL statements cause implicit commits and cannot be rolled back automatically.
    /// Returns a [MigrationReport] describing which migrations were rolled back.
    pub fn downgrade(
        &self,
        conn: &mut Conn,
        target_version: u32,
    ) -> Result<MigrationReport<'_>, Error> {
        self.migrator
            .generic_downgrade::<MysqlBackend>(conn, target_version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_mysql::get_test_conn;

    #[tokio::test]
    async fn single_successful_from_clean() {
        use chrono::{DateTime, FixedOffset};

        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        assert_eq!(
            report,
            MigrationReport {
                schema_version_table_existed: false,
                schema_version_table_created: true,
                migrations_run: vec![1],
                failing_migration: None,
            }
        );

        // Verify schema version table exists and has recorded version 1
        let rows: Vec<(u32, String, String)> = conn
            .query("SELECT version, name, applied_at FROM _migratio_version_")
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 1); // version
        assert_eq!(rows[0].1, "Migration 1"); // name (default)

        let date_string_raw = &rows[0].2;
        let date = DateTime::parse_from_rfc3339(date_string_raw).unwrap();
        assert_eq!(date.timezone(), FixedOffset::east_opt(0).unwrap());

        // Ensure that the date is within 5 seconds of now
        let now = Utc::now();
        let diff = now.timestamp() - date.timestamp();
        assert!(diff < 5);
    }

    #[tokio::test]
    async fn single_unsuccessful_from_clean() {
        let (_pool, mut conn) = get_test_conn().await;

        // Set up a table and some data to ensure it's preserved
        conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY, value INT)")
            .unwrap();
        conn.query_drop("INSERT INTO test (id, value) VALUES (1, 100)")
            .unwrap();
        conn.query_drop("INSERT INTO test (id, value) VALUES (2, 200)")
            .unwrap();

        // Verify test table has original data
        let sum: i64 = conn
            .query_first("SELECT SUM(value) FROM test")
            .unwrap()
            .unwrap();
        assert_eq!(sum, 300);

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                // Do some DML operations that work (can be rolled back in MySQL)
                conn.query_drop("UPDATE test SET value = value * 2")?;
                conn.query_drop("INSERT INTO test (id, value) VALUES (3, 300)")?;
                // Then do something that fails
                conn.query_drop("THIS IS NOT VALID SQL")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        // Verify migration failed
        assert_eq!(report.migrations_run, Vec::<u32>::new());
        assert!(report.failing_migration.is_some());

        // result of migration is NOT rolled back
        let sum: i64 = conn
            .query_first("SELECT SUM(value) FROM test")
            .unwrap()
            .unwrap();
        assert_eq!(sum, 900);

        let count: i64 = conn
            .query_first("SELECT COUNT(*) FROM test")
            .unwrap()
            .unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn upgrade_to_specific_version() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE users (id INT PRIMARY KEY)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE posts (id INT PRIMARY KEY)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE comments (id INT PRIMARY KEY)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);

        // Upgrade to version 2
        let report = migrator.upgrade_to(&mut conn, 2).unwrap();
        assert_eq!(report.migrations_run, vec![1, 2]);

        // Verify only migrations 1 and 2 ran
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 2);

        // Verify users and posts tables exist
        let users_exists: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'users'")
            .unwrap()
            .unwrap();
        assert_eq!(users_exists, 1);

        let posts_exists: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'posts'")
            .unwrap()
            .unwrap();
        assert_eq!(posts_exists, 1);

        // Verify comments table does not exist
        let comments_exists: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'comments'")
            .unwrap()
            .unwrap();
        assert_eq!(comments_exists, 0);
    }

    #[tokio::test]
    async fn upgrade_to_nonexistent_version() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let result = migrator.upgrade_to(&mut conn, 99);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn success_then_failure_from_clean() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE users (id INT PRIMARY KEY)")?;
                conn.query_drop("INSERT INTO users VALUES (1), (2)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("INVALID SQL")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        assert_eq!(report.migrations_run, vec![1]);
        assert!(report.failing_migration.is_some());

        // Verify users table exists with data
        let count: i64 = conn
            .query_first("SELECT COUNT(*) FROM users")
            .unwrap()
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    #[should_panic(expected = "Migration version must be greater than 0")]
    async fn new_rejects_zero_version() {
        struct Migration0;
        impl Migration for Migration0 {
            fn version(&self) -> u32 {
                0
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }
        MysqlMigrator::new(vec![Box::new(Migration0)]);
    }

    #[tokio::test]
    #[should_panic(expected = "Duplicate migration version found: 2")]
    async fn new_rejects_duplicate_versions() {
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2a;
        impl Migration for Migration2a {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2b;
        impl Migration for Migration2b {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        MysqlMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2a),
            Box::new(Migration2b),
        ]);
    }

    #[tokio::test]
    async fn new_accepts_non_starting_at_one() {
        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        // [2, 3] is now valid — version 2 is treated as a baseline
        let migrator = MysqlMigrator::new(vec![Box::new(Migration2), Box::new(Migration3)]);
        assert_eq!(migrator.migrations().len(), 2);
        assert_eq!(migrator.migrations()[0].version(), 2);
        assert_eq!(migrator.migrations()[1].version(), 3);
    }

    #[tokio::test]
    #[should_panic(expected = "Migration versions must be contiguous")]
    async fn new_rejects_non_contiguous() {
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration3)]);
    }

    #[tokio::test]
    async fn try_new_returns_ok_for_non_starting_at_one() {
        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        // version 2 as the sole migration is now valid — it becomes the baseline
        let result = MysqlMigrator::try_new(vec![Box::new(Migration2)]);
        assert!(result.is_ok());
        let migrator = result.unwrap();
        assert_eq!(migrator.migrations().len(), 1);
        assert_eq!(migrator.migrations()[0].version(), 2);
    }

    #[tokio::test]
    async fn try_new_returns_err_for_duplicate_versions() {
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2a;
        impl Migration for Migration2a {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2b;
        impl Migration for Migration2b {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let result = MysqlMigrator::try_new(vec![
            Box::new(Migration1),
            Box::new(Migration2a),
            Box::new(Migration2b),
        ]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Duplicate migration version"));
    }

    #[tokio::test]
    async fn try_new_returns_ok_for_valid_migrations() {
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let result = MysqlMigrator::try_new(vec![Box::new(Migration1), Box::new(Migration2)]);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn checksum_validation_detects_modified_migration() {
        let (_pool, mut conn) = get_test_conn().await;

        // Apply migration with original name
        struct Migration1V1;
        impl Migration for Migration1V1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "original_name".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1V1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Try to run again with modified name (should fail checksum)
        struct Migration1V2;
        impl Migration for Migration1V2 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "modified_name".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator2 = MysqlMigrator::new(vec![Box::new(Migration1V2)]);
        let result = migrator2.upgrade(&mut conn);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));
    }

    #[tokio::test]
    async fn checksum_validation_passes_for_unmodified_migrations() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        // Apply migration 1
        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Apply migrations 1 and 2 (should validate migration 1's checksum)
        let migrator2 = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let result = migrator2.upgrade(&mut conn);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn checksums_stored_in_database() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "test_migration".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Verify checksum is stored
        let checksum: String = conn
            .query_first("SELECT checksum FROM _migratio_version_ WHERE version = 1")
            .unwrap()
            .unwrap();
        assert!(!checksum.is_empty());
        assert_eq!(checksum.len(), 64); // SHA256 hex string
    }

    #[tokio::test]
    async fn downgrade_single_migration() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn mysql_down(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("DROP TABLE test")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);

        // Apply migration
        migrator.upgrade(&mut conn).unwrap();
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 1);

        // Rollback
        let report = migrator.downgrade(&mut conn, 0).unwrap();
        assert_eq!(report.migrations_run, vec![1]);

        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 0);

        // Verify table is dropped
        let table_exists: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'test'")
            .unwrap()
            .unwrap();
        assert_eq!(table_exists, 0);
    }

    #[tokio::test]
    async fn downgrade_multiple_migrations() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn mysql_down(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("DROP TABLE test1")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn mysql_down(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("DROP TABLE test2")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test3 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn mysql_down(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("DROP TABLE test3")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);

        // Apply all migrations
        migrator.upgrade(&mut conn).unwrap();

        // Rollback to version 1
        let report = migrator.downgrade(&mut conn, 1).unwrap();
        assert_eq!(report.migrations_run, vec![3, 2]);

        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 1);
    }

    #[tokio::test]
    async fn downgrade_on_clean_database() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            fn mysql_down(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.downgrade(&mut conn, 0).unwrap();
        assert_eq!(report.migrations_run, Vec::<u32>::new());
    }

    #[tokio::test]
    async fn downgrade_with_invalid_target_version() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn mysql_down(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("DROP TABLE test")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Try to downgrade to version 5 (greater than current)
        let result = migrator.downgrade(&mut conn, 5);
        assert!(result.is_err());
    }

    #[tokio::test]
    #[should_panic(expected = "does not support downgrade")]
    async fn downgrade_panics_when_down_not_implemented() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();
        migrator.downgrade(&mut conn, 0).unwrap();
    }

    #[tokio::test]
    async fn get_current_version_on_clean_database() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 0);
    }

    #[tokio::test]
    async fn get_current_version_after_migrations() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        migrator.upgrade(&mut conn).unwrap();

        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 2);
    }

    #[tokio::test]
    async fn preview_upgrade_on_clean_database() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let pending = migrator.preview_upgrade(&mut conn).unwrap();

        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].version(), 1);
        assert_eq!(pending[1].version(), 2);
    }

    #[tokio::test]
    async fn preview_upgrade_with_partial_migrations() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        // Apply only migration 1
        let migrator1 = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator1.upgrade(&mut conn).unwrap();

        // Preview with all 3 migrations
        let migrator2 = MysqlMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);
        let pending = migrator2.preview_upgrade(&mut conn).unwrap();

        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].version(), 2);
        assert_eq!(pending[1].version(), 3);
    }

    #[tokio::test]
    async fn preview_downgrade_to_zero() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn mysql_down(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("DROP TABLE test1")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn mysql_down(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("DROP TABLE test2")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        migrator.upgrade(&mut conn).unwrap();

        let to_rollback = migrator.preview_downgrade(&mut conn, 0).unwrap();
        assert_eq!(to_rollback.len(), 2);
        assert_eq!(to_rollback[0].version(), 2); // Reverse order
        assert_eq!(to_rollback[1].version(), 1);
    }

    #[tokio::test]
    async fn get_migration_history_on_clean_database() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let history = migrator.get_migration_history(&mut conn).unwrap();
        assert_eq!(history.len(), 0);
    }

    #[tokio::test]
    async fn get_migration_history_after_migrations() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        migrator.upgrade(&mut conn).unwrap();

        let history = migrator.get_migration_history(&mut conn).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].version, 1);
        assert_eq!(history[0].name, "create_test1");
        assert_eq!(history[1].version, 2);
        assert_eq!(history[1].name, "create_test2");
    }

    #[tokio::test]
    async fn hooks_are_called_on_successful_migration() {
        use std::sync::{Arc, Mutex};

        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "test_migration".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let events1 = Arc::clone(&events);
        let events2 = Arc::clone(&events);

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)])
            .on_migration_start(move |version, name| {
                events1
                    .lock()
                    .unwrap()
                    .push(format!("start:{}:{}", version, name));
            })
            .on_migration_complete(move |version, name, _duration| {
                events2
                    .lock()
                    .unwrap()
                    .push(format!("complete:{}:{}", version, name));
            });

        migrator.upgrade(&mut conn).unwrap();

        let logged_events = events.lock().unwrap().clone();
        assert_eq!(
            logged_events,
            vec!["start:1:test_migration", "complete:1:test_migration"]
        );
    }

    #[tokio::test]
    async fn hooks_are_called_on_failed_migration() {
        use std::sync::{Arc, Mutex};

        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Err(Error::Generic("Test error".to_string()))
            }
            fn name(&self) -> String {
                "failing_migration".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let events1 = Arc::clone(&events);
        let events2 = Arc::clone(&events);

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)])
            .on_migration_start(move |version, name| {
                events1
                    .lock()
                    .unwrap()
                    .push(format!("start:{}:{}", version, name));
            })
            .on_migration_error(move |version, name, _error| {
                events2
                    .lock()
                    .unwrap()
                    .push(format!("error:{}:{}", version, name));
            });

        migrator.upgrade(&mut conn).unwrap();

        let logged_events = events.lock().unwrap().clone();
        assert_eq!(
            logged_events,
            vec!["start:1:failing_migration", "error:1:failing_migration"]
        );
    }

    #[tokio::test]
    async fn precondition_already_satisfied() {
        use std::cell::Cell;

        let (_pool, mut conn) = get_test_conn().await;

        // Create the table manually
        conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")
            .unwrap();

        struct Migration1 {
            up_called: Cell<bool>,
        }
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                self.up_called.set(true);
                Ok(())
            }
            fn mysql_precondition(&self, conn: &mut Conn) -> Result<Precondition, Error> {
                // Check if table exists
                let exists: i64 = conn
                    .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'test'")?
                    .unwrap();
                if exists > 0 {
                    Ok(Precondition::AlreadySatisfied)
                } else {
                    Ok(Precondition::NeedsApply)
                }
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migration = Migration1 {
            up_called: Cell::new(false),
        };
        let migrator = MysqlMigrator::new(vec![Box::new(migration)]);
        migrator.upgrade(&mut conn).unwrap();

        // up() should not have been called
        // Note: We can't check up_called here because migration was moved into Box
        // Instead verify table still exists (wasn't dropped and recreated)
        let count: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'test'")
            .unwrap()
            .unwrap();
        assert_eq!(count, 1);

        // Verify migration was recorded
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 1);
    }

    #[tokio::test]
    async fn precondition_needs_apply() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn mysql_precondition(&self, conn: &mut Conn) -> Result<Precondition, Error> {
                let exists: i64 = conn
                    .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'test'")?
                    .unwrap();
                if exists > 0 {
                    Ok(Precondition::AlreadySatisfied)
                } else {
                    Ok(Precondition::NeedsApply)
                }
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Verify table was created (up() was called)
        let count: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'test'")
            .unwrap()
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn precondition_error() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            fn mysql_precondition(&self, _conn: &mut Conn) -> Result<Precondition, Error> {
                Err(Error::Generic("Precondition check failed".to_string()))
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        // Verify migration failed
        assert_eq!(report.migrations_run, Vec::<u32>::new());
        assert!(report.failing_migration.is_some());
    }

    #[tokio::test]
    async fn migration_failure_accessors() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Err(Error::Generic("Test error".to_string()))
            }
            fn name(&self) -> String {
                "failing_migration".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, _conn: &mut Conn) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        assert!(report.failing_migration.is_some());
        let failure = report.failing_migration.unwrap();
        assert_eq!(failure.migration().version(), 1);
        assert_eq!(failure.migration().name(), "failing_migration");
        assert_eq!(failure.error().to_string(), "Test error");
    }

    #[tokio::test]
    async fn detects_missing_migration_in_code() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "migration_1".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "migration_2".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
                conn.query_drop("CREATE TABLE test3 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "migration_3".to_string()
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        // Apply all 3 migrations
        let migrator = MysqlMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);
        migrator.upgrade(&mut conn).unwrap();

        // Now manually delete migration 2 from the tracking table to simulate it being removed
        conn.exec_drop("DELETE FROM _migratio_version_ WHERE version = 2", ())
            .unwrap();

        // Try to upgrade again - should detect that migration 2 is in code but not in DB
        // while migration 3 is in DB (orphaned migration scenario)
        let result = migrator.upgrade(&mut conn);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not applied"));
    }
}
