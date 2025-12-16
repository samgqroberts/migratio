//! Testing utilities for PostgreSQL migration development.
//!
//! This module provides a test harness for PostgreSQL migration testing: [PostgresTestHarness]

use crate::{postgres::PostgresMigrator, Error};
use postgres::types::FromSql;
use postgres::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A test harness for PostgreSQL migration testing that provides state control and assertion helpers.
///
/// # Example
///
/// ```ignore
/// use migratio::testing::postgres::PostgresTestHarness;
/// use migratio::{Migration, Error};
/// use migratio::postgres::PostgresMigrator;
///
/// struct Migration1;
/// impl Migration for Migration1 {
///     fn version(&self) -> u32 { 1 }
///     fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
///         tx.execute("CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT)", &[])?;
///         Ok(())
///     }
///     fn postgres_down(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
///         tx.execute("DROP TABLE users", &[])?;
///         Ok(())
///     }
/// }
///
/// #[tokio::test]
/// async fn test() -> Result<(), Error> {
///     let client = get_test_client(); // however you want to connect to a postgres database in your tests
///     let mut harness = PostgresTestHarness::new(client, PostgresMigrator::new(vec![Box::new(Migration1)]));
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
pub struct PostgresTestHarness {
    client: Client,
    migrator: PostgresMigrator,
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

impl PostgresTestHarness {
    /// Create a new test harness with the given PostgreSQL client and migrator.
    ///
    /// Uses the provided PostgreSQL client, which can be obtained any way you like.
    pub fn new(client: Client, migrator: PostgresMigrator) -> Self {
        Self { client, migrator }
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
                return Err(Error::Generic(format!(
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
            self.migrator.upgrade_to(&mut self.client, target_version)?;
        } else if target_version < current {
            // Migrate down
            self.migrator.downgrade(&mut self.client, target_version)?;
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
            return Err(Error::Generic(
                "Already at version 0, cannot migrate down".to_string(),
            ));
        }

        self.migrator.downgrade(&mut self.client, current - 1)?;
        Ok(())
    }

    /// Get the current migration version.
    pub fn current_version(&mut self) -> Result<u32, Error> {
        self.migrator.get_current_version(&mut self.client)
    }

    /// Execute a SQL statement (for setting up test data).
    pub fn execute(&mut self, sql: &str) -> Result<(), Error> {
        self.client.execute(sql, &[])?;
        Ok(())
    }

    /// Query a single value from the database.
    ///
    /// Note: The type `T` must be an owned type (e.g., `String` not `&str`).
    pub fn query_one<T>(&mut self, sql: &str) -> Result<T, Error>
    where
        T: for<'a> FromSql<'a>,
    {
        let row = self.client.query_one(sql, &[])?;
        Ok(row.get(0))
    }

    /// Query all values from a single-column result.
    ///
    /// Note: The type `T` must be an owned type (e.g., `String` not `&str`).
    pub fn query_all<T>(&mut self, sql: &str) -> Result<Vec<T>, Error>
    where
        T: for<'a> FromSql<'a>,
    {
        let rows = self.client.query(sql, &[])?;
        Ok(rows.iter().map(|row| row.get(0)).collect())
    }

    /// Query with a custom row mapper.
    pub fn query_map<T, F>(&mut self, sql: &str, mut f: F) -> Result<Vec<T>, Error>
    where
        F: FnMut(postgres::Row) -> Result<T, Error>,
    {
        let rows = self.client.query(sql, &[])?;
        rows.into_iter().map(|row| f(row)).collect()
    }

    /// Assert that a table exists in the database.
    pub fn assert_table_exists(&mut self, table_name: &str) -> Result<(), Error> {
        let exists: bool = self
            .client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = $1)",
                &[&table_name],
            )?
            .get(0);

        if !exists {
            return Err(Error::Generic(format!(
                "Table '{}' does not exist",
                table_name
            )));
        }

        Ok(())
    }

    /// Assert that a table does not exist in the database.
    pub fn assert_table_not_exists(&mut self, table_name: &str) -> Result<(), Error> {
        let exists: bool = self
            .client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = $1)",
                &[&table_name],
            )?
            .get(0);

        if exists {
            return Err(Error::Generic(format!(
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
            return Err(Error::Generic(format!(
                "Column '{}' does not exist in table '{}'",
                column_name, table_name
            )));
        }

        Ok(())
    }

    /// Assert that an index exists.
    pub fn assert_index_exists(&mut self, index_name: &str) -> Result<(), Error> {
        let exists: bool = self
            .client
            .query_one(
                "SELECT EXISTS (SELECT FROM pg_indexes WHERE schemaname = 'public' AND indexname = $1)",
                &[&index_name],
            )?
            .get(0);

        if !exists {
            return Err(Error::Generic(format!(
                "Index '{}' does not exist",
                index_name
            )));
        }

        Ok(())
    }

    /// Capture the current PostgreSQL database schema as a snapshot.
    pub fn capture_schema(&mut self) -> Result<SchemaSnapshot, Error> {
        let mut tables = HashMap::new();

        // Get all user tables (exclude system tables and migration table)
        let table_rows = self.client.query(
            "SELECT table_name FROM information_schema.tables
             WHERE table_schema = 'public'
             AND table_name != '_migratio_version_'
             ORDER BY table_name",
            &[],
        )?;

        for row in table_rows {
            let table_name: String = row.get(0);
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

            return Err(Error::Generic(format!(
                "Schema mismatch detected:\n{}",
                differences.join("\n")
            )));
        }

        Ok(())
    }

    /// Get column information for a table.
    fn get_columns(&mut self, table_name: &str) -> Result<Vec<ColumnInfo>, Error> {
        // Get primary key columns using a subquery to convert table name to oid
        let pk_rows = self.client.query(
            "SELECT a.attname
             FROM pg_index i
             JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
             JOIN pg_class c ON c.oid = i.indrelid
             WHERE c.relname = $1 AND c.relnamespace = 'public'::regnamespace AND i.indisprimary",
            &[&table_name],
        )?;
        let pk_columns: Vec<String> = pk_rows.iter().map(|row| row.get(0)).collect();

        let rows = self.client.query(
            "SELECT
                column_name,
                data_type,
                is_nullable,
                column_default
             FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = $1
             ORDER BY ordinal_position",
            &[&table_name],
        )?;

        let columns = rows
            .into_iter()
            .map(|row| {
                let name: String = row.get(0);
                let type_name: String = row.get(1);
                let is_nullable: String = row.get(2);
                let default_value: Option<String> = row.get(3);
                let primary_key = pk_columns.contains(&name);

                ColumnInfo {
                    name,
                    type_name,
                    not_null: is_nullable == "NO",
                    default_value,
                    primary_key,
                }
            })
            .collect();

        Ok(columns)
    }

    /// Get index information for a table.
    fn get_indexes(&mut self, table_name: &str) -> Result<Vec<IndexInfo>, Error> {
        // Get all indexes for this table, excluding primary key indexes
        let rows = self.client.query(
            "SELECT
                i.relname AS index_name,
                ix.indisunique AS is_unique,
                array_agg(a.attname ORDER BY array_position(ix.indkey, a.attnum)) AS columns
             FROM pg_class t
             JOIN pg_index ix ON t.oid = ix.indrelid
             JOIN pg_class i ON i.oid = ix.indexrelid
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY(ix.indkey)
             WHERE t.relkind = 'r'
               AND t.relname = $1
               AND NOT ix.indisprimary
             GROUP BY i.relname, ix.indisunique
             ORDER BY i.relname",
            &[&table_name],
        )?;

        let indexes = rows
            .into_iter()
            .map(|row| {
                let name: String = row.get(0);
                let unique: bool = row.get(1);
                let columns: Vec<String> = row.get(2);

                IndexInfo {
                    name,
                    unique,
                    columns,
                }
            })
            .collect();

        Ok(indexes)
    }

    /// Get a mutable reference to the underlying client for advanced usage.
    pub fn client(&mut self) -> &mut Client {
        &mut self.client
    }
}

#[cfg(test)]
mod tests {
    use crate::test_postgres::get_test_client;
    use crate::Migration;

    use super::*;

    struct TestMigration1;
    impl Migration for TestMigration1 {
        fn version(&self) -> u32 {
            1
        }
        fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
            tx.execute(
                "CREATE TABLE users (id SERIAL PRIMARY KEY, name VARCHAR(255))",
                &[],
            )?;
            Ok(())
        }
        fn postgres_down(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
            tx.execute("DROP TABLE users", &[])?;
            Ok(())
        }
        fn name(&self) -> String {
            "create_users_table".to_string()
        }
        #[cfg(feature = "sqlite")]
        fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
            Ok(())
        }
        #[cfg(feature = "mysql")]
        fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
            Ok(())
        }
    }

    struct TestMigration2;
    impl Migration for TestMigration2 {
        fn version(&self) -> u32 {
            2
        }
        fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
            tx.execute("ALTER TABLE users ADD COLUMN email VARCHAR(255)", &[])?;
            Ok(())
        }
        fn postgres_down(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
            tx.execute("ALTER TABLE users DROP COLUMN email", &[])?;
            Ok(())
        }
        fn name(&self) -> String {
            "add_email_column".to_string()
        }
        #[cfg(feature = "sqlite")]
        fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
            Ok(())
        }
        #[cfg(feature = "mysql")]
        fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
            Ok(())
        }
    }

    struct TestMigration3;
    impl Migration for TestMigration3 {
        fn version(&self) -> u32 {
            3
        }
        fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
            tx.execute("CREATE INDEX idx_users_email ON users(email)", &[])?;
            Ok(())
        }
        fn postgres_down(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
            tx.execute("DROP INDEX idx_users_email", &[])?;
            Ok(())
        }
        fn name(&self) -> String {
            "add_email_index".to_string()
        }
        #[cfg(feature = "sqlite")]
        fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
            Ok(())
        }
        #[cfg(feature = "mysql")]
        fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
            Ok(())
        }
    }

    #[test]
    fn test_migrate_to() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![
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

    #[test]
    fn test_migrate_to_nonexistent_version() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        let result = harness.migrate_to(5);
        assert!(result.is_err());

        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("Migration version 5 does not exist"));
        assert!(err_msg.contains("Available versions: 1, 2"));
    }

    #[test]
    fn test_migrate_up_one() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        assert_eq!(harness.current_version().unwrap(), 0);

        harness.migrate_up_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 1);

        harness.migrate_up_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 2);
    }

    #[test]
    fn test_migrate_down_one() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        harness.migrate_to(2).unwrap();
        assert_eq!(harness.current_version().unwrap(), 2);

        harness.migrate_down_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 1);

        harness.migrate_down_one().unwrap();
        assert_eq!(harness.current_version().unwrap(), 0);
    }

    #[test]
    fn test_execute_and_query() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1)]),
        );

        harness.migrate_to(1).unwrap();
        harness
            .execute("INSERT INTO users (name) VALUES ('alice')")
            .unwrap();

        let name: String = harness
            .query_one("SELECT name FROM users WHERE id = 1")
            .unwrap();
        assert_eq!(name, "alice");
    }

    #[test]
    fn test_query_all() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1)]),
        );

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

    #[test]
    fn test_assert_table_exists() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1)]),
        );

        harness.migrate_to(1).unwrap();
        harness.assert_table_exists("users").unwrap();

        let result = harness.assert_table_exists("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_assert_table_not_exists() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1)]),
        );

        harness.assert_table_not_exists("users").unwrap();

        harness.migrate_to(1).unwrap();
        let result = harness.assert_table_not_exists("users");
        assert!(result.is_err());
    }

    #[test]
    fn test_assert_column_exists() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
        );

        harness.migrate_to(1).unwrap();
        harness.assert_column_exists("users", "name").unwrap();

        let result = harness.assert_column_exists("users", "email");
        assert!(result.is_err());

        harness.migrate_to(2).unwrap();
        harness.assert_column_exists("users", "email").unwrap();
    }

    #[test]
    fn test_assert_index_exists() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![
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

    #[test]
    fn test_capture_schema() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
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

    #[test]
    fn test_schema_reversibility() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1), Box::new(TestMigration2)]),
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

    #[test]
    fn test_assert_schema_matches() {
        let client = get_test_client();
        let mut harness = PostgresTestHarness::new(
            client,
            PostgresMigrator::new(vec![Box::new(TestMigration1)]),
        );

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
}
