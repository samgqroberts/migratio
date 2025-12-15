//! Database migrations for the MySQL example application.

use migratio::mysql::{MysqlConn as Conn, MysqlMigrator, MysqlQueryable as Queryable};
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

    fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
        conn.query_drop(
            "CREATE TABLE users (
                id INT PRIMARY KEY AUTO_INCREMENT,
                name VARCHAR(255) NOT NULL
            )",
        )?;
        Ok(())
    }

    fn mysql_down(&self, conn: &mut Conn) -> Result<(), Error> {
        conn.query_drop("DROP TABLE users")?;
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

    fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
        conn.query_drop("ALTER TABLE users ADD COLUMN email VARCHAR(255)")?;
        Ok(())
    }

    fn mysql_down(&self, conn: &mut Conn) -> Result<(), Error> {
        conn.query_drop("ALTER TABLE users DROP COLUMN email")?;
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

    fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
        conn.query_drop(
            "CREATE TABLE posts (
                id INT PRIMARY KEY AUTO_INCREMENT,
                user_id INT NOT NULL,
                title VARCHAR(255) NOT NULL,
                body TEXT NOT NULL,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (user_id) REFERENCES users(id)
            )",
        )?;
        Ok(())
    }

    fn mysql_down(&self, conn: &mut Conn) -> Result<(), Error> {
        conn.query_drop("DROP TABLE posts")?;
        Ok(())
    }
}

/// Returns the configured MySQL migrator with all migrations.
pub fn get_migrator() -> MysqlMigrator {
    MysqlMigrator::new(vec![
        Box::new(CreateUsersTable),
        Box::new(AddEmailColumn),
        Box::new(CreatePostsTable),
    ])
}
