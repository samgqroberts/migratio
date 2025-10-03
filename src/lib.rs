//! # Migratio
//!
//! `migratio` is a lightweight library for managing database migrations (currently for Sqlite).
//!
//! ## Example
//!
//! ```
//! use migratio::{Migration, SqliteMigrator, MigrationReport, Error};
//! use rusqlite::Connection;
//!
//! // define your migrations as structs that implement the Migration trait
//! struct Migration1;
//!
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn up(&self, conn: &mut Connection) -> Result<(), Error> {
//!         conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
//!         Ok(())
//!     }
//! }
//!
//! struct Migration2;
//!
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 {
//!         2
//!     }
//!     fn up(&self, conn: &mut Connection) -> Result<(), Error> {
//!         conn.execute("ALTER TABLE users ADD COLUMN email TEXT", [])?;
//!         Ok(())
//!     }
//! }
//!
//! // construct a migrator with migrations
//! let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
//!
//! // connect to your database and run the migrations, receiving a report of the results.
//! let mut conn = Connection::open_in_memory().unwrap();
//! let report = migrator.upgrade(&mut conn).unwrap();
//! assert_eq!(
//!     report,
//!     MigrationReport {
//!         schema_version_table_existed: false,
//!         schema_version_table_created: true,
//!         migrations_run: vec![1, 2],
//!         failing_migration: None
//!     }
//! );
//!
//! // assert the migration logic was applied to the database
//! let mut stmt = conn.prepare("PRAGMA table_info(users)").unwrap();
//! let columns = stmt
//!     .query_map([], |row| Ok(row.get::<_, String>(1).unwrap()))
//!     .unwrap()
//!     .collect::<Result<Vec<_>, _>>()
//!     .unwrap();
//! assert_eq!(columns, vec!["id", "name", "email"]);
//! ```

mod error;
mod migrator;

pub use error::Error;
pub use migrator::{Migration, MigrationReport, SqliteMigrator, SCHEMA_VERSION_TABLE_NAME};
