//! Database migrations for the sample application.
//!
//! This module demonstrates how to define migrations and expose a
//! `get_migrator()` function for use with `cargo migratio`.

use migratio::sqlite::SqliteMigrator;
use migratio::{Error, Migration, Precondition};
use rusqlite::Transaction;

/// Migration 1: Create the users table.
pub struct CreateUsersTable;

impl Migration for CreateUsersTable {
    fn version(&self) -> u32 {
        1
    }

    fn name(&self) -> String {
        "Create users table".to_string()
    }

    fn description(&self) -> Option<&'static str> {
        Some("Creates the initial users table with id and name columns")
    }

    fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
        tx.execute(
            "CREATE TABLE users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL
            )",
            [],
        )?;
        Ok(())
    }

    fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
        tx.execute("DROP TABLE users", [])?;
        Ok(())
    }

    fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
        let mut stmt =
            tx.prepare("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'")?;
        let count: i64 = stmt.query_row([], |row| row.get(0))?;

        if count > 0 {
            Ok(Precondition::AlreadySatisfied)
        } else {
            Ok(Precondition::NeedsApply)
        }
    }
}

/// Migration 2: Add email column to users table.
pub struct AddEmailColumn;

impl Migration for AddEmailColumn {
    fn version(&self) -> u32 {
        2
    }

    fn name(&self) -> String {
        "Add email column".to_string()
    }

    fn description(&self) -> Option<&'static str> {
        Some("Adds an email column to the users table")
    }

    fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
        tx.execute("ALTER TABLE users ADD COLUMN email TEXT", [])?;
        Ok(())
    }

    fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
        // SQLite doesn't support DROP COLUMN directly in older versions,
        // so we need to recreate the table
        tx.execute(
            "CREATE TABLE users_new (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL
            )",
            [],
        )?;
        tx.execute(
            "INSERT INTO users_new (id, name) SELECT id, name FROM users",
            [],
        )?;
        tx.execute("DROP TABLE users", [])?;
        tx.execute("ALTER TABLE users_new RENAME TO users", [])?;
        Ok(())
    }
}

/// Migration 3: Create posts table with foreign key to users.
pub struct CreatePostsTable;

impl Migration for CreatePostsTable {
    fn version(&self) -> u32 {
        3
    }

    fn name(&self) -> String {
        "Create posts table".to_string()
    }

    fn description(&self) -> Option<&'static str> {
        Some("Creates a posts table with a foreign key reference to users")
    }

    fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
        tx.execute(
            "CREATE TABLE posts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (user_id) REFERENCES users(id)
            )",
            [],
        )?;
        Ok(())
    }

    fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
        tx.execute("DROP TABLE posts", [])?;
        Ok(())
    }
}

/// Returns the configured SQLite migrator with all migrations.
///
/// This function is referenced in `Cargo.toml` under `[package.metadata.migratio]`
/// and is called by `cargo migratio` to get the migrator instance.
pub fn get_migrator() -> SqliteMigrator {
    SqliteMigrator::new(vec![
        Box::new(CreateUsersTable),
        Box::new(AddEmailColumn),
        Box::new(CreatePostsTable),
    ])
}
