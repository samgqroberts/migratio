//! Testing utilities for migration development and verification.
//!
//! This module provides a test harness for writing comprehensive migration tests,
//! including data transformation tests, schema validation, and reversibility checks.

use crate::{Error, SqliteMigrator};
use rusqlite::{Connection, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A test harness for migration testing that provides state control and assertion helpers.
///
/// # Example
///
/// ```
/// use migratio::testing::MigrationTestHarness;
/// use migratio::{Migration, SqliteMigrator, Error};
/// use rusqlite::Transaction;
///
/// struct Migration1;
/// impl Migration for Migration1 {
///     fn version(&self) -> u32 { 1 }
///     fn up(&self, tx: &Transaction) -> Result<(), Error> {
///         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
///         Ok(())
///     }
///     fn down(&self, tx: &Transaction) -> Result<(), Error> {
///         tx.execute("DROP TABLE users", [])?;
///         Ok(())
///     }
/// }
///
/// # fn test() -> Result<(), Error> {
/// let mut harness = MigrationTestHarness::new(SqliteMigrator::new(vec![Box::new(Migration1)]));
///
/// // Migrate to version 1
/// harness.migrate_to(1)?;
///
/// // Insert test data
/// harness.execute("INSERT INTO users VALUES (1, 'alice')")?;
///
/// // Assert table exists
/// harness.assert_table_exists("users")?;
///
/// // Query data
/// let name: String = harness.query_one("SELECT name FROM users WHERE id = 1")?;
/// assert_eq!(name, "alice");
/// # Ok(())
/// # }
/// ```
pub struct MigrationTestHarness {
    conn: Connection,
    migrator: SqliteMigrator,
}

/// Represents a captured database schema for comparison and snapshotting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaSnapshot {
    /// Map of table name to table definitions
    pub tables: HashMap<String, TableSchema>,
}

/// Represents a table's schema.
/// TODO remove sql from this. if one (manual) setup script constructed the table with newlines
/// vs the migration script without newlines, this equality check would fail, but that should not matter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableSchema {
    /// SQL CREATE statement for the table
    pub sql: String,
    /// List of columns
    pub columns: Vec<ColumnInfo>,
    /// List of indexes
    pub indexes: Vec<IndexInfo>,
}

/// Information about a column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
    pub not_null: bool,
    pub default_value: Option<String>,
    pub primary_key: bool,
}

/// Information about an index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub sql: String,
}

impl MigrationTestHarness {
    /// Create a new test harness with the given migrator.
    /// This should be the same migrator that is used in the production environment:
    /// as it changes, asserts on previous migrations SHOULD NOT CHANGE.
    ///
    /// It is recommended to have a function somewhere that constructs the migrator, eg:
    /// ```ignore
    /// fn migrator() -> SqliteMigrator {
    ///     SqliteMigrator::new(vec![
    ///         Box::new(Migration1),
    ///         Box::new(Migration2),
    ///     ])
    /// }
    /// ```
    ///
    /// and then in each test, construct the harness like:
    /// ```ignore
    /// let harness = MigrationTestHarness::new(migrator());
    /// ```
    ///
    /// Uses an in-memory SQLite database by default.
    pub fn new(migrator: SqliteMigrator) -> Self {
        let conn = Connection::open_in_memory().expect("Failed to create in-memory test database");
        Self { conn, migrator }
    }

    /// Create a test harness with a custom connection.
    /// Useful for testing with file-based databases or custom settings.
    ///
    /// See [MigrationTestHarness::new] for more information.
    pub fn with_connection(conn: Connection, migrator: SqliteMigrator) -> Self {
        Self { conn, migrator }
    }

    /// Migrate to a specific version.
    ///
    /// Returns an error if the target version does not exist in the migration list.
    pub fn migrate_to(&mut self, target_version: u32) -> Result<(), Error> {
        // Validate target version exists (version 0 is always valid for empty state)
        if target_version > 0 {
            let version_exists = self
                .migrator
                .migrations()
                .iter()
                .any(|m| m.version() == target_version);
            if !version_exists {
                return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                    format!(
                        "Migration version {} does not exist. Available versions: {}",
                        target_version,
                        self.migrator
                            .migrations()
                            .iter()
                            .map(|m| m.version().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )));
            }
        }

        let current = self.current_version()?;

        if target_version > current {
            // Migrate up to target version
            self.migrator.upgrade_to(&mut self.conn, target_version)?;
        } else if target_version < current {
            // Migrate down
            self.migrator.downgrade(&mut self.conn, target_version)?;
        }

        Ok(())
    }

    /// Migrate up by exactly one migration.
    ///
    /// Note: This will run all pending migrations and verify exactly one was run.
    /// If you have multiple pending migrations and only want to run one, use `migrate_to()` instead.
    pub fn migrate_up_one(&mut self) -> Result<(), Error> {
        let current = self.current_version()?;
        let target = current + 1;

        // Use migrate_to to only go up one version
        self.migrate_to(target)?;

        Ok(())
    }

    /// Migrate down by exactly one migration.
    pub fn migrate_down_one(&mut self) -> Result<(), Error> {
        let current = self.current_version()?;
        if current == 0 {
            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                "Already at version 0, cannot migrate down".to_string(),
            )));
        }

        self.migrator.downgrade(&mut self.conn, current - 1)?;
        Ok(())
    }

    /// Get the current migration version.
    pub fn current_version(&mut self) -> Result<u32, Error> {
        self.migrator.get_current_version(&mut self.conn)
    }

    /// Execute a SQL statement (for setting up test data).
    pub fn execute(&mut self, sql: &str) -> Result<(), Error> {
        self.conn.execute(sql, [])?;
        Ok(())
    }

    /// Query a single value from the database.
    pub fn query_one<T>(&mut self, sql: &str) -> Result<T, Error>
    where
        T: rusqlite::types::FromSql,
    {
        let result = self.conn.query_row(sql, [], |row| row.get(0))?;
        Ok(result)
    }

    /// Query all values from a single-column result.
    pub fn query_all<T>(&mut self, sql: &str) -> Result<Vec<T>, Error>
    where
        T: rusqlite::types::FromSql,
    {
        let mut stmt = self.conn.prepare(sql)?;
        let results = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<T>, _>>()?;
        Ok(results)
    }

    /// Query with a custom row mapper.
    pub fn query_map<T, F>(&mut self, sql: &str, f: F) -> Result<Vec<T>, Error>
    where
        F: FnMut(&Row) -> rusqlite::Result<T>,
    {
        let mut stmt = self.conn.prepare(sql)?;
        let results = stmt.query_map([], f)?.collect::<Result<Vec<T>, _>>()?;
        Ok(results)
    }

    /// Assert that a table exists in the database.
    pub fn assert_table_exists(&mut self, table_name: &str) -> Result<(), Error> {
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table_name],
            |row| row.get(0),
        )?;

        if count == 0 {
            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                format!("Table '{}' does not exist", table_name),
            )));
        }

        Ok(())
    }

    /// Assert that a table does not exist in the database.
    pub fn assert_table_not_exists(&mut self, table_name: &str) -> Result<(), Error> {
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table_name],
            |row| row.get(0),
        )?;

        if count > 0 {
            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                format!("Table '{}' exists but should not", table_name),
            )));
        }

        Ok(())
    }

    /// Assert that a column exists in a table.
    pub fn assert_column_exists(
        &mut self,
        table_name: &str,
        column_name: &str,
    ) -> Result<(), Error> {
        let columns = self.get_columns(table_name)?;

        if !columns.iter().any(|c| c.name == column_name) {
            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                format!(
                    "Column '{}' does not exist in table '{}'",
                    column_name, table_name
                ),
            )));
        }

        Ok(())
    }

    /// Assert that an index exists.
    pub fn assert_index_exists(&mut self, index_name: &str) -> Result<(), Error> {
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
            [index_name],
            |row| row.get(0),
        )?;

        if count == 0 {
            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                format!("Index '{}' does not exist", index_name),
            )));
        }

        Ok(())
    }

    /// Capture the current database schema as a snapshot.
    pub fn capture_schema(&mut self) -> Result<SchemaSnapshot, Error> {
        let mut tables = HashMap::new();

        // Get all user tables (exclude sqlite internal tables and migration table)
        let table_names: Vec<String> = self.conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name != '_migratio_version_'")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        for table_name in table_names {
            let sql: String = self.conn.query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                [&table_name],
                |row| row.get(0),
            )?;

            // Normalize SQL to avoid quoting differences
            let normalized_sql = sql.replace("\"", "");

            let columns = self.get_columns(&table_name)?;
            let indexes = self.get_indexes(&table_name)?;

            tables.insert(
                table_name,
                TableSchema {
                    sql: normalized_sql,
                    columns,
                    indexes,
                },
            );
        }

        Ok(SchemaSnapshot { tables })
    }

    /// Assert that the current schema matches a previously captured snapshot.
    /// TODO this should provide much better error message than the broad debug left != right default
    pub fn assert_schema_matches(&mut self, expected: &SchemaSnapshot) -> Result<(), Error> {
        let actual = self.capture_schema()?;

        if actual != *expected {
            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                format!(
                    "Schema mismatch.\nExpected: {:#?}\nActual: {:#?}",
                    expected, actual
                ),
            )));
        }

        Ok(())
    }

    /// Get column information for a table.
    fn get_columns(&mut self, table_name: &str) -> Result<Vec<ColumnInfo>, Error> {
        let mut stmt = self
            .conn
            .prepare(&format!("PRAGMA table_info({})", table_name))?;
        let columns = stmt
            .query_map([], |row| {
                Ok(ColumnInfo {
                    name: row.get(1)?,
                    type_name: row.get(2)?,
                    not_null: row.get::<_, i32>(3)? != 0,
                    default_value: row.get(4)?,
                    primary_key: row.get::<_, i32>(5)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(columns)
    }

    /// Get index information for a table.
    fn get_indexes(&mut self, table_name: &str) -> Result<Vec<IndexInfo>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT name, \"unique\", sql FROM sqlite_master WHERE type='index' AND tbl_name=?1 AND sql IS NOT NULL"
        )?;

        let indexes = stmt
            .query_map([table_name], |row| {
                Ok(IndexInfo {
                    name: row.get(0)?,
                    unique: row.get::<_, i32>(1)? != 0,
                    sql: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(indexes)
    }

    /// Get a reference to the underlying connection for advanced usage.
    pub fn connection(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

#[cfg(test)]
mod tests {
    use crate::Migration;

    use super::*;
    use rusqlite::Transaction;

    struct TestMigration1;
    impl Migration for TestMigration1 {
        fn version(&self) -> u32 {
            1
        }
        fn up(&self, tx: &Transaction) -> Result<(), Error> {
            tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
            Ok(())
        }
        fn down(&self, tx: &Transaction) -> Result<(), Error> {
            tx.execute("DROP TABLE users", [])?;
            Ok(())
        }
        fn name(&self) -> String {
            "create_users_table".to_string()
        }
    }

    struct TestMigration2;
    impl Migration for TestMigration2 {
        fn version(&self) -> u32 {
            2
        }
        fn up(&self, tx: &Transaction) -> Result<(), Error> {
            tx.execute("ALTER TABLE users ADD COLUMN email TEXT", [])?;
            Ok(())
        }
        fn down(&self, tx: &Transaction) -> Result<(), Error> {
            tx.execute(
                "CREATE TABLE users_temp (id INTEGER PRIMARY KEY, name TEXT)",
                [],
            )?;
            tx.execute("INSERT INTO users_temp SELECT id, name FROM users", [])?;
            tx.execute("DROP TABLE users", [])?;
            tx.execute("ALTER TABLE users_temp RENAME TO users", [])?;
            Ok(())
        }
        fn name(&self) -> String {
            "add_email_column".to_string()
        }
    }

    struct TestMigration3;
    impl Migration for TestMigration3 {
        fn version(&self) -> u32 {
            3
        }
        fn up(&self, tx: &Transaction) -> Result<(), Error> {
            tx.execute("CREATE INDEX idx_users_email ON users(email)", [])?;
            Ok(())
        }
        fn down(&self, tx: &Transaction) -> Result<(), Error> {
            tx.execute("DROP INDEX idx_users_email", [])?;
            Ok(())
        }
        fn name(&self) -> String {
            "add_email_index".to_string()
        }
    }

    #[test]
    fn test_migrate_to() {
        let mut harness = MigrationTestHarness::new(SqliteMigrator::new(vec![
            Box::new(TestMigration1),
            Box::new(TestMigration2),
            Box::new(TestMigration3),
        ]));

        assert_eq!(harness.current_version().unwrap(), 0);

        harness.migrate_to(2).unwrap();
        assert_eq!(harness.current_version().unwrap(), 2);

        harness.migrate_to(1).unwrap();
        assert_eq!(harness.current_version().unwrap(), 1);

        harness.migrate_to(3).unwrap();
        assert_eq!(harness.current_version().unwrap(), 3);
    }

    #[test]
    fn test_migrate_to_nonexistent_version() {
        let mut harness = MigrationTestHarness::new(SqliteMigrator::new(vec![
            Box::new(TestMigration1),
            Box::new(TestMigration2),
        ]));

        let result = harness.migrate_to(5);
        assert!(result.is_err());

        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("Migration version 5 does not exist"));
        assert!(err_msg.contains("Available versions: 1, 2"));
    }

    #[test]
    fn test_migrate_up_one() {
        let mut harness = MigrationTestHarness::new(SqliteMigrator::new(vec![
            Box::new(TestMigration1),
            Box::new(TestMigration2),
        ]));

        assert_eq!(harness.current_version().unwrap(), 0);

        harness.migrate_up_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 1);

        harness.migrate_up_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 2);
    }

    #[test]
    fn test_migrate_down_one() {
        let mut harness = MigrationTestHarness::new(SqliteMigrator::new(vec![
            Box::new(TestMigration1),
            Box::new(TestMigration2),
        ]));

        harness.migrate_to(2).unwrap();
        assert_eq!(harness.current_version().unwrap(), 2);

        harness.migrate_down_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 1);

        harness.migrate_down_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 0);
    }

    #[test]
    fn test_execute_and_query() {
        let mut harness =
            MigrationTestHarness::new(SqliteMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        harness
            .execute("INSERT INTO users (id, name) VALUES (1, 'alice')")
            .unwrap();

        let name: String = harness
            .query_one("SELECT name FROM users WHERE id = 1")
            .unwrap();
        assert_eq!(name, "alice");
    }

    #[test]
    fn test_query_all() {
        let mut harness =
            MigrationTestHarness::new(SqliteMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        harness
            .execute("INSERT INTO users (id, name) VALUES (1, 'alice')")
            .unwrap();
        harness
            .execute("INSERT INTO users (id, name) VALUES (2, 'bob')")
            .unwrap();

        let names: Vec<String> = harness
            .query_all("SELECT name FROM users ORDER BY id")
            .unwrap();
        assert_eq!(names, vec!["alice", "bob"]);
    }

    #[test]
    fn test_assert_table_exists() {
        let mut harness =
            MigrationTestHarness::new(SqliteMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        harness.assert_table_exists("users").unwrap();

        let result = harness.assert_table_exists("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_assert_table_not_exists() {
        let mut harness =
            MigrationTestHarness::new(SqliteMigrator::new(vec![Box::new(TestMigration1)]));

        harness.assert_table_not_exists("users").unwrap();

        harness.migrate_to(1).unwrap();
        let result = harness.assert_table_not_exists("users");
        assert!(result.is_err());
    }

    #[test]
    fn test_assert_column_exists() {
        let mut harness = MigrationTestHarness::new(SqliteMigrator::new(vec![
            Box::new(TestMigration1),
            Box::new(TestMigration2),
        ]));

        harness.migrate_to(1).unwrap();
        harness.assert_column_exists("users", "name").unwrap();

        let result = harness.assert_column_exists("users", "email");
        assert!(result.is_err());

        harness.migrate_to(2).unwrap();
        harness.assert_column_exists("users", "email").unwrap();
    }

    #[test]
    fn test_assert_index_exists() {
        let mut harness = MigrationTestHarness::new(SqliteMigrator::new(vec![
            Box::new(TestMigration1),
            Box::new(TestMigration2),
            Box::new(TestMigration3),
        ]));

        harness.migrate_to(2).unwrap();
        let result = harness.assert_index_exists("idx_users_email");
        assert!(result.is_err());

        harness.migrate_to(3).unwrap();
        harness.assert_index_exists("idx_users_email").unwrap();
    }

    #[test]
    fn test_capture_schema() {
        let mut harness = MigrationTestHarness::new(SqliteMigrator::new(vec![
            Box::new(TestMigration1),
            Box::new(TestMigration2),
        ]));

        harness.migrate_to(2).unwrap();
        let snapshot = harness.capture_schema().unwrap();

        assert!(snapshot.tables.contains_key("users"));
        let users_table = &snapshot.tables["users"];
        assert_eq!(users_table.columns.len(), 3); // id, name, email
        assert!(users_table.columns.iter().any(|c| c.name == "id"));
        assert!(users_table.columns.iter().any(|c| c.name == "name"));
        assert!(users_table.columns.iter().any(|c| c.name == "email"));
    }

    #[test]
    fn test_schema_reversibility() {
        let mut harness = MigrationTestHarness::new(SqliteMigrator::new(vec![
            Box::new(TestMigration1),
            Box::new(TestMigration2),
        ]));

        // Capture schema at version 2
        harness.migrate_to(2).unwrap();
        let schema_at_2 = harness.capture_schema().unwrap();

        // Go back to version 1
        harness.migrate_to(1).unwrap();

        // Go back up to version 2
        harness.migrate_to(2).unwrap();
        let schema_at_2_again = harness.capture_schema().unwrap();

        // Should be identical
        assert_eq!(schema_at_2, schema_at_2_again);
    }

    #[test]
    fn test_assert_schema_matches() {
        let mut harness =
            MigrationTestHarness::new(SqliteMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        let snapshot = harness.capture_schema().unwrap();

        // Should match itself
        harness.assert_schema_matches(&snapshot).unwrap();

        // Add a column - should no longer match
        harness
            .execute("ALTER TABLE users ADD COLUMN age INTEGER")
            .unwrap();
        let result = harness.assert_schema_matches(&snapshot);
        assert!(result.is_err());
    }

    #[test]
    fn test_data_transformation() {
        struct DataTransformMigration1;
        impl Migration for DataTransformMigration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE prefs (name TEXT PRIMARY KEY, data TEXT)", [])?;
                Ok(())
            }
            fn down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE prefs", [])?;
                Ok(())
            }
        }

        struct DataTransformMigration2;
        impl Migration for DataTransformMigration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &Transaction) -> Result<(), Error> {
                // Transform data from "key:value" to JSON
                let mut stmt = tx.prepare("SELECT name, data FROM prefs")?;
                let rows = stmt.query_map([], |row| {
                    let name: String = row.get(0)?;
                    let data: String = row.get(1)?;
                    Ok((name, data))
                })?;

                for row in rows {
                    let (name, data) = row?;
                    let parts: Vec<&str> = data.split(':').collect();
                    let json = format!("{{\"{}\":\"{}\"}}", parts[0], parts[1]);
                    tx.execute("UPDATE prefs SET data = ?1 WHERE name = ?2", [json, name])?;
                }

                Ok(())
            }
            fn down(&self, _tx: &Transaction) -> Result<(), Error> {
                // Down not needed for this test
                Ok(())
            }
        }

        let mut harness = MigrationTestHarness::new(SqliteMigrator::new(vec![
            Box::new(DataTransformMigration1),
            Box::new(DataTransformMigration2),
        ]));

        harness.migrate_to(1).unwrap();
        harness
            .execute("INSERT INTO prefs VALUES ('alice', 'theme:dark')")
            .unwrap();

        harness.migrate_up_one().unwrap();

        let data: String = harness
            .query_one("SELECT data FROM prefs WHERE name = 'alice'")
            .unwrap();
        assert_eq!(data, r#"{"theme":"dark"}"#);
    }
}
