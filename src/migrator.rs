use crate::error::Error;
use chrono::Utc;
use rusqlite::{params, Connection, Transaction};
use sha2::{Digest, Sha256};

/// Represents a failure during a migration.
#[derive(Debug, PartialEq)]
pub struct MigrationFailure<'migration> {
    migration: &'migration Box<dyn Migration>,
    error: Error,
}

impl<'migration> MigrationFailure<'migration> {
    /// Get the migration that failed.
    pub fn migration(&self) -> &dyn Migration {
        self.migration.as_ref()
    }

    /// Get the error that caused the migration to fail.
    pub fn error(&self) -> &Error {
        &self.error
    }
}

/// A report of actions performed during a migration.
#[derive(Debug, PartialEq)]
pub struct MigrationReport<'migration> {
    pub schema_version_table_existed: bool,
    pub schema_version_table_created: bool,
    pub migrations_run: Vec<u32>,
    pub failing_migration: Option<MigrationFailure<'migration>>,
}

/// Represents a migration that has been applied to the database.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedMigration {
    /// The version number of the migration.
    pub version: u32,
    /// The name of the migration.
    pub name: String,
    /// The timestamp when the migration was applied.
    pub applied_at: chrono::DateTime<Utc>,
    /// The checksum of the migration at the time it was applied.
    pub checksum: String,
}

const DEFAULT_VERSION_TABLE_NAME: &str = "_migratio_version_";

/// A trait that must be implemented to define a migration.
/// The `version` value must be unique among all migrations supplied to the migrator, and greater than 0.
/// Implement your migration logic in the `up` method, using the supplied [Transaction] to perform database operations.
/// The transaction will be automatically committed if the migration succeeds, or rolled back if it fails.
/// Optionally implement the `down` method to enable rollback support via [SqliteMigrator::downgrade].
/// The `name` and `description` methods are optional, and only aid in debugging / observability.
pub trait Migration {
    fn version(&self) -> u32;

    fn up(&self, tx: &Transaction) -> Result<(), Error>;

    /// Rollback this migration. This is optional - the default implementation panics.
    /// If you want to support downgrade functionality, implement this method.
    fn down(&self, _tx: &Transaction) -> Result<(), Error> {
        panic!(
            "Migration {} ('{}') does not support downgrade. Implement the down() method to enable rollback.",
            self.version(),
            self.name()
        )
    }

    fn name(&self) -> String {
        format!("Migration {}", self.version())
    }

    // This default implementation does nothing
    fn description(&self) -> Option<&'static str> {
        None
    }
}

impl PartialEq for dyn Migration {
    fn eq(&self, other: &Self) -> bool {
        self.version() == other.version()
    }
}

impl std::fmt::Debug for dyn Migration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Migration")
            .field("version", &self.version())
            .field("name", &self.name())
            .finish()
    }
}

/// The entrypoint for running a sequence of [Migration]s.
/// Construct this struct with the list of all [Migration]s to be applied.
/// [Migration::version]s must be contiguous, greater than zero, and unique.
#[derive(Debug)]
pub struct SqliteMigrator {
    migrations: Vec<Box<dyn Migration>>,
    schema_version_table_name: String,
    busy_timeout: std::time::Duration,
}

impl SqliteMigrator {
    /// Set up connection for safe concurrent access.
    /// This sets a busy timeout so concurrent operations wait instead of failing immediately.
    fn setup_concurrency_protection(&self, conn: &Connection) -> Result<(), Error> {
        // Set the configured busy timeout
        // This makes concurrent migration attempts wait instead of failing immediately
        conn.busy_timeout(self.busy_timeout)?;
        Ok(())
    }

    /// Calculate a checksum for a migration based on its version and name.
    /// This is used to verify that migrations haven't been modified after being applied.
    fn calculate_checksum(migration: &Box<dyn Migration>) -> String {
        let mut hasher = Sha256::new();
        hasher.update(migration.version().to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(migration.name().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Create a new SqliteMigrator, validating migration invariants.
    /// Returns an error if migrations are invalid.
    pub fn try_new(migrations: Vec<Box<dyn Migration>>) -> Result<Self, String> {
        // Verify invariants
        let mut versions: Vec<u32> = migrations.iter().map(|m| m.version()).collect();
        versions.sort();

        // Check for duplicates and zero versions
        for (i, &version) in versions.iter().enumerate() {
            // Version must be greater than zero
            if version == 0 {
                return Err("Migration version must be greater than 0, found version 0".to_string());
            }

            // Check for duplicates
            if i > 0 && versions[i - 1] == version {
                return Err(format!("Duplicate migration version found: {}", version));
            }
        }

        // Check for contiguity (versions must be 1, 2, 3, ...)
        if !versions.is_empty() {
            if versions[0] != 1 {
                return Err(format!(
                    "Migration versions must start at 1, found starting version: {}",
                    versions[0]
                ));
            }

            for (i, &version) in versions.iter().enumerate() {
                let expected = (i + 1) as u32;
                if version != expected {
                    return Err(format!(
                        "Migration versions must be contiguous. Expected version {}, found {}",
                        expected, version
                    ));
                }
            }
        }

        Ok(Self {
            migrations,
            schema_version_table_name: DEFAULT_VERSION_TABLE_NAME.to_string(),
            busy_timeout: std::time::Duration::from_secs(30),
        })
    }

    /// Create a new SqliteMigrator, panicking if migration metadata is invalid.
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
        self.schema_version_table_name = name.into();
        self
    }

    /// Set the busy timeout for SQLite database operations.
    /// This controls how long concurrent migration attempts will wait for locks.
    /// Defaults to 30 seconds.
    pub fn with_busy_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.busy_timeout = timeout;
        self
    }

    /// Get the current migration version from the database.
    /// Returns 0 if no migrations have been applied.
    pub fn get_current_version(&self, conn: &mut Connection) -> Result<u32, Error> {
        // Check if schema version table exists
        let mut stmt =
            conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name=?1")?;
        let table_exists = stmt
            .query([&self.schema_version_table_name])?
            .next()?
            .is_some();

        if !table_exists {
            return Ok(0);
        }

        // Get current version (highest version number)
        let mut stmt = conn.prepare(&format!(
            "SELECT MAX(version) from {}",
            self.schema_version_table_name
        ))?;
        let version: Option<u32> = stmt.query_row([], |row| row.get(0))?;
        Ok(version.unwrap_or(0))
    }

    /// Get the history of all migrations that have been applied to the database.
    /// Returns migrations in the order they were applied (by version number).
    /// Returns an empty vector if no migrations have been applied.
    pub fn get_migration_history(
        &self,
        conn: &mut Connection,
    ) -> Result<Vec<AppliedMigration>, Error> {
        // Check if schema version table exists
        let mut stmt =
            conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name=?1")?;
        let table_exists = stmt
            .query([&self.schema_version_table_name])?
            .next()?
            .is_some();

        if !table_exists {
            return Ok(vec![]);
        }

        // Query all applied migrations, ordered by version
        let mut stmt = conn.prepare(&format!(
            "SELECT version, name, applied_at, checksum FROM {} ORDER BY version",
            self.schema_version_table_name
        ))?;

        let migrations = stmt
            .query_map([], |row| {
                let applied_at_str: String = row.get(2)?;
                let applied_at = chrono::DateTime::parse_from_rfc3339(&applied_at_str)
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?
                    .with_timezone(&Utc);

                Ok(AppliedMigration {
                    version: row.get(0)?,
                    name: row.get(1)?,
                    applied_at,
                    checksum: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(migrations)
    }

    /// Preview which migrations would be applied by `upgrade()` without actually running them.
    /// Returns a list of migrations that would be executed, in the order they would run.
    pub fn preview_upgrade(
        &self,
        conn: &mut Connection,
    ) -> Result<Vec<&Box<dyn Migration>>, Error> {
        let current_version = self.get_current_version(conn)?;

        let mut pending_migrations = self
            .migrations
            .iter()
            .filter(|m| m.version() > current_version)
            .collect::<Vec<_>>();
        pending_migrations.sort_by_key(|m| m.version());

        Ok(pending_migrations)
    }

    /// Preview which migrations would be rolled back by `downgrade(target_version)` without actually running them.
    /// Returns a list of migrations that would be executed, in the order they would run (reverse order).
    pub fn preview_downgrade(
        &self,
        conn: &mut Connection,
        target_version: u32,
    ) -> Result<Vec<&Box<dyn Migration>>, Error> {
        let current_version = self.get_current_version(conn)?;

        // Validate target version
        if target_version > current_version {
            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                format!(
                    "Cannot downgrade to version {} when current version is {}. Target must be <= current version.",
                    target_version, current_version
                ),
            )));
        }

        let mut migrations_to_rollback = self
            .migrations
            .iter()
            .filter(|m| m.version() > target_version && m.version() <= current_version)
            .collect::<Vec<_>>();
        migrations_to_rollback.sort_by_key(|m| std::cmp::Reverse(m.version())); // Reverse order

        Ok(migrations_to_rollback)
    }

    /// Apply all previously-unapplied [Migration]s to the database with the given [Connection].
    /// Each migration runs within its own transaction, which is automatically rolled back if the migration fails.
    /// This method uses SQLite's busy timeout to handle concurrent migration attempts safely.
    pub fn upgrade(&self, conn: &mut Connection) -> Result<MigrationReport<'_>, Error> {
        // Set up concurrency protection
        self.setup_concurrency_protection(conn)?;
        // if schema version tracking table does not exist, create it
        let schema_version_table_existed = {
            let mut stmt =
                conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name=?1")?;
            let schema_version_table_existed = stmt
                .query([&self.schema_version_table_name])?
                .next()?
                .is_some();
            if !schema_version_table_existed {
                // create table with name and checksum columns
                // Use IF NOT EXISTS to handle concurrent creation attempts
                conn.execute(
                &format!(
                    "CREATE TABLE IF NOT EXISTS {} (version integer primary key not null, name text not null, applied_at text not null, checksum text not null)",
                    self.schema_version_table_name
                ),
                [],)?;
            }
            schema_version_table_existed
        };

        // Validate checksums of previously-applied migrations
        if schema_version_table_existed {
            // Check if the checksum column exists (for backwards compatibility)
            let has_checksum_column = {
                let mut stmt = conn.prepare(&format!(
                    "PRAGMA table_info({})",
                    self.schema_version_table_name
                ))?;
                let columns: Vec<String> = stmt
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                columns.contains(&"checksum".to_string())
            };

            if has_checksum_column {
                // Get all applied migrations with their checksums
                let mut stmt = conn.prepare(&format!(
                    "SELECT version, name, checksum FROM {}",
                    self.schema_version_table_name
                ))?;
                let applied_migrations: Vec<(u32, String, String)> = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<Result<Vec<_>, _>>()?;

                // Verify checksums match for migrations that were already applied
                for (applied_version, applied_name, applied_checksum) in applied_migrations {
                    if let Some(migration) = self
                        .migrations
                        .iter()
                        .find(|m| m.version() == applied_version)
                    {
                        let current_checksum = Self::calculate_checksum(migration);
                        if current_checksum != applied_checksum {
                            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                                format!(
                                    "Migration {} checksum mismatch. Expected '{}' but found '{}'. \
                                    Migration name in DB: '{}', current name: '{}'. \
                                    This indicates the migration was modified after being applied.",
                                    applied_version,
                                    applied_checksum,
                                    current_checksum,
                                    applied_name,
                                    migration.name()
                                ),
                            )));
                        }
                    }
                }
            }
        }

        // get current migration version (highest version number)
        let current_version: u32 = {
            let mut stmt = conn.prepare(&format!(
                "SELECT MAX(version) from {}",
                self.schema_version_table_name
            ))?;
            let version: Option<u32> = stmt.query_row([], |row| row.get(0))?;
            version.unwrap_or(0)
        };
        // iterate through migrations, run those that haven't been run
        let mut migrations_run: Vec<u32> = Vec::new();
        let mut migrations_sorted = self
            .migrations
            .iter()
            .map(|x| (x.version(), x))
            .collect::<Vec<_>>();
        migrations_sorted.sort_by_key(|m| m.0);
        let mut failing_migration: Option<MigrationFailure> = None;
        let mut schema_version_table_created = false;
        // Track the applied_at time for this upgrade() call - all migrations in this batch get the same timestamp
        let batch_applied_at = Utc::now().to_rfc3339();
        for (migration_version, migration) in migrations_sorted {
            if current_version < migration_version {
                // Start a transaction for this migration
                let tx = conn.transaction()?;
                let migration_result = migration.up(&tx);

                match migration_result {
                    Ok(_) => {
                        // Commit the transaction
                        tx.commit()?;

                        // Calculate checksum for this migration
                        let checksum = Self::calculate_checksum(migration);

                        // Insert a row for this migration with its name, timestamp, and checksum
                        conn.execute(
                            &format!(
                                "INSERT INTO {} (version, name, applied_at, checksum) VALUES(?1, ?2, ?3, ?4)",
                                self.schema_version_table_name
                            ),
                            params![migration_version, migration.name(), &batch_applied_at, checksum],
                        )?;

                        // record migration as run
                        migrations_run.push(migration_version);
                        // also, since any migration succeeded, if schema version table had not originally existed,
                        // we can mark that it was created
                        schema_version_table_created = true;
                    }
                    Err(e) => {
                        // Transaction will be automatically rolled back when dropped
                        failing_migration = Some(MigrationFailure {
                            migration,
                            error: e,
                        });
                        break;
                    }
                }
            }
        }
        // return report
        Ok(MigrationReport {
            schema_version_table_existed,
            schema_version_table_created,
            failing_migration,
            migrations_run,
        })
    }

    /// Rollback migrations down to the specified target version.
    /// Pass `target_version = 0` to rollback all migrations.
    /// Each migration's `down()` method runs within its own transaction, which is automatically rolled back if it fails.
    /// Returns a [MigrationReport] describing which migrations were rolled back.
    /// This method uses SQLite's busy timeout to handle concurrent migration attempts safely.
    pub fn downgrade(
        &self,
        conn: &mut Connection,
        target_version: u32,
    ) -> Result<MigrationReport<'_>, Error> {
        // Set up concurrency protection
        self.setup_concurrency_protection(conn)?;
        // Check if schema version table exists
        let schema_version_table_existed = {
            let mut stmt =
                conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name=?1")?;
            let exists = stmt
                .query([&self.schema_version_table_name])?
                .next()?
                .is_some();
            exists
        };

        if !schema_version_table_existed {
            // No migrations have been applied yet
            return Ok(MigrationReport {
                schema_version_table_existed: false,
                schema_version_table_created: false,
                failing_migration: None,
                migrations_run: vec![],
            });
        }

        // Validate checksums of previously-applied migrations (same as upgrade)
        let has_checksum_column = {
            let mut stmt = conn.prepare(&format!(
                "PRAGMA table_info({})",
                self.schema_version_table_name
            ))?;
            let columns: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            columns.contains(&"checksum".to_string())
        };

        if has_checksum_column {
            let mut stmt = conn.prepare(&format!(
                "SELECT version, name, checksum FROM {}",
                self.schema_version_table_name
            ))?;
            let applied_migrations: Vec<(u32, String, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;

            for (applied_version, applied_name, applied_checksum) in applied_migrations {
                if let Some(migration) = self
                    .migrations
                    .iter()
                    .find(|m| m.version() == applied_version)
                {
                    let current_checksum = Self::calculate_checksum(migration);
                    if current_checksum != applied_checksum {
                        return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                            format!(
                                "Migration {} checksum mismatch. Expected '{}' but found '{}'. \
                                Migration name in DB: '{}', current name: '{}'. \
                                This indicates the migration was modified after being applied.",
                                applied_version,
                                applied_checksum,
                                current_checksum,
                                applied_name,
                                migration.name()
                            ),
                        )));
                    }
                }
            }
        }

        // Get current version
        let current_version: u32 = {
            let mut stmt = conn.prepare(&format!(
                "SELECT MAX(version) from {}",
                self.schema_version_table_name
            ))?;
            let version: Option<u32> = stmt.query_row([], |row| row.get(0))?;
            version.unwrap_or(0)
        };

        // Validate target version
        if target_version > current_version {
            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                format!(
                    "Cannot downgrade to version {} when current version is {}. Target must be <= current version.",
                    target_version, current_version
                ),
            )));
        }

        // Get migrations to rollback (in reverse order)
        let mut migrations_to_rollback = self
            .migrations
            .iter()
            .filter(|m| m.version() > target_version && m.version() <= current_version)
            .map(|x| (x.version(), x))
            .collect::<Vec<_>>();
        migrations_to_rollback.sort_by_key(|m| std::cmp::Reverse(m.0)); // Reverse order

        let mut migrations_run: Vec<u32> = Vec::new();
        let mut failing_migration: Option<MigrationFailure> = None;

        for (migration_version, migration) in migrations_to_rollback {
            // Start a transaction for this migration rollback
            let tx = conn.transaction()?;
            let migration_result = migration.down(&tx);

            match migration_result {
                Ok(_) => {
                    // Commit the transaction
                    tx.commit()?;

                    // Delete this migration from the tracking table
                    conn.execute(
                        &format!(
                            "DELETE FROM {} WHERE version = ?1",
                            self.schema_version_table_name
                        ),
                        params![migration_version],
                    )?;

                    // record migration as rolled back
                    migrations_run.push(migration_version);
                }
                Err(e) => {
                    // Transaction will be automatically rolled back when dropped
                    failing_migration = Some(MigrationFailure {
                        migration,
                        error: e,
                    });
                    break;
                }
            }
        }

        Ok(MigrationReport {
            schema_version_table_existed,
            schema_version_table_created: false,
            failing_migration,
            migrations_run,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_successful_from_clean() {
        use chrono::{DateTime, FixedOffset};

        let mut conn = Connection::open_in_memory().unwrap();
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
        }
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
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
        // expect schema version table to exist and have recorded version 1
        let mut stmt = conn.prepare("SELECT * FROM _migratio_version_").unwrap();
        let rows = stmt
            .query_map([], |row| {
                let version: u32 = row.get("version").unwrap();
                let name: String = row.get("name").unwrap();
                let applied_at: String = row.get("applied_at").unwrap();
                Ok((version, name, applied_at))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 1); // version
        assert_eq!(rows[0].1, "Migration 1"); // name (default)
        let date_string_raw = &rows[0].2;
        let date = DateTime::parse_from_rfc3339(&date_string_raw).unwrap();
        assert_eq!(date.timezone(), FixedOffset::east_opt(0).unwrap());
        // ensure that the date is within 5 seconds of now
        // this assumes this test will not take >5 seconds to run
        let now = Utc::now();
        let diff = now.timestamp() - date.timestamp();
        assert!(diff < 5);
    }

    #[test]
    fn single_unsuccessful_from_clean() {
        let mut conn = Connection::open_in_memory().unwrap();
        // before running migration set up some data in the database to ensure it's preserved
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        conn.execute("INSERT INTO test (id) VALUES (1)", [])
            .unwrap();
        conn.execute("INSERT INTO test (id) VALUES (2)", [])
            .unwrap();
        // define a migration that will fail
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                // do something that works
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test2 (id) SELECT id FROM test", [])?;
                tx.execute("DROP TABLE test", [])?;
                // but then do something that fails
                tx.execute("bleep blorp", [])?;
                Ok(())
            }
        }
        // run migration, expecting failure
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(
            report,
            MigrationReport {
                schema_version_table_existed: false,
                schema_version_table_created: false,
                migrations_run: vec![],
                failing_migration: Some(MigrationFailure {
                    migration: &(Box::new(Migration1) as Box<dyn Migration>),
                    error: Error::Rusqlite(rusqlite::Error::SqlInputError {
                        error: rusqlite::ffi::Error {
                            code: rusqlite::ErrorCode::Unknown,
                            extended_code: 1
                        },
                        msg: "near \"bleep\": syntax error".to_string(),
                        sql: "bleep blorp".to_string(),
                        offset: 0
                    })
                })
            }
        );
        // expect sqlite database to be rolled back (transaction)
        // schema version table was created before the migration ran, so it persists
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap();
        let mut tables = stmt
            .query_map([], |x| {
                let name: String = x.get(0)?;
                Ok(name)
            })
            .unwrap()
            .collect::<Result<Vec<String>, rusqlite::Error>>()
            .unwrap();
        tables.sort();
        assert_eq!(
            tables,
            vec!["_migratio_version_".to_string(), "test".to_string()]
        );
        // expect data to be unchanged
        let mut stmt = conn.prepare("SELECT * FROM test").unwrap();
        let rows = stmt
            .query_map([], |x| {
                let x: i64 = x.get(0)?;
                Ok(x)
            })
            .unwrap()
            .collect::<Result<Vec<i64>, rusqlite::Error>>()
            .unwrap();
        assert_eq!(rows, vec![1, 2]);
    }

    #[test]
    fn success_then_failure_from_clean() {
        let mut conn = Connection::open_in_memory().unwrap();
        // before running migration set up some data in the database to ensure it's preserved
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        conn.execute("INSERT INTO test (id) VALUES (1)", [])
            .unwrap();
        conn.execute("INSERT INTO test (id) VALUES (2)", [])
            .unwrap();
        // define a migration that will succeed
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                // this will succeed
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                // move all data from test to test2
                tx.execute("INSERT INTO test2 (id) SELECT id FROM test", [])?;
                // drop test
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
        }
        // define a migration that will fail
        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                // do something that works
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test3 (id) SELECT id FROM test2", [])?;
                tx.execute("DROP TABLE test2", [])?;
                // then do something that fails
                tx.execute("bleep blorp", [])?;
                Ok(())
            }
        }
        // run migration, expecting failure
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(
            report,
            MigrationReport {
                schema_version_table_existed: false,
                schema_version_table_created: true,
                migrations_run: vec![1],
                failing_migration: Some(MigrationFailure {
                    migration: &(Box::new(Migration2) as Box<dyn Migration>),
                    error: Error::Rusqlite(rusqlite::Error::SqlInputError {
                        error: rusqlite::ffi::Error {
                            code: rusqlite::ErrorCode::Unknown,
                            extended_code: 1
                        },
                        msg: "near \"bleep\": syntax error".to_string(),
                        sql: "bleep blorp".to_string(),
                        offset: 0
                    })
                })
            }
        );
        // expect sqlite database to be left after migration 1
        // expect table names to be as expected
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap();
        let mut tables = stmt
            .query_map([], |x| {
                let name: String = x.get(0)?;
                Ok(name)
            })
            .unwrap()
            .collect::<Result<Vec<String>, rusqlite::Error>>()
            .unwrap();
        tables.sort();
        assert_eq!(
            tables,
            vec!["_migratio_version_".to_string(), "test2".to_string()]
        );
        // expect data to be as expected
        let mut stmt = conn.prepare("SELECT * FROM test2").unwrap();
        let rows = stmt
            .query_map([], |x| {
                let x: i64 = x.get(0)?;
                Ok(x)
            })
            .unwrap()
            .collect::<Result<Vec<i64>, rusqlite::Error>>()
            .unwrap();
        assert_eq!(rows, vec![1, 2]);
    }

    #[test]
    fn panic_in_migration_verify_state() {
        let mut conn = Connection::open_in_memory().unwrap();
        // before running migration set up some data in the database
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        conn.execute("INSERT INTO test (id) VALUES (1)", [])
            .unwrap();
        conn.execute("INSERT INTO test (id) VALUES (2)", [])
            .unwrap();
        // define a migration that will succeed
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                // this will succeed
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test2 (id) SELECT id FROM test", [])?;
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
        }
        // define a migration that will fail
        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                // do something that works
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test3 (id) SELECT id FROM test2", [])?;
                tx.execute("DROP TABLE test2", [])?;
                // then do something that fails
                tx.execute("bleep blorp", [])?;
                Ok(())
            }
        }
        // run migrations, expecting failure
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(
            report,
            MigrationReport {
                schema_version_table_existed: false,
                schema_version_table_created: true,
                migrations_run: vec![1],
                failing_migration: Some(MigrationFailure {
                    migration: &(Box::new(Migration2) as Box<dyn Migration>),
                    error: Error::Rusqlite(rusqlite::Error::SqlInputError {
                        error: rusqlite::ffi::Error {
                            code: rusqlite::ErrorCode::Unknown,
                            extended_code: 1
                        },
                        msg: "near \"bleep\": syntax error".to_string(),
                        sql: "bleep blorp".to_string(),
                        offset: 0
                    })
                })
            }
        );
        // With NoBackup AND transactions:
        // Migration1 succeeded and was committed, so those changes remain
        // Migration2 failed and was rolled back by the transaction
        // So the database should be left after Migration1 (test2 exists, test is gone)
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap();
        let mut tables = stmt
            .query_map([], |x| {
                let name: String = x.get(0)?;
                Ok(name)
            })
            .unwrap()
            .collect::<Result<Vec<String>, rusqlite::Error>>()
            .unwrap();
        tables.sort();
        assert_eq!(
            tables,
            vec!["_migratio_version_".to_string(), "test2".to_string()]
        );
        // expect data to still be in test2 (from Migration1)
        let mut stmt = conn.prepare("SELECT * FROM test2").unwrap();
        let rows = stmt
            .query_map([], |x| {
                let x: i64 = x.get(0)?;
                Ok(x)
            })
            .unwrap()
            .collect::<Result<Vec<i64>, rusqlite::Error>>()
            .unwrap();
        assert_eq!(rows, vec![1, 2]);
    }

    #[test]
    fn panic_with_successful_prior_migration() {
        use std::panic;

        let mut conn = Connection::open_in_memory().unwrap();
        // Set up initial data
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        conn.execute("INSERT INTO test (id) VALUES (1)", [])
            .unwrap();
        conn.execute("INSERT INTO test (id) VALUES (2)", [])
            .unwrap();

        // Define a migration that succeeds
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                // This migration succeeds
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test2 (id) SELECT id FROM test", [])?;
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
        }

        // Define a migration that panics
        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                // Make some changes
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test3 (id) SELECT id FROM test2", [])?;
                tx.execute("DROP TABLE test2", [])?;
                // Then panic
                panic!("Migration panic!");
            }
        }

        // Run migrations - catch the panic
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| migrator.upgrade(&mut conn)));

        // Verify panic occurred
        assert!(result.is_err());

        // When a panic occurs during Migration2:
        // Migration1 completed successfully and was committed
        // Migration2's transaction is automatically rolled back when the panic unwinds
        // The database should be in the state after Migration1 (test2 exists, test is gone)
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap();
        let mut tables = stmt
            .query_map([], |x| {
                let name: String = x.get(0)?;
                Ok(name)
            })
            .unwrap()
            .collect::<Result<Vec<String>, rusqlite::Error>>()
            .unwrap();
        tables.sort();

        // Assert current behavior: Migration1 succeeded, Migration2 rolled back
        assert_eq!(
            tables,
            vec!["_migratio_version_".to_string(), "test2".to_string()]
        );

        // Verify data from Migration1 is intact
        let mut stmt = conn.prepare("SELECT * FROM test2").unwrap();
        let rows = stmt
            .query_map([], |x| {
                let x: i64 = x.get(0)?;
                Ok(x)
            })
            .unwrap()
            .collect::<Result<Vec<i64>, rusqlite::Error>>()
            .unwrap();
        assert_eq!(rows, vec![1, 2]);

        // Verify schema_version table has version 1 (Migration1 completed, Migration2 never completed)
        let mut stmt = conn
            .prepare("SELECT version FROM _migratio_version_")
            .unwrap();
        let version: u32 = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn incremental_migrations_different_applied_at() {
        use std::thread;
        use std::time::Duration;

        let mut conn = Connection::open_in_memory().unwrap();

        // Define first set of migrations
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1_table".to_string()
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2_table".to_string()
            }
        }

        // Run first batch of migrations
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![1, 2]);

        // Get the applied_at timestamp for first batch
        let first_batch: Vec<(u32, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT version, name, applied_at FROM _migratio_version_ ORDER BY version",
                )
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        assert_eq!(first_batch.len(), 2);
        assert_eq!(first_batch[0].0, 1);
        assert_eq!(first_batch[0].1, "create_test1_table");
        assert_eq!(first_batch[1].0, 2);
        assert_eq!(first_batch[1].1, "create_test2_table");
        // Both migrations in first batch should have same timestamp
        assert_eq!(first_batch[0].2, first_batch[1].2);
        let first_batch_timestamp = first_batch[0].2.clone();

        // Wait a bit to ensure different timestamp
        thread::sleep(Duration::from_millis(2));

        // Define third migration
        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test3_table".to_string()
            }
        }

        // Run second batch with all three migrations (only migration 3 should run)
        let migrator = SqliteMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![3]);

        // Verify all three migrations are recorded
        let all_migrations: Vec<(u32, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT version, name, applied_at FROM _migratio_version_ ORDER BY version",
                )
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        assert_eq!(all_migrations.len(), 3);
        assert_eq!(all_migrations[0].0, 1);
        assert_eq!(all_migrations[1].0, 2);
        assert_eq!(all_migrations[2].0, 3);
        assert_eq!(all_migrations[2].1, "create_test3_table");

        // First two should have same timestamp (from first batch)
        assert_eq!(all_migrations[0].2, all_migrations[1].2);
        // Third should have different timestamp (from second batch)
        assert_ne!(all_migrations[2].2, first_batch_timestamp);
    }

    #[test]
    #[should_panic(expected = "Migration version must be greater than 0")]
    fn new_rejects_zero_version() {
        struct Migration0;
        impl Migration for Migration0 {
            fn version(&self) -> u32 {
                0
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let _ = SqliteMigrator::new(vec![Box::new(Migration0)]);
    }

    #[test]
    #[should_panic(expected = "Duplicate migration version found: 2")]
    fn new_rejects_duplicate_versions() {
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2a;
        impl Migration for Migration2a {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2b;
        impl Migration for Migration2b {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let _ = SqliteMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2a),
            Box::new(Migration2b),
        ]);
    }

    #[test]
    #[should_panic(expected = "Migration versions must start at 1, found starting version: 2")]
    fn new_rejects_non_starting_at_one() {
        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let _ = SqliteMigrator::new(vec![Box::new(Migration2), Box::new(Migration3)]);
    }

    #[test]
    #[should_panic(expected = "Migration versions must be contiguous. Expected version 2, found 3")]
    fn new_rejects_non_contiguous() {
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let _ = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration3)]);
    }

    #[test]
    fn try_new_returns_err_for_non_starting_at_one() {
        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let result = SqliteMigrator::try_new(vec![Box::new(Migration2)]);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Migration versions must start at 1, found starting version: 2"
        );
    }

    #[test]
    fn try_new_returns_err_for_duplicate_versions() {
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2a;
        impl Migration for Migration2a {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2b;
        impl Migration for Migration2b {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let result = SqliteMigrator::try_new(vec![
            Box::new(Migration1),
            Box::new(Migration2a),
            Box::new(Migration2b),
        ]);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Duplicate migration version found: 2");
    }

    #[test]
    fn try_new_returns_ok_for_valid_migrations() {
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let result = SqliteMigrator::try_new(vec![Box::new(Migration1), Box::new(Migration2)]);
        assert!(result.is_ok());
    }

    #[test]
    fn checksum_validation_detects_modified_migration() {
        let mut conn = Connection::open_in_memory().unwrap();

        // Define initial migration
        struct Migration1V1;
        impl Migration for Migration1V1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test_table".to_string()
            }
        }

        // Run first migration
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1V1)]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![1]);

        // Now define the same migration but with a different name (simulating modification)
        struct Migration1V2;
        impl Migration for Migration1V2 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test_table_modified".to_string() // Different name!
            }
        }

        // Try to upgrade with modified migration - should fail
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1V2)]);
        let result = migrator.upgrade(&mut conn);

        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("checksum mismatch"));
        assert!(err_msg.contains("Migration 1"));
    }

    #[test]
    fn checksum_validation_passes_for_unmodified_migrations() {
        let mut conn = Connection::open_in_memory().unwrap();

        // Define migrations
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1_table".to_string()
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2_table".to_string()
            }
        }

        // Run first migration
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![1]);

        // Run both migrations (only second should execute)
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![2]);

        // Run again - should succeed with no migrations run (validates checksums are still correct)
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![] as Vec<u32>);
    }

    #[test]
    fn checksums_stored_in_database() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "my_migration".to_string()
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Verify checksum was stored
        let mut stmt = conn
            .prepare("SELECT checksum FROM _migratio_version_ WHERE version = 1")
            .unwrap();
        let checksum: String = stmt.query_row([], |row| row.get(0)).unwrap();

        // Checksum should be a non-empty hex string (SHA-256 = 64 chars)
        assert_eq!(checksum.len(), 64);
        assert!(checksum.chars().all(|c| c.is_ascii_hexdigit()));

        // Verify it matches the calculated checksum
        let migration = Box::new(Migration1) as Box<dyn Migration>;
        let expected_checksum = SqliteMigrator::calculate_checksum(&migration);
        assert_eq!(checksum, expected_checksum);
    }

    #[test]
    fn downgrade_single_migration() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
        }

        // Apply migration
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![1]);

        // Verify table exists
        {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='test'")
                .unwrap();
            assert!(stmt.query([]).unwrap().next().unwrap().is_some());
        }

        // Rollback migration
        let report = migrator.downgrade(&mut conn, 0).unwrap();
        assert_eq!(report.migrations_run, vec![1]);

        // Verify table is gone
        {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='test'")
                .unwrap();
            assert!(stmt.query([]).unwrap().next().unwrap().is_none());
        }

        // Verify tracking table shows no migrations
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM _migratio_version_")
            .unwrap();
        let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn downgrade_multiple_migrations() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test3", [])?;
                Ok(())
            }
        }

        // Apply all migrations
        let migrator = SqliteMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![1, 2, 3]);

        // Rollback to version 1 (should rollback migrations 3 and 2, in that order)
        let report = migrator.downgrade(&mut conn, 1).unwrap();
        assert_eq!(report.migrations_run, vec![3, 2]);

        // Verify only test1 table exists
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tables, vec!["_migratio_version_", "test1"]);

        // Verify tracking table shows only migration 1
        let mut stmt = conn
            .prepare("SELECT version FROM _migratio_version_ ORDER BY version")
            .unwrap();
        let versions: Vec<u32> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(versions, vec![1]);
    }

    #[test]
    fn downgrade_all_migrations() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
        }

        // Apply migrations
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![1, 2]);

        // Rollback all migrations (target_version = 0)
        let report = migrator.downgrade(&mut conn, 0).unwrap();
        assert_eq!(report.migrations_run, vec![2, 1]);

        // Verify no tables except tracking table
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tables, vec!["_migratio_version_"]);

        // Verify tracking table is empty
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM _migratio_version_")
            .unwrap();
        let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn downgrade_on_clean_database() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            fn down(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        // Downgrade on clean database should succeed with no migrations run
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.downgrade(&mut conn, 0).unwrap();
        assert_eq!(report.migrations_run, vec![] as Vec<u32>);
        assert!(!report.schema_version_table_existed);
    }

    #[test]
    fn downgrade_with_invalid_target_version() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
        }

        // Apply migration
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Try to downgrade to a version higher than current (should fail)
        let result = migrator.downgrade(&mut conn, 5);
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("Cannot downgrade to version 5 when current version is 1"));
    }

    #[test]
    #[should_panic(expected = "does not support downgrade")]
    fn downgrade_panics_when_down_not_implemented() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            // No down() implementation - uses default that panics
        }

        // Apply migration
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Try to downgrade - should panic
        let _ = migrator.downgrade(&mut conn, 0);
    }

    #[test]
    fn downgrade_validates_checksums() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1V1;
        impl Migration for Migration1V1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test_table".to_string()
            }
        }

        // Apply migration
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1V1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Try to downgrade with modified migration
        struct Migration1V2;
        impl Migration for Migration1V2 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test_table_modified".to_string() // Different!
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1V2)]);
        let result = migrator.downgrade(&mut conn, 0);

        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("checksum mismatch"));
    }

    #[test]
    fn get_current_version_on_clean_database() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 0);
    }

    #[test]
    fn get_current_version_after_migrations() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);

        // Before any migrations
        assert_eq!(migrator.get_current_version(&mut conn).unwrap(), 0);

        // After first migration
        migrator.upgrade(&mut conn).unwrap();
        assert_eq!(migrator.get_current_version(&mut conn).unwrap(), 2);
    }

    #[test]
    fn preview_upgrade_on_clean_database() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let pending = migrator.preview_upgrade(&mut conn).unwrap();

        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].version(), 1);
        assert_eq!(pending[1].version(), 2);
    }

    #[test]
    fn preview_upgrade_with_partial_migrations() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
        }

        // Apply first migration
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Preview with all three migrations
        let migrator = SqliteMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);
        let pending = migrator.preview_upgrade(&mut conn).unwrap();

        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].version(), 2);
        assert_eq!(pending[1].version(), 3);
    }

    #[test]
    fn preview_upgrade_when_up_to_date() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        let pending = migrator.preview_upgrade(&mut conn).unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[test]
    fn preview_downgrade_to_zero() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        migrator.upgrade(&mut conn).unwrap();

        let to_rollback = migrator.preview_downgrade(&mut conn, 0).unwrap();

        assert_eq!(to_rollback.len(), 2);
        assert_eq!(to_rollback[0].version(), 2); // Reverse order
        assert_eq!(to_rollback[1].version(), 1);
    }

    #[test]
    fn preview_downgrade_to_specific_version() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test3", [])?;
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);
        migrator.upgrade(&mut conn).unwrap();

        let to_rollback = migrator.preview_downgrade(&mut conn, 1).unwrap();

        assert_eq!(to_rollback.len(), 2);
        assert_eq!(to_rollback[0].version(), 3); // Reverse order
        assert_eq!(to_rollback[1].version(), 2);
    }

    #[test]
    fn preview_downgrade_on_clean_database() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            fn down(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        let to_rollback = migrator.preview_downgrade(&mut conn, 0).unwrap();

        assert_eq!(to_rollback.len(), 0);
    }

    #[test]
    fn preview_downgrade_with_invalid_target() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        let result = migrator.preview_downgrade(&mut conn, 5);
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("Cannot downgrade to version 5 when current version is 1"));
    }

    #[test]
    fn migration_failure_accessors() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test_table".to_string()
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                // Intentionally fail
                tx.execute("INVALID SQL HERE", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "failing_migration".to_string()
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        // Check that we can access the failure
        assert!(report.failing_migration.is_some());

        let failure = report.failing_migration.as_ref().unwrap();

        // Test migration accessor
        let migration = failure.migration();
        assert_eq!(migration.version(), 2);
        assert_eq!(migration.name(), "failing_migration");

        // Test error accessor
        let error = failure.error();
        assert!(matches!(error, Error::Rusqlite(_)));
    }

    #[test]
    fn migration_failure_can_be_pattern_matched() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("INVALID SQL", [])?;
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        // Demonstrate pattern matching on the failure
        match report.failing_migration {
            Some(ref failure) => {
                println!(
                    "Migration {} failed: {:?}",
                    failure.migration().version(),
                    failure.error()
                );
                assert_eq!(failure.migration().version(), 1);
            }
            None => panic!("Expected a failure"),
        }
    }

    #[test]
    fn get_migration_history_on_clean_database() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        let history = migrator.get_migration_history(&mut conn).unwrap();
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn get_migration_history_after_migrations() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1_table".to_string()
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2_table".to_string()
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        migrator.upgrade(&mut conn).unwrap();

        let history = migrator.get_migration_history(&mut conn).unwrap();

        assert_eq!(history.len(), 2);

        // Check first migration
        assert_eq!(history[0].version, 1);
        assert_eq!(history[0].name, "create_test1_table");
        assert!(history[0].applied_at.timestamp() > 0);
        assert_eq!(history[0].checksum.len(), 64); // SHA-256 hex string

        // Check second migration
        assert_eq!(history[1].version, 2);
        assert_eq!(history[1].name, "create_test2_table");
        assert!(history[1].applied_at.timestamp() > 0);
        assert_eq!(history[1].checksum.len(), 64);

        // Both should have same applied_at timestamp (same batch)
        assert_eq!(history[0].applied_at, history[1].applied_at);
    }

    #[test]
    fn get_migration_history_shows_incremental_batches() {
        use std::thread;
        use std::time::Duration;

        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "migration_one".to_string()
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "migration_two".to_string()
            }
        }

        // Run first migration
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        let history = migrator.get_migration_history(&mut conn).unwrap();
        assert_eq!(history.len(), 1);
        let first_timestamp = history[0].applied_at;

        // Wait a bit to ensure different timestamp
        thread::sleep(Duration::from_millis(2));

        // Run second migration in a new batch
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        migrator.upgrade(&mut conn).unwrap();

        let history = migrator.get_migration_history(&mut conn).unwrap();
        assert_eq!(history.len(), 2);

        // First migration timestamp should be unchanged
        assert_eq!(history[0].applied_at, first_timestamp);

        // Second migration should have different timestamp
        assert_ne!(history[1].applied_at, first_timestamp);
    }

    #[test]
    fn get_migration_history_includes_checksums() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "test_migration".to_string()
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        let history = migrator.get_migration_history(&mut conn).unwrap();
        assert_eq!(history.len(), 1);

        // Verify checksum matches what would be calculated
        let migration = Box::new(Migration1) as Box<dyn Migration>;
        let expected_checksum = SqliteMigrator::calculate_checksum(&migration);
        assert_eq!(history[0].checksum, expected_checksum);
    }

    #[test]
    fn concurrent_migrations_are_safe() {
        use std::sync::{Arc, Barrier};
        use std::thread;
        use tempfile::NamedTempFile;

        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap().to_string();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                // Simulate some work
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                std::thread::sleep(std::time::Duration::from_millis(10));
                tx.execute("INSERT INTO test1 (id) VALUES (1)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                std::thread::sleep(std::time::Duration::from_millis(10));
                tx.execute("INSERT INTO test2 (id) VALUES (1)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
        }

        // Create a barrier to synchronize thread starts
        let barrier = Arc::new(Barrier::new(3));
        let db_path_arc = Arc::new(db_path);

        // Spawn 3 threads that all try to run migrations concurrently
        let handles: Vec<_> = (0..3)
            .map(|i| {
                let barrier = Arc::clone(&barrier);
                let db_path = Arc::clone(&db_path_arc);
                thread::spawn(move || {
                    // Wait for all threads to be ready
                    barrier.wait();

                    // Each thread tries to upgrade
                    let mut conn = Connection::open(db_path.as_str()).unwrap();
                    let migrator =
                        SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
                    let result = migrator.upgrade(&mut conn);

                    // All threads should succeed (busy timeout allows them to wait)
                    assert!(result.is_ok(), "Thread {} failed: {:?}", i, result);

                    // Return only the migrations_run count
                    result.unwrap().migrations_run.len()
                })
            })
            .collect();

        // Wait for all threads to complete
        let migrations_run_counts: Vec<_> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();

        let two_migrations = migrations_run_counts.iter().filter(|&&c| c == 2).count();
        let zero_migrations = migrations_run_counts.iter().filter(|&&c| c == 0).count();

        assert_eq!(
            two_migrations, 1,
            "Exactly one thread should have run 2 migrations"
        );
        assert_eq!(
            zero_migrations, 2,
            "Two threads should have found db already migrated"
        );

        // Verify final state
        let mut conn = Connection::open(db_path_arc.as_str()).unwrap();
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let current_version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(current_version, 2);

        // Verify both tables exist and have data
        let count1: i64 = conn
            .query_row("SELECT COUNT(*) FROM test1", [], |row| row.get(0))
            .unwrap();
        let count2: i64 = conn
            .query_row("SELECT COUNT(*) FROM test2", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count1, 1);
        assert_eq!(count2, 1);
    }

    #[test]
    fn concurrent_upgrade_and_downgrade() {
        use std::sync::{Arc, Barrier};
        use std::thread;
        use tempfile::NamedTempFile;

        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap().to_string();

        // First, apply some migrations
        {
            let mut conn = Connection::open(&db_path).unwrap();

            struct Migration1;
            impl Migration for Migration1 {
                fn version(&self) -> u32 {
                    1
                }
                fn up(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                    Ok(())
                }
                fn down(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("DROP TABLE test1", [])?;
                    Ok(())
                }
            }

            struct Migration2;
            impl Migration for Migration2 {
                fn version(&self) -> u32 {
                    2
                }
                fn up(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                    Ok(())
                }
                fn down(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("DROP TABLE test2", [])?;
                    Ok(())
                }
            }

            let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
            migrator.upgrade(&mut conn).unwrap();
        }

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
        }

        // Now spawn threads - some upgrading, some downgrading
        let barrier = Arc::new(Barrier::new(4));
        let db_path_arc = Arc::new(db_path);

        let handles: Vec<_> = (0..4)
            .map(|i| {
                let barrier = Arc::clone(&barrier);
                let db_path = Arc::clone(&db_path_arc);
                thread::spawn(move || {
                    barrier.wait();

                    let mut conn = Connection::open(db_path.as_str()).unwrap();
                    let migrator =
                        SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);

                    let result = if i % 2 == 0 {
                        // Even threads try to upgrade
                        migrator.upgrade(&mut conn)
                    } else {
                        // Odd threads try to downgrade to version 1
                        migrator.downgrade(&mut conn, 1)
                    };

                    // Assert success and return a simple value instead of the result
                    assert!(result.is_ok(), "Thread {} got error: {:?}", i, result);
                    true
                })
            })
            .collect();

        // All threads should complete successfully without panicking
        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.join();
            assert!(result.is_ok(), "Thread {} panicked", i);
            assert!(result.unwrap(), "Thread {} failed", i);
        }

        // Database should be in a valid state (either version 1 or 2)
        let mut conn = Connection::open(db_path_arc.as_str()).unwrap();
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let current_version = migrator.get_current_version(&mut conn).unwrap();
        assert!(
            current_version == 1 || current_version == 2,
            "Version should be 1 or 2, got {}",
            current_version
        );
    }

    #[test]
    fn custom_busy_timeout() {
        use tempfile::NamedTempFile;

        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap().to_string();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
        }

        // Create migrator with custom 5-second timeout
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
            .with_busy_timeout(std::time::Duration::from_secs(5));

        // Verify it can run migrations successfully
        let mut conn = Connection::open(&db_path).unwrap();
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![1]);

        // Verify the custom timeout was applied (we can't directly test the timeout value,
        // but we can verify the migration completed successfully with the custom setting)
        let current_version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(current_version, 1);
    }
}
