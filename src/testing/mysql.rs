//! MySQL testing utilities.
//!
//! This module provides a test harness for MySQL migration testing.

use crate::{mysql::MysqlMigrator, Error};
use mysql::prelude::*;
use mysql::Conn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A test harness for MySQL migration testing that provides state control and assertion helpers.
///
/// # Example
///
/// ```ignore
/// use migratio::testing::mysql::MysqlTestHarness;
/// use migratio::{Migration, Error};
/// use migratio::mysql::MysqlMigrator;
///
/// struct Migration1;
/// impl Migration for Migration1 {
///     fn version(&self) -> u32 { 1 }
///     fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
///         conn.query_drop("CREATE TABLE users (id INT PRIMARY KEY AUTO_INCREMENT, name VARCHAR(255))")?;
///         Ok(())
///     }
///     fn mysql_down(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
///         conn.query_drop("DROP TABLE users")?;
///         Ok(())
///     }
/// }
///
/// #[tokio::test]
/// async fn test() -> Result<(), Error> {
///     let conn = get_test_conn(); // however you want to connect to a mysql database in your tests
///     let mut harness = MysqlTestHarness::new(conn, MysqlMigrator::new(vec![Box::new(Migration1)]));
///
///     // Migrate to version 1
///     harness.migrate_to(1)?;
///
///     // Insert test data
///     harness.execute("INSERT INTO users (name) VALUES ('alice')")?;
///
///     // Assert table exists
///     harness.assert_table_exists("users")?;
///
///     // Query data
///     let name: String = harness.query_one("SELECT name FROM users WHERE id = 1")?;
///     assert_eq!(name, "alice");
///     Ok(())
/// }
/// ```
pub struct MysqlTestHarness {
    conn: Conn,
    migrator: MysqlMigrator,
}

/// Represents a captured database schema for comparison and snapshotting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaSnapshot {
    /// Map of table name to table definitions
    pub tables: HashMap<String, TableSchema>,
}

/// Represents a table's schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableSchema {
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
    pub columns: Vec<String>,
}

impl MysqlTestHarness {
    /// Create a new test harness with the given MySQL connection and migrator.
    ///
    /// Uses the provided MySQL connection, which can be obtained any way you like.
    pub fn new(conn: Conn, migrator: MysqlMigrator) -> Self {
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
                return Err(Error::Mysql(format!(
                    "Migration version {} does not exist. Available versions: {}",
                    target_version,
                    self.migrator
                        .migrations()
                        .iter()
                        .map(|m| m.version().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
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
            return Err(Error::Mysql(
                "Already at version 0, cannot migrate down".to_string(),
            ));
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
        self.conn.query_drop(sql)?;
        Ok(())
    }

    /// Query a single value from the database.
    pub fn query_one<T>(&mut self, sql: &str) -> Result<T, Error>
    where
        T: FromValue,
    {
        let result: Option<T> = self.conn.query_first(sql)?;
        result.ok_or_else(|| Error::Mysql("Query returned no results".to_string()))
    }

    /// Query all values from a single-column result.
    pub fn query_all<T>(&mut self, sql: &str) -> Result<Vec<T>, Error>
    where
        T: FromValue,
    {
        let results: Vec<T> = self.conn.query(sql)?;
        Ok(results)
    }

    /// Query with a custom row mapper.
    pub fn query_map<T, F>(&mut self, sql: &str, mut f: F) -> Result<Vec<T>, Error>
    where
        F: FnMut(mysql::Row) -> Result<T, Error>,
    {
        let rows: Vec<mysql::Row> = self.conn.query(sql)?;
        rows.into_iter().map(|row| f(row)).collect()
    }

    /// Assert that a table exists in the database.
    pub fn assert_table_exists(&mut self, table_name: &str) -> Result<(), Error> {
        let count: Option<i64> = self.conn.query_first(
            format!(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = '{}'",
                table_name
            )
        )?;

        if count.unwrap_or(0) == 0 {
            return Err(Error::Mysql(format!(
                "Table '{}' does not exist",
                table_name
            )));
        }

        Ok(())
    }

    /// Assert that a table does not exist in the database.
    pub fn assert_table_not_exists(&mut self, table_name: &str) -> Result<(), Error> {
        let count: Option<i64> = self.conn.query_first(
            format!(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = '{}'",
                table_name
            )
        )?;

        if count.unwrap_or(0) > 0 {
            return Err(Error::Mysql(format!(
                "Table '{}' exists but should not",
                table_name
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
            return Err(Error::Mysql(format!(
                "Column '{}' does not exist in table '{}'",
                column_name, table_name
            )));
        }

        Ok(())
    }

    /// Assert that an index exists.
    pub fn assert_index_exists(&mut self, index_name: &str) -> Result<(), Error> {
        let count: Option<i64> = self.conn.query_first(
            format!(
                "SELECT COUNT(*) FROM information_schema.statistics WHERE table_schema = DATABASE() AND index_name = '{}'",
                index_name
            )
        )?;

        if count.unwrap_or(0) == 0 {
            return Err(Error::Mysql(format!(
                "Index '{}' does not exist",
                index_name
            )));
        }

        Ok(())
    }

    /// Capture the current MySQL database schema as a snapshot.
    pub fn capture_schema(&mut self) -> Result<SchemaSnapshot, Error> {
        let mut tables = HashMap::new();

        // Get all user tables (exclude system tables and migration table)
        let table_names: Vec<String> = self.conn.query(
            "SELECT table_name FROM information_schema.tables
             WHERE table_schema = DATABASE()
             AND table_name != '_migratio_version_'
             ORDER BY table_name",
        )?;

        for table_name in table_names {
            let columns = self.get_columns(&table_name)?;
            let indexes = self.get_indexes(&table_name)?;

            tables.insert(table_name, TableSchema { columns, indexes });
        }

        Ok(SchemaSnapshot { tables })
    }

    /// Assert that the current schema matches a previously captured snapshot.
    pub fn assert_schema_matches(&mut self, expected: &SchemaSnapshot) -> Result<(), Error> {
        let actual = self.capture_schema()?;

        if actual != *expected {
            let mut differences = Vec::new();

            // Sort table names for deterministic ordering
            let mut expected_table_names: Vec<_> = expected.tables.keys().collect();
            expected_table_names.sort();
            let mut actual_table_names: Vec<_> = actual.tables.keys().collect();
            actual_table_names.sort();

            // Check for tables in expected but not in actual
            for table_name in &expected_table_names {
                if !actual.tables.contains_key(*table_name) {
                    differences.push(format!("  - Table '{}' is missing", table_name));
                }
            }

            // Check for tables in actual but not in expected
            for table_name in &actual_table_names {
                if !expected.tables.contains_key(*table_name) {
                    differences.push(format!("  - Unexpected table '{}' found", table_name));
                }
            }

            // Check for differences in common tables (sorted order)
            for table_name in &expected_table_names {
                let expected_table = &expected.tables[*table_name];
                if let Some(actual_table) = actual.tables.get(*table_name) {
                    // Check column differences
                    if expected_table.columns != actual_table.columns {
                        let expected_cols: Vec<_> =
                            expected_table.columns.iter().map(|c| &c.name).collect();
                        let actual_cols: Vec<_> =
                            actual_table.columns.iter().map(|c| &c.name).collect();

                        if expected_cols != actual_cols {
                            differences.push(format!(
                                "  - Table '{}' column mismatch:\n    Expected columns: {:?}\n    Actual columns:   {:?}",
                                table_name, expected_cols, actual_cols
                            ));
                        } else {
                            // Same column names but different properties
                            for (expected_col, actual_col) in
                                expected_table.columns.iter().zip(&actual_table.columns)
                            {
                                if expected_col != actual_col {
                                    differences.push(format!(
                                        "  - Table '{}' column '{}' properties differ:\n    Expected: {:?}\n    Actual:   {:?}",
                                        table_name, expected_col.name, expected_col, actual_col
                                    ));
                                }
                            }
                        }
                    }

                    // Check index differences
                    if expected_table.indexes != actual_table.indexes {
                        let expected_idxs: Vec<_> =
                            expected_table.indexes.iter().map(|i| &i.name).collect();
                        let actual_idxs: Vec<_> =
                            actual_table.indexes.iter().map(|i| &i.name).collect();
                        differences.push(format!(
                            "  - Table '{}' index mismatch:\n    Expected indexes: {:?}\n    Actual indexes:   {:?}",
                            table_name, expected_idxs, actual_idxs
                        ));
                    }
                }
            }

            return Err(Error::Mysql(format!(
                "Schema mismatch detected:\n{}",
                differences.join("\n")
            )));
        }

        Ok(())
    }

    /// Get column information for a table.
    fn get_columns(&mut self, table_name: &str) -> Result<Vec<ColumnInfo>, Error> {
        let rows: Vec<(String, String, String, Option<String>, String)> =
            self.conn.query(format!(
                "SELECT
                column_name,
                column_type,
                is_nullable,
                column_default,
                column_key
             FROM information_schema.columns
             WHERE table_schema = DATABASE() AND table_name = '{}'
             ORDER BY ordinal_position",
                table_name
            ))?;

        let columns = rows
            .into_iter()
            .map(
                |(name, type_name, is_nullable, default_value, column_key)| ColumnInfo {
                    name,
                    type_name,
                    not_null: is_nullable == "NO",
                    default_value,
                    primary_key: column_key == "PRI",
                },
            )
            .collect();

        Ok(columns)
    }

    /// Get index information for a table.
    fn get_indexes(&mut self, table_name: &str) -> Result<Vec<IndexInfo>, Error> {
        // Get all indexes for this table, grouped by index name
        let rows: Vec<(String, i64, String)> = self.conn.query(format!(
            "SELECT
                index_name,
                non_unique,
                column_name
             FROM information_schema.statistics
             WHERE table_schema = DATABASE() AND table_name = '{}'
             AND index_name != 'PRIMARY'
             ORDER BY index_name, seq_in_index",
            table_name
        ))?;

        // Group by index name
        let mut index_map: HashMap<String, (bool, Vec<String>)> = HashMap::new();

        for (index_name, non_unique, column_name) in rows {
            index_map
                .entry(index_name)
                .or_insert((non_unique == 0, Vec::new()))
                .1
                .push(column_name);
        }

        let mut indexes: Vec<IndexInfo> = index_map
            .into_iter()
            .map(|(name, (unique, columns))| IndexInfo {
                name,
                unique,
                columns,
            })
            .collect();

        // Sort for deterministic ordering
        indexes.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(indexes)
    }

    /// Get a mutable reference to the underlying connection for advanced usage.
    pub fn connection(&mut self) -> &mut Conn {
        &mut self.conn
    }
}

#[cfg(test)]
mod tests {
    use crate::test_mysql::get_test_conn;
    use crate::Migration;

    use super::*;

    struct TestMigration1;
    impl Migration for TestMigration1 {
        fn version(&self) -> u32 {
            1
        }
        fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
            conn.query_drop(
                "CREATE TABLE users (id INT PRIMARY KEY AUTO_INCREMENT, name VARCHAR(255))",
            )?;
            Ok(())
        }
        fn mysql_down(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
            conn.query_drop("DROP TABLE users")?;
            Ok(())
        }
        fn name(&self) -> String {
            "create_users_table".to_string()
        }
        #[cfg(feature = "sqlite")]
        fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
            Ok(())
        }
    }

    struct TestMigration2;
    impl Migration for TestMigration2 {
        fn version(&self) -> u32 {
            2
        }
        fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
            conn.query_drop("ALTER TABLE users ADD COLUMN email VARCHAR(255)")?;
            Ok(())
        }
        fn mysql_down(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
            conn.query_drop("ALTER TABLE users DROP COLUMN email")?;
            Ok(())
        }
        fn name(&self) -> String {
            "add_email_column".to_string()
        }
        #[cfg(feature = "sqlite")]
        fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
            Ok(())
        }
    }

    struct TestMigration3;
    impl Migration for TestMigration3 {
        fn version(&self) -> u32 {
            3
        }
        fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
            conn.query_drop("CREATE INDEX idx_users_email ON users(email)")?;
            Ok(())
        }
        fn mysql_down(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
            conn.query_drop("DROP INDEX idx_users_email ON users")?;
            Ok(())
        }
        fn name(&self) -> String {
            "add_email_index".to_string()
        }
        #[cfg(feature = "sqlite")]
        fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_migrate_to() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![
                Box::new(TestMigration1),
                Box::new(TestMigration2),
                Box::new(TestMigration3),
            ]),
        );

        assert_eq!(harness.current_version().unwrap(), 0);

        harness.migrate_to(2).unwrap();
        assert_eq!(harness.current_version().unwrap(), 2);

        harness.migrate_to(1).unwrap();
        assert_eq!(harness.current_version().unwrap(), 1);

        harness.migrate_to(3).unwrap();
        assert_eq!(harness.current_version().unwrap(), 3);
    }

    #[tokio::test]
    async fn test_migrate_to_nonexistent_version() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        let result = harness.migrate_to(5);
        assert!(result.is_err());

        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("Migration version 5 does not exist"));
        assert!(err_msg.contains("Available versions: 1, 2"));
    }

    #[tokio::test]
    async fn test_migrate_up_one() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        assert_eq!(harness.current_version().unwrap(), 0);

        harness.migrate_up_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 1);

        harness.migrate_up_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 2);
    }

    #[tokio::test]
    async fn test_migrate_down_one() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        harness.migrate_to(2).unwrap();
        assert_eq!(harness.current_version().unwrap(), 2);

        harness.migrate_down_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 1);

        harness.migrate_down_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_execute_and_query() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness =
            MysqlTestHarness::new(conn, MysqlMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        harness
            .execute("INSERT INTO users (name) VALUES ('alice')")
            .unwrap();

        let name: String = harness
            .query_one("SELECT name FROM users WHERE id = 1")
            .unwrap();
        assert_eq!(name, "alice");
    }

    #[tokio::test]
    async fn test_query_all() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness =
            MysqlTestHarness::new(conn, MysqlMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        harness
            .execute("INSERT INTO users (name) VALUES ('alice')")
            .unwrap();
        harness
            .execute("INSERT INTO users (name) VALUES ('bob')")
            .unwrap();

        let names: Vec<String> = harness
            .query_all("SELECT name FROM users ORDER BY id")
            .unwrap();
        assert_eq!(names, vec!["alice", "bob"]);
    }

    #[tokio::test]
    async fn test_assert_table_exists() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness =
            MysqlTestHarness::new(conn, MysqlMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        harness.assert_table_exists("users").unwrap();

        let result = harness.assert_table_exists("nonexistent");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_assert_table_not_exists() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness =
            MysqlTestHarness::new(conn, MysqlMigrator::new(vec![Box::new(TestMigration1)]));

        harness.assert_table_not_exists("users").unwrap();

        harness.migrate_to(1).unwrap();
        let result = harness.assert_table_not_exists("users");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_assert_column_exists() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        harness.migrate_to(1).unwrap();
        harness.assert_column_exists("users", "name").unwrap();

        let result = harness.assert_column_exists("users", "email");
        assert!(result.is_err());

        harness.migrate_to(2).unwrap();
        harness.assert_column_exists("users", "email").unwrap();
    }

    #[tokio::test]
    async fn test_assert_index_exists() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![
                Box::new(TestMigration1),
                Box::new(TestMigration2),
                Box::new(TestMigration3),
            ]),
        );

        harness.migrate_to(2).unwrap();
        let result = harness.assert_index_exists("idx_users_email");
        assert!(result.is_err());

        harness.migrate_to(3).unwrap();
        harness.assert_index_exists("idx_users_email").unwrap();
    }

    #[tokio::test]
    async fn test_capture_schema() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        harness.migrate_to(2).unwrap();
        let snapshot = harness.capture_schema().unwrap();

        assert!(snapshot.tables.contains_key("users"));
        let users_table = &snapshot.tables["users"];
        assert_eq!(users_table.columns.len(), 3); // id, name, email
        assert!(users_table.columns.iter().any(|c| c.name == "id"));
        assert!(users_table.columns.iter().any(|c| c.name == "name"));
        assert!(users_table.columns.iter().any(|c| c.name == "email"));
    }

    #[tokio::test]
    async fn test_schema_reversibility() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

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

    #[tokio::test]
    async fn test_assert_schema_matches() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness =
            MysqlTestHarness::new(conn, MysqlMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        let snapshot = harness.capture_schema().unwrap();

        // Should match itself
        harness.assert_schema_matches(&snapshot).unwrap();

        // Add a column - should no longer match
        harness
            .execute("ALTER TABLE users ADD COLUMN age INT")
            .unwrap();
        let result = harness.assert_schema_matches(&snapshot);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_assert_schema_matches_error_missing_table() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness =
            MysqlTestHarness::new(conn, MysqlMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        let snapshot = harness.capture_schema().unwrap();

        // Drop the table
        harness.execute("DROP TABLE users").unwrap();

        let result = harness.assert_schema_matches(&snapshot);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = match err {
            Error::Mysql(msg) => msg,
            _ => panic!("Expected Mysql error"),
        };
        assert_eq!(
            err_msg,
            r#"Schema mismatch detected:
  - Table 'users' is missing"#
        );
    }

    #[tokio::test]
    async fn test_assert_schema_matches_error_unexpected_table() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness =
            MysqlTestHarness::new(conn, MysqlMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        let snapshot = harness.capture_schema().unwrap();

        // Add an unexpected table
        harness
            .execute("CREATE TABLE posts (id INT PRIMARY KEY AUTO_INCREMENT)")
            .unwrap();

        let result = harness.assert_schema_matches(&snapshot);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = match err {
            Error::Mysql(msg) => msg,
            _ => panic!("Expected Mysql error"),
        };
        assert_eq!(
            err_msg,
            r#"Schema mismatch detected:
  - Unexpected table 'posts' found"#
        );
    }

    #[tokio::test]
    async fn test_assert_schema_matches_error_column_added() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness =
            MysqlTestHarness::new(conn, MysqlMigrator::new(vec![Box::new(TestMigration1)]));

        harness.migrate_to(1).unwrap();
        let snapshot = harness.capture_schema().unwrap();

        // Add a column
        harness
            .execute("ALTER TABLE users ADD COLUMN age INT")
            .unwrap();

        let result = harness.assert_schema_matches(&snapshot);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = match err {
            Error::Mysql(msg) => msg,
            _ => panic!("Expected Mysql error"),
        };
        assert_eq!(
            err_msg,
            r#"Schema mismatch detected:
  - Table 'users' column mismatch:
    Expected columns: ["id", "name"]
    Actual columns:   ["id", "name", "age"]"#
        );
    }

    #[tokio::test]
    async fn test_assert_schema_matches_error_index_mismatch() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        harness.migrate_to(2).unwrap();
        let snapshot = harness.capture_schema().unwrap();

        // Add an index
        harness
            .execute("CREATE INDEX idx_users_name ON users(name)")
            .unwrap();

        let result = harness.assert_schema_matches(&snapshot);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = match err {
            Error::Mysql(msg) => msg,
            other => panic!("Expected Mysql error, got: {:?}", other),
        };
        assert_eq!(
            err_msg,
            r#"Schema mismatch detected:
  - Table 'users' index mismatch:
    Expected indexes: []
    Actual indexes:   ["idx_users_name"]"#
        );
    }

    #[tokio::test]
    async fn test_assert_schema_matches_error_multiple_differences() {
        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        harness.migrate_to(2).unwrap();
        let snapshot = harness.capture_schema().unwrap();

        // Make multiple changes
        harness
            .execute("ALTER TABLE users ADD COLUMN age INT")
            .unwrap();
        harness
            .execute("CREATE TABLE posts (id INT PRIMARY KEY AUTO_INCREMENT)")
            .unwrap();

        let result = harness.assert_schema_matches(&snapshot);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = match err {
            Error::Mysql(msg) => msg,
            _ => panic!("Expected Mysql error"),
        };
        assert_eq!(
            err_msg,
            r#"Schema mismatch detected:
  - Unexpected table 'posts' found
  - Table 'users' column mismatch:
    Expected columns: ["id", "name", "email"]
    Actual columns:   ["id", "name", "email", "age"]"#
        );
    }

    #[tokio::test]
    async fn test_data_transformation() {
        struct DataTransformMigration1;
        impl Migration for DataTransformMigration1 {
            fn version(&self) -> u32 {
                1
            }
            fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
                conn.query_drop(
                    "CREATE TABLE prefs (name VARCHAR(255) PRIMARY KEY, data VARCHAR(255))",
                )?;
                Ok(())
            }
            fn mysql_down(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
                conn.query_drop("DROP TABLE prefs")?;
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct DataTransformMigration2;
        impl Migration for DataTransformMigration2 {
            fn version(&self) -> u32 {
                2
            }
            fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> {
                // Transform data from "key:value" to JSON
                let rows: Vec<(String, String)> = conn.query("SELECT name, data FROM prefs")?;

                for (name, data) in rows {
                    let parts: Vec<&str> = data.split(':').collect();
                    let json = format!("{{\"{}\":\"{}\"}}", parts[0], parts[1]);
                    conn.exec_drop("UPDATE prefs SET data = ? WHERE name = ?", (json, name))?;
                }

                Ok(())
            }
            fn mysql_down(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                // Down not needed for this test
                Ok(())
            }
            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let (_pool, conn) = get_test_conn().await;
        let mut harness = MysqlTestHarness::new(
            conn,
            MysqlMigrator::new(vec![
                Box::new(DataTransformMigration1),
                Box::new(DataTransformMigration2),
            ]),
        );

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
