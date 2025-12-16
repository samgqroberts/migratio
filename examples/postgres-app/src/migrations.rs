//! Database migrations for the PostgreSQL example application.

use migratio::postgres::{PostgresMigrator, PostgresTransaction as Transaction};
use migratio::{Error, Migration};

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

    fn postgres_up(&self, tx: &mut Transaction) -> Result<(), Error> {
        tx.execute(
            "CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            )",
            &[],
        )?;
        Ok(())
    }

    fn postgres_down(&self, tx: &mut Transaction) -> Result<(), Error> {
        tx.execute("DROP TABLE users", &[])?;
        Ok(())
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

    fn postgres_up(&self, tx: &mut Transaction) -> Result<(), Error> {
        tx.execute("ALTER TABLE users ADD COLUMN email VARCHAR(255)", &[])?;
        Ok(())
    }

    fn postgres_down(&self, tx: &mut Transaction) -> Result<(), Error> {
        tx.execute("ALTER TABLE users DROP COLUMN email", &[])?;
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

    fn postgres_up(&self, tx: &mut Transaction) -> Result<(), Error> {
        tx.execute(
            "CREATE TABLE posts (
                id SERIAL PRIMARY KEY,
                user_id INT NOT NULL REFERENCES users(id),
                title VARCHAR(255) NOT NULL,
                body TEXT NOT NULL,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            )",
            &[],
        )?;
        Ok(())
    }

    fn postgres_down(&self, tx: &mut Transaction) -> Result<(), Error> {
        tx.execute("DROP TABLE posts", &[])?;
        Ok(())
    }
}

/// Returns the configured PostgreSQL migrator with all migrations.
pub fn get_migrator() -> PostgresMigrator {
    PostgresMigrator::new(vec![
        Box::new(CreateUsersTable),
        Box::new(AddEmailColumn),
        Box::new(CreatePostsTable),
    ])
}
