use crate::error::Error;
use chrono::Utc;
use rusqlite::{params, Connection, Transaction};

/// Represents a failure during a migration.
#[derive(Debug, PartialEq)]
pub struct MigrationFailure<'migration> {
    migration: &'migration Box<dyn Migration>,
    error: Error,
}

/// A report of actions performed during a migration.
#[derive(Debug, PartialEq)]
pub struct MigrationReport<'migration> {
    pub schema_version_table_existed: bool,
    pub schema_version_table_created: bool,
    pub migrations_run: Vec<u32>,
    pub failing_migration: Option<MigrationFailure<'migration>>,
}

const DEFAULT_VERSION_TABLE_NAME: &str = "_migratio_version_";

/// A trait that must be implemented to define a migration.
/// The `version` value must be unique among all migrations supplied to the migrator, and greater than 0.
/// Implement your migration logic in the `up` method, using the supplied [Transaction] to perform database operations.
/// The transaction will be automatically committed if the migration succeeds, or rolled back if it fails.
/// The `name` and `description` methods are optional, and only aid in debugging / observability.
pub trait Migration {
    fn version(&self) -> u32;

    fn up(&self, tx: &Transaction) -> Result<(), Error>;

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
}

impl SqliteMigrator {
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
    pub fn with_schema_version_table_name(mut self, name: String) -> Self {
        self.schema_version_table_name = name;
        self
    }

    /// Apply all previously-unapplied [Migration]s to the database with the given [Connection].
    /// Each migration runs within its own transaction, which is automatically rolled back if the migration fails.
    pub fn upgrade(&self, conn: &mut Connection) -> Result<MigrationReport<'_>, Error> {
        // if schema version tracking table does not exist, create it
        let schema_version_table_existed = {
            let mut stmt =
                conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name=?1")?;
            let schema_version_table_existed = stmt
                .query([&self.schema_version_table_name])?
                .next()?
                .is_some();
            if !schema_version_table_existed {
                // create table with name column
                conn.execute(
                &format!(
                    "CREATE TABLE {} (version integer primary key not null, name text not null, applied_at text not null)",
                    self.schema_version_table_name
                ),
                [],)?;
            }
            schema_version_table_existed
        };
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

                        // Insert a row for this migration with its name and the batch timestamp
                        conn.execute(
                            &format!(
                                "INSERT INTO {} (version, name, applied_at) VALUES(?1, ?2, ?3)",
                                self.schema_version_table_name
                            ),
                            params![migration_version, migration.name(), &batch_applied_at],
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
}
