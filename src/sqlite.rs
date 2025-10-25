//!
//! # Example
//!
//! ```
//! use migratio::{Migration, MigrationReport, Error};
//! use migratio::sqlite::SqliteMigrator;
//! use rusqlite::{Connection, Transaction};
//!
//! // define your migrations as structs that implement the Migration trait
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! struct Migration2;
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 {
//!         2
//!     }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("ALTER TABLE users ADD COLUMN email TEXT", [])?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
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
//!
//! # Motivation
//!
//! ## Using a Live Database Connection
//!
//! Most Rust-based migration solutions focus only on using SQL to define migration logic.
//! Even the ones that say they support writing migrations in Rust use Rust to simply construct SQL instructions, like [Refinery](https://github.com/rust-db/refinery/blob/main/examples/migrations/V3__add_brand_to_cars_table.rs).
//!
//! Taking a hint from [Alembic](https://alembic.sqlalchemy.org/en/latest/), this library provides the user with a live connection to the database with which to define their migrations.
//! With a live connection, a migration can query the data, transform it in Rust, and write it back.
//! Migrations defined as pure SQL statements can only accomplish this with the toolkit provided by SQL, which is much more limited.
//!
//! Note: [SeaORM](https://www.sea-ql.org/SeaORM/) allows this, but `migratio` aims to provide an alternative for developers that don't want to adopt a full ORM solution.
//!
//! ```
//! use migratio::{Migration, MigrationReport, Error};
//! use migratio::sqlite::SqliteMigrator;
//! use rusqlite::{Connection, Transaction};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute(
//!             "CREATE TABLE user_preferences (name TEXT PRIMARY KEY, preferences TEXT)",
//!             [],
//!         )?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! // run this first migration
//! let mut conn = Connection::open_in_memory().unwrap();
//! SqliteMigrator::new(vec![Box::new(Migration1)])
//!     .upgrade(&mut conn)
//!     .unwrap();
//!
//! // simulate actual usage of the database
//! // here we have a suboptimal database serialization format for user preferences
//! conn.execute(
//!     "INSERT INTO user_preferences VALUES ('alice', 'scheme:dark|help:off')",
//!     [],
//! )
//! .unwrap();
//! conn.execute(
//!     "INSERT INTO user_preferences VALUES ('bob', 'scheme:light|help:off')",
//!     [],
//! )
//! .unwrap();
//! conn.execute(
//!     "INSERT INTO user_preferences VALUES ('charlie', 'scheme:dark|help:on')",
//!     [],
//! )
//! .unwrap();
//!
//! // define another migration that transforms the user preferences data
//! // using arbitrary Rust logic.
//! struct Migration2;
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 {
//!         2
//!     }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         // read all user preferences
//!         let mut stmt = tx.prepare("SELECT name, preferences FROM user_preferences")?;
//!         let rows = stmt.query_map([], |row| {
//!             let name: String = row.get(0)?;
//!             let preferences: String = row.get(1)?;
//!             Ok((name, preferences))
//!         })?;
//!
//!         // transform the preferences data
//!         for row in rows {
//!             let (name, preferences) = row?;
//!             let key_value_pairs = preferences
//!                 .split("|")
//!                 .map(|x| {
//!                     let mut split = x.split(":");
//!                     let key = split.next().unwrap();
//!                     let value = split.next().unwrap();
//!                     format!("\"{}\":\"{}\"", key, value)
//!                 })
//!                 .collect::<Vec<String>>();
//!             let new_preferences = format!("{{{}}}", key_value_pairs.join(","));
//!             tx.execute(
//!                 "UPDATE user_preferences SET preferences = ? WHERE name = ?",
//!                 [new_preferences, name],
//!             )?;
//!         }
//!
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! // run new migration
//! SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)])
//!     .upgrade(&mut conn)
//!     .unwrap();
//!
//! // read all data out of connection
//! let mut stmt = conn
//!     .prepare("SELECT name, preferences FROM user_preferences")
//!     .unwrap();
//! let rows = stmt
//!     .query_map([], |row| {
//!         let id: String = row.get(0)?;
//!         let preferences: String = row.get(1)?;
//!         Ok((id, preferences))
//!     })
//!     .unwrap()
//!     .collect::<Result<Vec<(String, String)>, _>>()
//!     .unwrap();
//!
//! // assert that the data has been transformed
//! assert_eq!(
//!     rows,
//!     vec![
//!         (
//!             "alice".to_string(),
//!             "{\"scheme\":\"dark\",\"help\":\"off\"}".to_string()
//!         ),
//!         (
//!             "bob".to_string(),
//!             "{\"scheme\":\"light\",\"help\":\"off\"}".to_string()
//!         ),
//!         (
//!             "charlie".to_string(),
//!             "{\"scheme\":\"dark\",\"help\":\"on\"}".to_string()
//!         )
//!     ]
//! );
//! ```
//!
//!
//! ```
//! # use migratio::{Migration, Error};
//! # use migratio::sqlite::SqliteMigrator;
//! # use rusqlite::{Connection, Transaction};
//! // migrations.rs
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! pub fn migrator() -> SqliteMigrator {
//!     SqliteMigrator::new(vec![Box::new(Migration1)])
//! }
//!
//! // main.rs
//!
//! fn main() {
//!     let mut conn = Connection::open_in_memory().expect("Failed to open database"); // or, rather, where your database is located
//!     migrator().upgrade(&mut conn).expect("Migration failed");
//!
//!     // ... rest of app startup
//! }
//! ```
//!
//! # Adoption
//!
//! ## In new projects
//!
//! This is the easiest way to get started with `migratio`.
//! Add it to your `Cargo.toml` file ( `cargo add migratio` ), construct a [SqliteMigrator] instance with some [Migration]s, and call [SqliteMigrator::upgrade] wherever you want migrations to be considered.
//!
//! ## In projects currently executing manual database setup on app run
//!
//! In this case, your app has some consideration on app startup (or whenever you initialize a connection to the database) that looks like this, which relies on an idempotent procedure to initialize the database's schema:
//!
//! ```
//! # use rusqlite::Connection;
//! fn get_conn(db_path: &str) -> Result<Connection, String> {
//!     let conn = Connection::open(db_path)
//!         .map_err(|e| format!("Failed to open database: {}", e))?;
//!
//!     conn.execute("CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)", [])
//!         .map_err(|e| format!("Failed to create users table: {}", e))?;
//!
//!     conn.execute(
//!         "CREATE TABLE IF NOT EXISTS preferences (
//!             user_id INTEGER NOT NULL,
//!             preferences TEXT NOT NULL
//!         )",
//!         [],
//!     ).map_err(|e| format!("Failed to create preferences table: {}", e))?;
//!
//!     Ok(conn)
//! }
//! ```
//!
//! You rely on the idempotence of this setup code to be able to run it multiple times without causing any issues.
//! In this case, this logic can be the first migration in your project:
//!
//! ```
//! # use migratio::{Migration, Error};
//! # use migratio::sqlite::SqliteMigrator;
//! # use rusqlite::{Connection, Transaction};
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)", [])?;
//!
//!         tx.execute(
//!             "CREATE TABLE IF NOT EXISTS preferences (
//!                 user_id INTEGER NOT NULL,
//!                 preferences TEXT NOT NULL
//!             )",
//!             [],
//!         )?;
//!
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! // Then replace your existing setup code with:
//! fn get_conn(db_path: &str) -> Result<Connection, String> {
//!     let mut conn = Connection::open(db_path)
//!         .map_err(|e| format!("Failed to open database: {}", e))?;
//!
//!     let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
//!     migrator.upgrade(&mut conn)
//!         .map_err(|e| format!("Migration failed: {}", e))?;
//!
//!     Ok(conn)
//! }
//! ```
//!
//! This relies on the same idempotence the original code relied on.
//! From here, you can add new [Migration]s (which do not have to be idempotent) to the [SqliteMigrator] that ship in future app updates.
//! Note that this migration consideration can (and maybe should) be moved somewhere that only runs once on app startup.
//!
//! ## In projects currently using another migration tool
//!
//! Let's say you have another migration tool, with two existing migrations:
//! 1. Create users table
//! 2. Create preferences table
//!
//! You may have deployments whose databases:
//! - Have never been initialized (have applied neither of these migrations)
//! - Have applied only Migration 1
//! - Have applied Migration 1 and 2
//!
//! One approach, similar to the previous section, is to ensure the migration logic is idempotent when you translate it from the old tool to `migratio`.
//! Even if you have `CREATE TABLE` statements in the old tool, you can translate them to `CREATE TABLE IF NOT EXISTS` in the `migratio` migrations.
//! The idempotence will ensure that in each of the 3 initial states the database could be in, it will be brought to the desired state.
//!
//! Another approach is to use the [Migration::precondition] method to check if the migration should be applied again.
//!
//! ```
//! # use migratio::{Migration, Precondition, Error};
//! # use rusqlite::{Transaction};
//! struct CreateUsersTable;
//! impl Migration for CreateUsersTable {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         // logic copied / translated from the other migration tool usage
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)", [])?;
//!         Ok(())
//!     }
//!     fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
//!         let mut stmt = tx.prepare(
//!             "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
//!         )?;
//!         let count: i64 = stmt.query_row([], |row| row.get(0))?;
//!         Ok(if count == 0 { Precondition::NeedsApply } else { Precondition::AlreadySatisfied })
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! struct CreatePreferencesTable;
//! impl Migration for CreatePreferencesTable {
//!     fn version(&self) -> u32 {
//!         2
//!     }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         // logic copied / translated from the other migration tool usage
//!         tx.execute(
//!             "CREATE TABLE IF NOT EXISTS preferences (
//!                 user_id INTEGER NOT NULL,
//!                 preferences TEXT NOT NULL
//!             )",
//!             [],
//!         )?;
//!         Ok(())
//!     }
//!     fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
//!         let mut stmt = tx.prepare(
//!             "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='preferences'",
//!         )?;
//!         let count: i64 = stmt.query_row([], |row| row.get(0))?;
//!         Ok(if count == 0 { Precondition::NeedsApply } else { Precondition::AlreadySatisfied })
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//! ```
//!
//! When the precondition returns `Precondition::AlreadySatisfied`, the migration up() logic will not be run, but the migration will still be recorded as applied.
//!
//! This way, in each of the possible cases, the database will be brought to the desired state.
//! - No migrations yet run -> both migration preconditions will return `Precondition::NeedsApply` and will be run.
//! - Migration 1 already applied -> `CreateUsersTable.precondition()` will return `Precondition::AlreadySatisfied`, but `CreatePreferencesTable.precondition()` will return `Precondition::NeedsApply`, and that will be run.
//! - Migration 1 and 2 already applied -> both migration preconditions will return `Precondition::AlreadySatisfied`, and neither migration up() logic will be run.
//!
//! Note: the old migration library will have used its own table to track migrations. You may want to add a new migration at this point to clean up that migration table.
//!
//! ```
//! # use migratio::{Migration, Error};
//! # use rusqlite::Transaction;
//! // in this case, old migration library was alembic
//! struct CleanupAlembicTrackingTable;
//! impl Migration for CleanupAlembicTrackingTable {
//!     fn version(&self) -> u32 { 3 }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("DROP TABLE IF EXISTS alembic_version", [])?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//! ```
//!
//! # Error Handling
//!
//! When migrations fail, you can inspect the failure details:
//!
//! ```
//! use migratio::{Migration, Error};
//! use migratio::sqlite::SqliteMigrator;
//! use rusqlite::{Connection, Transaction};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! struct Migration2;
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 { 2 }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         // This will fail
//!         tx.execute("INVALID SQL", [])?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! let mut conn = Connection::open_in_memory().unwrap();
//! let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
//! let report = migrator.upgrade(&mut conn).unwrap();
//!
//! // Expect the second migration has failed
//! assert!(report.failing_migration.is_some());
//! let failure = report.failing_migration.unwrap();
//! assert_eq!(failure.migration().version(), 2);
//! assert_eq!(failure.migration().name(), "Migration 2");
//! assert!(failure.error().to_string().contains("near \"INVALID\": syntax error"));
//!
//! // Expect that the report indicates only the first migration was run
//! assert_eq!(report.migrations_run, vec![1]);
//!
//! // The database is left in the state it was in after the last successful migration (here, version 1)
//! assert_eq!(migrator.get_current_version(&mut conn).unwrap(), 1);
//! let mut stmt = conn.prepare(
//!    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1"
//! ).unwrap();
//! let count: i64 = stmt.query_row(["users"], |row| row.get(0)).unwrap();
//! assert_eq!(count, 1);
//! ```
//!
//! # Rollback Support
//!
//! Migrations can optionally implement the `sqlite_down()` method to enable rollback via `downgrade()`:
//!
//! ```
//! use migratio::{Migration, Error};
//! use migratio::sqlite::SqliteMigrator;
//! use rusqlite::{Connection, Transaction};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
//!         Ok(())
//!     }
//!     fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("DROP TABLE users", [])?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! let mut conn = Connection::open_in_memory().unwrap();
//! let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
//!
//! // Apply migration
//! migrator.upgrade(&mut conn).unwrap();
//!
//! // Rollback to version 0 (removes all migrations)
//! migrator.downgrade(&mut conn, 0).unwrap();
//!
//! // Or rollback to a specific version
//! // migrator.downgrade(&mut conn, 1).unwrap(); // Rollback to version 1
//! ```
//!
//! If a migration doesn't implement `down()`, calling `downgrade()` will panic with a helpful error message.
//!
//! # Preview / Dry-Run Mode
//!
//! Preview which migrations would be applied or rolled back without actually running them:
//!
//! ```
//! use migratio::{Migration, Error};
//! use migratio::sqlite::SqliteMigrator;
//! use rusqlite::{Connection, Transaction};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! struct Migration2;
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 { 2 }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("ALTER TABLE users ADD COLUMN email TEXT", [])?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! let mut conn = Connection::open_in_memory().unwrap();
//! let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
//!
//! // Preview pending migrations
//! let pending = migrator.preview_upgrade(&mut conn).unwrap();
//! assert_eq!(pending.len(), 2);
//! assert_eq!(pending[0].version(), 1);
//! assert_eq!(pending[1].version(), 2);
//!
//! // Actually apply them
//! migrator.upgrade(&mut conn).unwrap();
//!
//! // Preview what would be rolled back
//! let to_rollback = migrator.preview_downgrade(&mut conn, 0).unwrap();
//! assert_eq!(to_rollback.len(), 2);
//!
//! // Check current version
//! let current = migrator.get_current_version(&mut conn).unwrap();
//! assert_eq!(current, 2);
//! ```
//!
//! # Migration History
//!
//! Query the history of all applied migrations for auditing and debugging:
//!
//! ```
//! use migratio::{Migration, Error};
//! use migratio::sqlite::SqliteMigrator;
//! use rusqlite::{Connection, Transaction};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
//!         Ok(())
//!     }
//!     fn name(&self) -> String {
//!         "create_users_table".to_string()
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! let mut conn = Connection::open_in_memory().unwrap();
//! let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
//! migrator.upgrade(&mut conn).unwrap();
//!
//! // Get full migration history
//! let history = migrator.get_migration_history(&mut conn).unwrap();
//! assert_eq!(history.len(), 1);
//! assert_eq!(history[0].version, 1);
//! assert_eq!(history[0].name, "create_users_table");
//! assert!(!history[0].checksum.is_empty());
//! // applied_at is a timestamp, just verify it's parseable
//! assert!(history[0].applied_at.to_rfc3339().len() > 0);
//! ```
//!
//! # Observability Hooks
//!
//! Set callbacks to observe migration progress, useful for logging and metrics:
//!
//! ```
//! use migratio::{Migration, Error};
//! use migratio::sqlite::SqliteMigrator;
//! use rusqlite::{Connection, Transaction};
//! use std::sync::{Arc, Mutex};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
//!         Ok(())
//!     }
//!     fn name(&self) -> String {
//!         "create_users".to_string()
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! let events = Arc::new(Mutex::new(Vec::new()));
//!
//! let events_clone1 = Arc::clone(&events);
//! let events_clone2 = Arc::clone(&events);
//! let events_clone3 = Arc::clone(&events);
//!
//! let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
//!     .on_migration_start(move |version, name| {
//!         events_clone1.lock().unwrap().push(format!("Starting migration {} ({})", version, name));
//!     })
//!     .on_migration_complete(move |version, name, _duration| {
//!         events_clone2.lock().unwrap().push(format!("Completed migration {} ({})", version, name));
//!     })
//!     .on_migration_error(move |version, name, error| {
//!         events_clone3.lock().unwrap().push(format!("Migration {} ({}) failed: {:?}", version, name, error));
//!     });
//!
//! let mut conn = Connection::open_in_memory().unwrap();
//! migrator.upgrade(&mut conn).unwrap();
//!
//! assert_eq!(*events.lock().unwrap(), vec![
//!     "Starting migration 1 (create_users)",
//!     "Completed migration 1 (create_users)",
//! ]);
//! ```
//!
//! # Tracing Integration
//!
//! Enable the `tracing` feature for automatic structured logging using the `tracing` crate:
//!
//! ```toml
//! [dependencies]
//! migratio = { version = "0.1", features = ["tracing"] }
//! ```
//!
//! When enabled, migrations automatically emit tracing spans and events:
//!
//! ```rust
//! # #[cfg(not(feature = "tracing"))]
//! # fn main() {}
//! # #[cfg(feature = "tracing")]
//! # fn main() {
//! use migratio::{Migration, Error};
//! use migratio::sqlite::SqliteMigrator;
//! use rusqlite::{Connection, Transaction};
//! use tracing_subscriber;
//! use std::sync::{Arc, Mutex};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
//!         Ok(())
//!     }
//!     fn name(&self) -> String {
//!         "create_users".to_string()
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! // Capture tracing events for testing
//! let events = Arc::new(Mutex::new(Vec::<u8>::new()));
//! let events_clone = Arc::clone(&events);
//!
//! // Set up tracing subscriber without timestamps or colors for reproducible output
//! let subscriber = tracing_subscriber::fmt()
//!     .with_max_level(tracing::Level::INFO)
//!     .without_time()
//!     .with_target(false)
//!     .with_ansi(false)
//!     .with_writer(move || {
//!         struct W(Arc<Mutex<Vec<u8>>>);
//!         impl std::io::Write for W {
//!             fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
//!                 self.0.lock().unwrap().extend_from_slice(buf);
//!                 Ok(buf.len())
//!             }
//!             fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
//!         }
//!         W(events_clone.clone())
//!     })
//!     .finish();
//!
//! tracing::subscriber::with_default(subscriber, || {
//!     let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
//!     let mut conn = Connection::open_in_memory().unwrap();
//!     migrator.upgrade(&mut conn).unwrap();
//! });
//!
//! // Verify the exact captured tracing output
//! let output = String::from_utf8(events.lock().unwrap().clone()).unwrap();
//! assert_eq!(output, r#" INFO migration_up{version=1 name=create_users}: Starting migration
//!  INFO migration_up{version=1 name=create_users}: Migration completed successfully duration_ms=0
//! "#);
//! # }
//! ```
//!
//! # Migration Testing Utilities
//!
//! Enable the `testing` feature to access utilities for comprehensive migration testing:
//!
//! ```toml
//! [dev-dependencies]
//! migratio = { version = "0.1", features = ["testing"] }
//! ```
//!
//! The `SqliteTestHarness` provides controlled migration state management, query helpers,
//! and schema assertions:
//!
//! ```rust
//! # #[cfg(not(feature = "testing"))]
//! # fn main() {}
//! # #[cfg(feature = "testing")]
//! # fn main() {
//! use migratio::testing::SqliteTestHarness;
//! use migratio::{Migration, Error};
//! use migratio::sqlite::SqliteMigrator;
//! use rusqlite::Transaction;
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
//!         Ok(())
//!     }
//!     fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("DROP TABLE users", [])?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! // in legitimate cases, this function would be a #[test]
//! fn test_migration_1() {
//!     let mut harness = SqliteTestHarness::new(SqliteMigrator::new(vec![Box::new(Migration1)]));
//!
//!     // Migrate to version 1
//!     harness.migrate_to(1).unwrap();
//!
//!     // Convenience method: assert table exists
//!     harness.assert_table_exists("users").unwrap();
//!
//!     // Insert test data
//!     harness.execute("INSERT INTO users VALUES (1, 'alice')").unwrap();
//!
//!     // Convenience method: query one row
//!     let name: String = harness.query_one("SELECT name FROM users WHERE id = 1").unwrap();
//!     assert_eq!(name, "alice");
//!
//!     // Test reversibility
//!     let schema_at_1 = harness.capture_schema().unwrap();
//!     harness.migrate_to(0).unwrap();
//!     harness.migrate_to(1).unwrap();
//!     harness.assert_schema_matches(&schema_at_1).unwrap();
//! }
//! # test_migration_1();
//! # }
//! ```
//!
//! ## Testing Data Transformations
//!
//! The harness is particularly useful for testing complex data transformations:
//!
//! ```rust
//! # #[cfg(not(feature = "testing"))]
//! # fn main() {}
//! # #[cfg(feature = "testing")]
//! # fn main() {
//! use migratio::{Migration, MigrationReport, Error, testing::SqliteTestHarness};
//! use migratio::sqlite::SqliteMigrator;
//! use rusqlite::Transaction;
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute(
//!             "CREATE TABLE user_preferences (name TEXT PRIMARY KEY, preferences TEXT)",
//!             [],
//!         )?;
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! struct Migration2;
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 {
//!         2
//!     }
//!     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
//!         // read all user preferences
//!         let mut stmt = tx.prepare("SELECT name, preferences FROM user_preferences")?;
//!         let rows = stmt.query_map([], |row| {
//!             let name: String = row.get(0)?;
//!             let preferences: String = row.get(1)?;
//!             Ok((name, preferences))
//!         })?;
//!
//!         // transform the preferences data
//!         for row in rows {
//!             let (name, preferences) = row?;
//!             let key_value_pairs = preferences
//!                 .split("|")
//!                 .map(|x| {
//!                     let mut split = x.split(":");
//!                     let key = split.next().unwrap();
//!                     let value = split.next().unwrap();
//!                     format!("\"{}\":\"{}\"", key, value)
//!                 })
//!                 .collect::<Vec<String>>();
//!             let new_preferences = format!("{{{}}}", key_value_pairs.join(","));
//!             tx.execute(
//!                 "UPDATE user_preferences SET preferences = ? WHERE name = ?",
//!                 [new_preferences, name],
//!             )?;
//!         }
//!
//!         Ok(())
//!     }
//! #   #[cfg(feature = "mysql")]
//! #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
//! }
//!
//! // in legitimate cases, this function would be a #[test]
//! fn test_data_transformation_migration() {
//!     let mut harness = SqliteTestHarness::new(SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]));
//!
//!     // Set up test data in old format
//!     harness.migrate_to(1).unwrap();
//!     harness.execute("INSERT INTO user_preferences VALUES ('alice', 'theme:dark|help:off')").unwrap();
//!
//!     // Run migration that transforms data
//!     harness.migrate_up_one().unwrap();
//!
//!     // Assert transformation succeeded
//!     let prefs: String = harness.query_one("SELECT preferences FROM user_preferences WHERE name = 'alice'").unwrap();
//!     assert_eq!(prefs, r#"{"theme":"dark","help":"off"}"#);
//! }
//! # test_data_transformation_migration();
//! # }
//! ```
//!
//! ## Available Test Methods
//!
//! - **Navigation**: `migrate_to()`, `migrate_up_one()`, `migrate_down_one()`, `current_version()`
//! - **Queries**: `execute()`, `query_one()`, `query_all()`, `query_map()`
//! - **Schema Assertions**: `assert_table_exists()`, `assert_column_exists()`, `assert_index_exists()`
//! - **Schema Snapshots**: `capture_schema()`, `assert_schema_matches()`
//!

use crate::{
    core::DEFAULT_VERSION_TABLE_NAME, error::Error, AppliedMigration, Migration, MigrationFailure,
    MigrationReport, Precondition,
};
use chrono::Utc;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::time::Instant;

/// The entrypoint for running a sequence of [Migration]s.
/// Construct this struct with the list of all [Migration]s to be applied.
/// [Migration::version]s must be contiguous, greater than zero, and unique.
pub struct SqliteMigrator {
    migrations: Vec<Box<dyn Migration>>,
    schema_version_table_name: String,
    busy_timeout: std::time::Duration,
    on_migration_start: Option<Box<dyn Fn(u32, &str) + Send + Sync>>,
    on_migration_complete: Option<Box<dyn Fn(u32, &str, std::time::Duration) + Send + Sync>>,
    on_migration_skipped: Option<Box<dyn Fn(u32, &str) + Send + Sync>>,
    on_migration_error: Option<Box<dyn Fn(u32, &str, &Error) + Send + Sync>>,
}

// Manual Debug impl since closures don't implement Debug
impl std::fmt::Debug for SqliteMigrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMigrator")
            .field("migrations", &self.migrations)
            .field("schema_version_table_name", &self.schema_version_table_name)
            .field("busy_timeout", &self.busy_timeout)
            .field("on_migration_start", &self.on_migration_start.is_some())
            .field(
                "on_migration_complete",
                &self.on_migration_complete.is_some(),
            )
            .field("on_migration_skipped", &self.on_migration_skipped.is_some())
            .field("on_migration_error", &self.on_migration_error.is_some())
            .finish()
    }
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
            on_migration_start: None,
            on_migration_complete: None,
            on_migration_skipped: None,
            on_migration_error: None,
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

    /// Set a callback to be invoked when a migration starts.
    /// The callback receives the migration version and name.
    ///
    /// # Example
    /// ```
    /// use migratio::sqlite::SqliteMigrator;
    /// # use migratio::Migration;
    /// # use rusqlite::Transaction;
    /// # use migratio::Error;
    ///
    /// # struct Migration1;
    /// # impl Migration for Migration1 {
    /// #     fn version(&self) -> u32 { 1 }
    /// #     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> { Ok(()) }
    /// #     #[cfg(feature = "mysql")]
    /// #     fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
    /// # }
    /// let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
    ///     .on_migration_start(|version, name| {
    ///         println!("Starting migration {} ({})", version, name);
    ///     });
    /// ```
    pub fn on_migration_start<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str) + Send + Sync + 'static,
    {
        self.on_migration_start = Some(Box::new(callback));
        self
    }

    /// Set a callback to be invoked when a migration completes successfully.
    /// The callback receives the migration version, name, and duration.
    ///
    /// # Example
    /// ```
    /// use migratio::sqlite::SqliteMigrator;
    /// # use migratio::Migration;
    /// # use rusqlite::Transaction;
    /// # use migratio::Error;
    ///
    /// # struct Migration1;
    /// # impl Migration for Migration1 {
    /// #     fn version(&self) -> u32 { 1 }
    /// #     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> { Ok(()) }
    /// #     #[cfg(feature = "mysql")]
    /// #     fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
    /// # }
    /// let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
    ///     .on_migration_complete(|version, name, duration| {
    ///         println!("Migration {} ({}) completed in {:?}", version, name, duration);
    ///     });
    /// ```
    pub fn on_migration_complete<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str, std::time::Duration) + Send + Sync + 'static,
    {
        self.on_migration_complete = Some(Box::new(callback));
        self
    }

    /// Set a callback to be invoked when a migration is skipped because its precondition
    /// returned `Precondition::AlreadySatisfied`.
    /// The callback receives the migration version and name.
    ///
    /// # Example
    /// ```
    /// use migratio::sqlite::SqliteMigrator;
    /// # use migratio::Migration;
    /// # use rusqlite::Transaction;
    /// # use migratio::Error;
    ///
    /// # struct Migration1;
    /// # impl Migration for Migration1 {
    /// #     fn version(&self) -> u32 { 1 }
    /// #     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> { Ok(()) }
    /// #     #[cfg(feature = "mysql")]
    /// #     fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
    /// # }
    /// let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
    ///     .on_migration_skipped(|version, name| {
    ///         println!("Migration {} ({}) skipped - already satisfied", version, name);
    ///     });
    /// ```
    pub fn on_migration_skipped<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str) + Send + Sync + 'static,
    {
        self.on_migration_skipped = Some(Box::new(callback));
        self
    }

    /// Set a callback to be invoked when a migration fails.
    /// The callback receives the migration version, name, and error.
    ///
    /// # Example
    /// ```
    /// use migratio::sqlite::SqliteMigrator;
    /// # use migratio::Migration;
    /// # use rusqlite::Transaction;
    /// # use migratio::Error;
    ///
    /// # struct Migration1;
    /// # impl Migration for Migration1 {
    /// #     fn version(&self) -> u32 { 1 }
    /// #     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> { Ok(()) }
    /// #     #[cfg(feature = "mysql")]
    /// #     fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
    /// # }
    /// let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
    ///     .on_migration_error(|version, name, error| {
    ///         eprintln!("Migration {} ({}) failed: {:?}", version, name, error);
    ///     });
    /// ```
    pub fn on_migration_error<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str, &Error) + Send + Sync + 'static,
    {
        self.on_migration_error = Some(Box::new(callback));
        self
    }

    /// Get a reference to all migrations in this migrator.
    pub fn migrations(&self) -> &[Box<dyn Migration>] {
        &self.migrations
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
    /// Upgrade the database to a specific target version.
    ///
    /// This runs all pending migrations up to and including the target version.
    /// If the database is already at or beyond the target version, no migrations are run.
    pub fn upgrade_to(
        &self,
        conn: &mut Connection,
        target_version: u32,
    ) -> Result<MigrationReport<'_>, Error> {
        // Validate target version exists
        if target_version > 0
            && !self
                .migrations
                .iter()
                .any(|m| m.version() == target_version)
        {
            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                format!(
                    "Target version {} does not exist in migration list",
                    target_version
                ),
            )));
        }

        self.upgrade_internal(conn, Some(target_version))
    }

    /// Upgrade the database by running all pending migrations.
    pub fn upgrade(&self, conn: &mut Connection) -> Result<MigrationReport<'_>, Error> {
        self.upgrade_internal(conn, None)
    }

    fn upgrade_internal(
        &self,
        conn: &mut Connection,
        target_version: Option<u32>,
    ) -> Result<MigrationReport<'_>, Error> {
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
                // Also detect missing migrations (in DB but not in code)
                for (applied_version, applied_name, applied_checksum) in &applied_migrations {
                    if let Some(migration) = self
                        .migrations
                        .iter()
                        .find(|m| m.version() == *applied_version)
                    {
                        let current_checksum = Self::calculate_checksum(migration);
                        if current_checksum != *applied_checksum {
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
                    } else {
                        // Migration exists in database but not in code - this is a missing migration
                        return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                            format!(
                                "Migration {} ('{}') was previously applied but is no longer present in the migration list. \
                                Applied migrations cannot be removed from the codebase.",
                                applied_version,
                                applied_name
                            ),
                        )));
                    }
                }

                // Detect orphaned migrations (in code but applied migrations are not contiguous)
                // For example: DB has [1, 2, 4] but code has [1, 2, 3, 4]
                // This means migration 3 was added after migration 4 was already applied
                let applied_versions: Vec<u32> =
                    applied_migrations.iter().map(|(v, _, _)| *v).collect();
                if !applied_versions.is_empty() {
                    let max_applied = *applied_versions.iter().max().unwrap();

                    // Check if all migrations up to max_applied exist in the database
                    for expected_version in 1..=max_applied {
                        if !applied_versions.contains(&expected_version) {
                            // There's a gap - check if this migration exists in our code
                            if let Some(missing_migration) = self
                                .migrations
                                .iter()
                                .find(|m| m.version() == expected_version)
                            {
                                return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                                    format!(
                                        "Migration {} ('{}') exists in code but was not applied, yet later migrations are already applied. \
                                        This likely means migration {} was added after migration {} was already applied. \
                                        Applied migrations: {:?}",
                                        expected_version,
                                        missing_migration.name(),
                                        expected_version,
                                        max_applied,
                                        applied_versions
                                    ),
                                )));
                            }
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

        #[cfg(feature = "tracing")]
        tracing::debug!(
            current_version = current_version,
            target_version = ?target_version,
            available_migrations = ?migrations_sorted.iter().map(|(v, m)| (*v, m.name())).collect::<Vec<_>>(),
            "Considering migrations to run"
        );

        for (migration_version, migration) in migrations_sorted {
            // Stop if we've reached the target version (if specified)
            if let Some(target) = target_version {
                if migration_version > target {
                    #[cfg(feature = "tracing")]
                    tracing::debug!(
                        migration_version = migration_version,
                        target_version = target,
                        "Skipping migration (beyond target version)"
                    );
                    break;
                }
            }

            if current_version < migration_version {
                #[cfg(feature = "tracing")]
                tracing::debug!(
                    migration_version = migration_version,
                    migration_name = %migration.name(),
                    "Migration needs to be applied"
                );
                #[cfg(feature = "tracing")]
                let _span = tracing::info_span!(
                    "migration_up",
                    version = migration_version,
                    name = %migration.name()
                )
                .entered();

                #[cfg(feature = "tracing")]
                tracing::info!("Starting migration");

                // Call on_migration_start hook
                if let Some(ref callback) = self.on_migration_start {
                    callback(migration_version, &migration.name());
                }

                let migration_start = Instant::now();

                // Run migration or stamp if precondition is satisfied
                // We wrap this in a block to ensure the transaction is dropped before we use conn
                let migration_result = {
                    // Start a transaction for this migration
                    let tx = conn.transaction()?;

                    // Check precondition
                    let precondition = match migration.sqlite_precondition(&tx) {
                        Ok(p) => p,
                        Err(error) => {
                            #[cfg(feature = "tracing")]
                            tracing::error!(
                                error = %error,
                                "Precondition check failed"
                            );

                            // Call on_migration_error hook
                            if let Some(ref callback) = self.on_migration_error {
                                callback(migration_version, &migration.name(), &error);
                            }

                            // Transaction will be automatically rolled back when dropped
                            failing_migration = Some(MigrationFailure { migration, error });
                            break;
                        }
                    };

                    match precondition {
                        Precondition::AlreadySatisfied => {
                            #[cfg(feature = "tracing")]
                            tracing::info!("Precondition already satisfied, stamping migration without running up()");

                            // Call on_migration_skipped hook
                            if let Some(ref callback) = self.on_migration_skipped {
                                callback(migration_version, &migration.name());
                            }

                            // Commit the transaction (even though we didn't run up())
                            tx.commit()?;
                            Ok(())
                        }
                        Precondition::NeedsApply => {
                            let result = migration.sqlite_up(&tx);
                            if result.is_ok() {
                                // Commit the transaction
                                tx.commit()?;
                            }
                            result
                        }
                    }
                };

                match migration_result {
                    Ok(_) => {
                        let migration_duration = migration_start.elapsed();

                        #[cfg(feature = "tracing")]
                        tracing::info!(
                            duration_ms = migration_duration.as_millis(),
                            "Migration completed successfully"
                        );

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

                        // Call on_migration_complete hook
                        if let Some(ref callback) = self.on_migration_complete {
                            callback(migration_version, &migration.name(), migration_duration);
                        }
                    }
                    Err(e) => {
                        #[cfg(feature = "tracing")]
                        tracing::error!(
                            error = %e,
                            "Migration failed"
                        );

                        // Call on_migration_error hook
                        if let Some(ref callback) = self.on_migration_error {
                            callback(migration_version, &migration.name(), &e);
                        }

                        // Transaction will be automatically rolled back when dropped
                        failing_migration = Some(MigrationFailure {
                            migration,
                            error: e,
                        });
                        break;
                    }
                }
            } else {
                #[cfg(feature = "tracing")]
                tracing::debug!(
                    migration_version = migration_version,
                    current_version = current_version,
                    "Skipping migration (already applied)"
                );
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

            // Verify checksums and detect missing migrations
            for (applied_version, applied_name, applied_checksum) in &applied_migrations {
                if let Some(migration) = self
                    .migrations
                    .iter()
                    .find(|m| m.version() == *applied_version)
                {
                    let current_checksum = Self::calculate_checksum(migration);
                    if current_checksum != *applied_checksum {
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
                } else {
                    // Migration exists in database but not in code
                    return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                        format!(
                            "Migration {} ('{}') was previously applied but is no longer present in the migration list. \
                            Applied migrations cannot be removed from the codebase.",
                            applied_version,
                            applied_name
                        ),
                    )));
                }
            }

            // Detect orphaned migrations (gaps in applied migrations with code present)
            let applied_versions: Vec<u32> =
                applied_migrations.iter().map(|(v, _, _)| *v).collect();
            if !applied_versions.is_empty() {
                let max_applied = *applied_versions.iter().max().unwrap();

                for expected_version in 1..=max_applied {
                    if !applied_versions.contains(&expected_version) {
                        if let Some(missing_migration) = self
                            .migrations
                            .iter()
                            .find(|m| m.version() == expected_version)
                        {
                            return Err(Error::Rusqlite(rusqlite::Error::InvalidParameterName(
                                format!(
                                    "Migration {} ('{}') exists in code but was not applied, yet later migrations are already applied. \
                                    This likely means migration {} was added after migration {} was already applied. \
                                    Applied migrations: {:?}",
                                    expected_version,
                                    missing_migration.name(),
                                    expected_version,
                                    max_applied,
                                    applied_versions
                                ),
                            )));
                        }
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
            #[cfg(feature = "tracing")]
            let _span = tracing::info_span!(
                "migration_down",
                version = migration_version,
                name = %migration.name()
            )
            .entered();

            #[cfg(feature = "tracing")]
            tracing::info!("Rolling back migration");

            // Call on_migration_start hook
            if let Some(ref callback) = self.on_migration_start {
                callback(migration_version, &migration.name());
            }

            let migration_start = Instant::now();

            // Start a transaction for this migration rollback
            let tx = conn.transaction()?;
            let migration_result = migration.sqlite_down(&tx);

            match migration_result {
                Ok(_) => {
                    // Commit the transaction
                    tx.commit()?;

                    let migration_duration = migration_start.elapsed();

                    #[cfg(feature = "tracing")]
                    tracing::info!(
                        duration_ms = migration_duration.as_millis(),
                        "Migration rolled back successfully"
                    );

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

                    // Call on_migration_complete hook
                    if let Some(ref callback) = self.on_migration_complete {
                        callback(migration_version, &migration.name(), migration_duration);
                    }
                }
                Err(e) => {
                    #[cfg(feature = "tracing")]
                    tracing::error!(
                        error = %e,
                        "Migration rollback failed"
                    );

                    // Call on_migration_error hook
                    if let Some(ref callback) = self.on_migration_error {
                        callback(migration_version, &migration.name(), &e);
                    }

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
    use rusqlite::Transaction;

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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // do something that works
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test2 (id) SELECT id FROM test", [])?;
                tx.execute("DROP TABLE test", [])?;
                // but then do something that fails
                tx.execute("bleep blorp", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
                    error: Error::Rusqlite(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error {
                            code: rusqlite::ffi::ErrorCode::Unknown,
                            extended_code: 1
                        },
                        Some("near \"bleep\": syntax error".to_string())
                    ))
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
    fn upgrade_to_specific_version() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);

        // Upgrade to version 2 only
        let report = migrator.upgrade_to(&mut conn, 2).unwrap();
        assert_eq!(report.migrations_run, vec![1, 2]);

        // Verify we're at version 2
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 2);

        // Verify only tables for migrations 1 and 2 exist
        let table_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('test1', 'test2', 'test3')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 2); // test1 and test2, but not test3

        // Now upgrade to version 3
        let report = migrator.upgrade_to(&mut conn, 3).unwrap();
        assert_eq!(report.migrations_run, vec![3]);

        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 3);
    }

    #[test]
    fn upgrade_to_nonexistent_version() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);

        let result = migrator.upgrade_to(&mut conn, 5);
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("Target version 5 does not exist"));
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // this will succeed
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                // move all data from test to test2
                tx.execute("INSERT INTO test2 (id) SELECT id FROM test", [])?;
                // drop test
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }
        // define a migration that will fail
        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // do something that works
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test3 (id) SELECT id FROM test2", [])?;
                tx.execute("DROP TABLE test2", [])?;
                // then do something that fails
                tx.execute("bleep blorp", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
                    error: Error::Rusqlite(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error {
                            code: rusqlite::ffi::ErrorCode::Unknown,
                            extended_code: 1
                        },
                        Some("near \"bleep\": syntax error".to_string())
                    ))
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // this will succeed
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test2 (id) SELECT id FROM test", [])?;
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }
        // define a migration that will fail
        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // do something that works
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test3 (id) SELECT id FROM test2", [])?;
                tx.execute("DROP TABLE test2", [])?;
                // then do something that fails
                tx.execute("bleep blorp", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
                    error: Error::Rusqlite(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error {
                            code: rusqlite::ffi::ErrorCode::Unknown,
                            extended_code: 1
                        },
                        Some("near \"bleep\": syntax error".to_string())
                    ))
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // This migration succeeds
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test2 (id) SELECT id FROM test", [])?;
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        // Define a migration that panics
        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // Make some changes
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                tx.execute("INSERT INTO test3 (id) SELECT id FROM test2", [])?;
                tx.execute("DROP TABLE test2", [])?;
                // Then panic
                panic!("Migration panic!");
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1_table".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2_table".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test3_table".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2a;
        impl Migration for Migration2a {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2b;
        impl Migration for Migration2b {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2a;
        impl Migration for Migration2a {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2b;
        impl Migration for Migration2b {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test_table".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test_table_modified".to_string() // Different name!
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1_table".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2_table".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "my_migration".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test3", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            fn sqlite_down(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            // No down() implementation - uses default that panics
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test_table".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test_table_modified".to_string() // Different!
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test3", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            fn sqlite_down(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test_table".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // Intentionally fail
                tx.execute("INVALID SQL HERE", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "failing_migration".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("INVALID SQL", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1_table".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2_table".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "migration_one".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "migration_two".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "test_migration".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // Simulate some work
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                std::thread::sleep(std::time::Duration::from_millis(10));
                tx.execute("INSERT INTO test1 (id) VALUES (1)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                std::thread::sleep(std::time::Duration::from_millis(10));
                tx.execute("INSERT INTO test2 (id) VALUES (1)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
                fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                    Ok(())
                }
                fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("DROP TABLE test1", [])?;
                    Ok(())
                }
                #[cfg(feature = "mysql")]
                fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                    Ok(())
                }
            }

            struct Migration2;
            impl Migration for Migration2 {
                fn version(&self) -> u32 {
                    2
                }
                fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                    Ok(())
                }
                fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("DROP TABLE test2", [])?;
                    Ok(())
                }
                #[cfg(feature = "mysql")]
                fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
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

    #[test]
    fn detects_missing_migration_in_code() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test3".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        // Apply all three migrations
        let migrator = SqliteMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);
        migrator.upgrade(&mut conn).unwrap();

        // Now manually delete Migration2 from the database to simulate it being removed
        conn.execute("DELETE FROM _migratio_version_ WHERE version = 2", [])
            .unwrap();

        // Now try to run with all migrations - should detect that Migration2 was in the DB
        // but is now missing (we still have it in code, but DB shows it's gone)
        // Actually, let's simulate the opposite: Migration2 WAS applied but we removed it from code
        // We need to use a different Migration2 struct or manually manipulate the DB

        // Let's manually insert a migration that doesn't exist in our code
        conn.execute(
            "INSERT INTO _migratio_version_ (version, name, applied_at, checksum) VALUES (4, 'deleted_migration', datetime('now'), 'fakechecksum')",
            [],
        )
        .unwrap();

        // Now try to upgrade - should detect migration 4 exists in DB but not in code
        let result = migrator.upgrade(&mut conn);

        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("Migration 4"));
        assert!(err_msg.contains("deleted_migration"));
        assert!(err_msg.contains("was previously applied but is no longer present"));
    }

    #[test]
    fn detects_orphaned_migration_added_late() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test3".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        // Apply migrations 1 and 3 only (simulating migration 2 being added later)
        // First apply migration 1
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Manually insert migration 3 into the database to simulate it being applied
        let checksum = {
            let migration = Box::new(Migration3) as Box<dyn Migration>;
            SqliteMigrator::calculate_checksum(&migration)
        };
        conn.execute(
            "INSERT INTO _migratio_version_ (version, name, applied_at, checksum) VALUES (3, 'create_test3', datetime('now'), ?1)",
            [&checksum],
        )
        .unwrap();

        // Now try to upgrade with all three migrations
        // This should fail because migration 2 exists in code but wasn't applied,
        // yet migration 3 (which comes after it) was already applied
        let migrator_all = SqliteMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);
        let result = migrator_all.upgrade(&mut conn);

        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("Migration 2"));
        assert!(err_msg.contains("create_test2"));
        assert!(err_msg.contains("exists in code but was not applied"));
        assert!(err_msg.contains("later migrations are already applied"));
    }

    #[test]
    fn detects_missing_migration_during_downgrade() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test1", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test2", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        // Apply both migrations
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        migrator.upgrade(&mut conn).unwrap();

        // Manually insert a migration that doesn't exist in code
        conn.execute(
            "INSERT INTO _migratio_version_ (version, name, applied_at, checksum) VALUES (3, 'orphaned_migration', datetime('now'), 'fakechecksum')",
            [],
        )
        .unwrap();

        // Try to downgrade - should detect migration 3 exists in DB but not in code
        let result = migrator.downgrade(&mut conn, 0);

        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("Migration 3"));
        assert!(err_msg.contains("orphaned_migration"));
        assert!(err_msg.contains("was previously applied but is no longer present"));
    }

    #[test]
    fn hooks_are_called_on_successful_migration() {
        use std::sync::{Arc, Mutex};

        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "test_migration".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        let start_calls = Arc::new(Mutex::new(Vec::new()));
        let complete_calls = Arc::new(Mutex::new(Vec::new()));
        let error_calls = Arc::new(Mutex::new(Vec::new()));

        let start_calls_clone = Arc::clone(&start_calls);
        let complete_calls_clone = Arc::clone(&complete_calls);
        let error_calls_clone = Arc::clone(&error_calls);

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
            .on_migration_start(move |version, name| {
                start_calls_clone
                    .lock()
                    .unwrap()
                    .push((version, name.to_string()));
            })
            .on_migration_complete(move |version, name, duration| {
                complete_calls_clone
                    .lock()
                    .unwrap()
                    .push((version, name.to_string(), duration));
            })
            .on_migration_error(move |version, name, error| {
                error_calls_clone.lock().unwrap().push((
                    version,
                    name.to_string(),
                    format!("{:?}", error),
                ));
            });

        migrator.upgrade(&mut conn).unwrap();

        // Verify hooks were called correctly
        let starts = start_calls.lock().unwrap();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0], (1, "test_migration".to_string()));

        let completes = complete_calls.lock().unwrap();
        assert_eq!(completes.len(), 1);
        assert_eq!(completes[0].0, 1);
        assert_eq!(completes[0].1, "test_migration");
        // Duration is always non-negative, just verify it exists
        let _ = completes[0].2;

        let errors = error_calls.lock().unwrap();
        assert_eq!(errors.len(), 0); // No errors should have been called
    }

    #[test]
    fn hooks_are_called_on_failed_migration() {
        use std::sync::{Arc, Mutex};

        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("INVALID SQL", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "failing_migration".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        let start_calls = Arc::new(Mutex::new(Vec::new()));
        let complete_calls = Arc::new(Mutex::new(Vec::new()));
        let error_calls = Arc::new(Mutex::new(Vec::new()));

        let start_calls_clone = Arc::clone(&start_calls);
        let complete_calls_clone = Arc::clone(&complete_calls);
        let error_calls_clone = Arc::clone(&error_calls);

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
            .on_migration_start(move |version, name| {
                start_calls_clone
                    .lock()
                    .unwrap()
                    .push((version, name.to_string()));
            })
            .on_migration_complete(move |version, name, duration| {
                complete_calls_clone
                    .lock()
                    .unwrap()
                    .push((version, name.to_string(), duration));
            })
            .on_migration_error(move |version, name, _error| {
                error_calls_clone
                    .lock()
                    .unwrap()
                    .push((version, name.to_string()));
            });

        let _ = migrator.upgrade(&mut conn); // Expect this to fail

        // Verify hooks were called correctly
        let starts = start_calls.lock().unwrap();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0], (1, "failing_migration".to_string()));

        let completes = complete_calls.lock().unwrap();
        assert_eq!(completes.len(), 0); // Complete should not be called on error

        let errors = error_calls.lock().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0], (1, "failing_migration".to_string()));
    }

    #[test]
    fn hooks_are_called_on_downgrade() {
        use std::sync::{Arc, Mutex};

        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE test", [])?;
                Ok(())
            }
            fn name(&self) -> String {
                "test_migration".to_string()
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        // First apply the migration
        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Now set up hooks for downgrade
        let start_calls = Arc::new(Mutex::new(Vec::new()));
        let complete_calls = Arc::new(Mutex::new(Vec::new()));

        let start_calls_clone = Arc::clone(&start_calls);
        let complete_calls_clone = Arc::clone(&complete_calls);

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
            .on_migration_start(move |version, name| {
                start_calls_clone
                    .lock()
                    .unwrap()
                    .push((version, name.to_string()));
            })
            .on_migration_complete(move |version, name, duration| {
                complete_calls_clone
                    .lock()
                    .unwrap()
                    .push((version, name.to_string(), duration));
            });

        migrator.downgrade(&mut conn, 0).unwrap();

        // Verify hooks were called
        let starts = start_calls.lock().unwrap();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0], (1, "test_migration".to_string()));

        let completes = complete_calls.lock().unwrap();
        assert_eq!(completes.len(), 1);
        assert_eq!(completes[0].0, 1);
        assert_eq!(completes[0].1, "test_migration");
    }

    #[test]
    #[cfg(feature = "tracing")]
    fn tracing_logs_successful_migration() {
        use tracing_test::traced_test;

        #[traced_test]
        fn run_test() {
            let mut conn = Connection::open_in_memory().unwrap();

            struct Migration1;
            impl Migration for Migration1 {
                fn version(&self) -> u32 {
                    1
                }
                fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                    Ok(())
                }
                fn name(&self) -> String {
                    "test_migration".to_string()
                }
                #[cfg(feature = "mysql")]
                fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                    Ok(())
                }
            }

            let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
            migrator.upgrade(&mut conn).unwrap();

            // Verify tracing logs were emitted
            assert!(logs_contain("Starting migration"));
            assert!(logs_contain("Migration completed successfully"));
            assert!(logs_contain("duration_ms"));
        }

        run_test();
    }

    #[test]
    #[cfg(feature = "tracing")]
    fn tracing_logs_failed_migration() {
        use tracing_test::traced_test;

        #[traced_test]
        fn run_test() {
            let mut conn = Connection::open_in_memory().unwrap();

            struct Migration1;
            impl Migration for Migration1 {
                fn version(&self) -> u32 {
                    1
                }
                fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("INVALID SQL", [])?;
                    Ok(())
                }
                fn name(&self) -> String {
                    "failing_migration".to_string()
                }
                #[cfg(feature = "mysql")]
                fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                    Ok(())
                }
            }

            let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
            let _ = migrator.upgrade(&mut conn);

            // Verify tracing error logs were emitted
            assert!(logs_contain("Starting migration"));
            assert!(logs_contain("Migration failed"));
        }

        run_test();
    }

    #[test]
    #[cfg(feature = "tracing")]
    fn tracing_logs_downgrade() {
        use tracing_test::traced_test;

        #[traced_test]
        fn run_test() {
            let mut conn = Connection::open_in_memory().unwrap();

            struct Migration1;
            impl Migration for Migration1 {
                fn version(&self) -> u32 {
                    1
                }
                fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
                    Ok(())
                }
                fn sqlite_down(&self, tx: &Transaction) -> Result<(), Error> {
                    tx.execute("DROP TABLE test", [])?;
                    Ok(())
                }
                fn name(&self) -> String {
                    "test_migration".to_string()
                }
                #[cfg(feature = "mysql")]
                fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                    Ok(())
                }
            }

            let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
            migrator.upgrade(&mut conn).unwrap();
            migrator.downgrade(&mut conn, 0).unwrap();

            // Verify tracing logs for downgrade were emitted
            assert!(logs_contain("Rolling back migration"));
            assert!(logs_contain("Migration rolled back successfully"));
        }

        run_test();
    }

    #[test]
    fn precondition_already_satisfied() {
        use std::sync::{Arc, Mutex};

        let mut conn = Connection::open_in_memory().unwrap();

        // Create table manually (simulating existing migration from another tool)
        conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])
            .unwrap();

        // Track whether up() was called
        let up_called = Arc::new(Mutex::new(false));
        let up_called_clone = Arc::clone(&up_called);

        struct Migration1 {
            up_called: Arc<Mutex<bool>>,
        }
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // Mark that up() was called
                *self.up_called.lock().unwrap() = true;
                tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
                // Check if table already exists
                let mut stmt = tx.prepare(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
                )?;
                let count: i64 = stmt.query_row([], |row| row.get(0))?;

                if count > 0 {
                    Ok(Precondition::AlreadySatisfied)
                } else {
                    Ok(Precondition::NeedsApply)
                }
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1 {
            up_called: up_called_clone,
        })]);

        let report = migrator.upgrade(&mut conn).unwrap();

        // Verify migration was recorded as run
        assert_eq!(report.migrations_run, vec![1]);
        assert!(report.failing_migration.is_none());

        // Verify up() was NOT called
        assert!(!*up_called.lock().unwrap());

        // Verify migration is in version table
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 1);

        // Verify table still exists (wasn't modified)
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1);
    }

    #[test]
    fn precondition_needs_apply() {
        use std::sync::{Arc, Mutex};

        let mut conn = Connection::open_in_memory().unwrap();

        // Track whether up() was called
        let up_called = Arc::new(Mutex::new(false));
        let up_called_clone = Arc::clone(&up_called);

        struct Migration1 {
            up_called: Arc<Mutex<bool>>,
        }
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                // Mark that up() was called
                *self.up_called.lock().unwrap() = true;
                tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
                // Check if table already exists
                let mut stmt = tx.prepare(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
                )?;
                let count: i64 = stmt.query_row([], |row| row.get(0))?;

                if count > 0 {
                    Ok(Precondition::AlreadySatisfied)
                } else {
                    Ok(Precondition::NeedsApply)
                }
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1 {
            up_called: up_called_clone,
        })]);

        let report = migrator.upgrade(&mut conn).unwrap();

        // Verify migration was recorded as run
        assert_eq!(report.migrations_run, vec![1]);
        assert!(report.failing_migration.is_none());

        // Verify up() WAS called
        assert!(*up_called.lock().unwrap());

        // Verify migration is in version table
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 1);

        // Verify table was created
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1);
    }

    #[test]
    fn precondition_error() {
        let mut conn = Connection::open_in_memory().unwrap();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                Ok(())
            }
            fn sqlite_precondition(&self, _tx: &Transaction) -> Result<Precondition, Error> {
                // Simulate a precondition check error
                Err(rusqlite::Error::InvalidQuery.into())
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        // Verify migration failed
        assert_eq!(report.migrations_run, Vec::<u32>::new());
        assert!(report.failing_migration.is_some());

        let failure = report.failing_migration.unwrap();
        assert_eq!(failure.migration().version(), 1);

        // Verify migration is NOT in version table
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 0);
    }

    #[test]
    fn precondition_hooks_already_satisfied() {
        use std::sync::{Arc, Mutex};

        let mut conn = Connection::open_in_memory().unwrap();

        // Create table manually (simulating existing migration from another tool)
        conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])
            .unwrap();

        // Track hook calls
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone1 = Arc::clone(&events);
        let events_clone2 = Arc::clone(&events);
        let events_clone3 = Arc::clone(&events);
        let events_clone4 = Arc::clone(&events);

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, _tx: &Transaction) -> Result<(), Error> {
                // This should NOT be called
                panic!("up() should not be called when precondition is AlreadySatisfied");
            }
            fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
                // Check if table already exists
                let mut stmt = tx.prepare(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
                )?;
                let count: i64 = stmt.query_row([], |row| row.get(0))?;

                if count > 0 {
                    Ok(Precondition::AlreadySatisfied)
                } else {
                    Ok(Precondition::NeedsApply)
                }
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
            .on_migration_start(move |version, name| {
                events_clone1
                    .lock()
                    .unwrap()
                    .push(format!("start:{}:{}", version, name));
            })
            .on_migration_skipped(move |version, name| {
                events_clone2
                    .lock()
                    .unwrap()
                    .push(format!("skipped:{}:{}", version, name));
            })
            .on_migration_complete(move |version, name, _duration| {
                events_clone3
                    .lock()
                    .unwrap()
                    .push(format!("complete:{}:{}", version, name));
            })
            .on_migration_error(move |version, name, _error| {
                events_clone4
                    .lock()
                    .unwrap()
                    .push(format!("error:{}:{}", version, name));
            });

        let report = migrator.upgrade(&mut conn).unwrap();

        // Verify migration was recorded
        assert_eq!(report.migrations_run, vec![1]);
        assert!(report.failing_migration.is_none());

        // Verify hooks were called correctly
        let events_vec = events.lock().unwrap();
        assert_eq!(events_vec.len(), 3);
        assert_eq!(events_vec[0], "start:1:Migration 1");
        assert_eq!(events_vec[1], "skipped:1:Migration 1");
        assert_eq!(events_vec[2], "complete:1:Migration 1");
    }

    #[test]
    fn precondition_hooks_needs_apply() {
        use std::sync::{Arc, Mutex};

        let mut conn = Connection::open_in_memory().unwrap();

        // Track hook calls
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone1 = Arc::clone(&events);
        let events_clone2 = Arc::clone(&events);
        let events_clone3 = Arc::clone(&events);
        let events_clone4 = Arc::clone(&events);

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
                // Check if table already exists
                let mut stmt = tx.prepare(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
                )?;
                let count: i64 = stmt.query_row([], |row| row.get(0))?;

                if count > 0 {
                    Ok(Precondition::AlreadySatisfied)
                } else {
                    Ok(Precondition::NeedsApply)
                }
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![Box::new(Migration1)])
            .on_migration_start(move |version, name| {
                events_clone1
                    .lock()
                    .unwrap()
                    .push(format!("start:{}:{}", version, name));
            })
            .on_migration_skipped(move |version, name| {
                events_clone2
                    .lock()
                    .unwrap()
                    .push(format!("skipped:{}:{}", version, name));
            })
            .on_migration_complete(move |version, name, _duration| {
                events_clone3
                    .lock()
                    .unwrap()
                    .push(format!("complete:{}:{}", version, name));
            })
            .on_migration_error(move |version, name, _error| {
                events_clone4
                    .lock()
                    .unwrap()
                    .push(format!("error:{}:{}", version, name));
            });

        let report = migrator.upgrade(&mut conn).unwrap();

        // Verify migration was recorded
        assert_eq!(report.migrations_run, vec![1]);
        assert!(report.failing_migration.is_none());

        // Verify hooks were called correctly (no skipped hook)
        let events_vec = events.lock().unwrap();
        assert_eq!(events_vec.len(), 2);
        assert_eq!(events_vec[0], "start:1:Migration 1");
        assert_eq!(events_vec[1], "complete:1:Migration 1");
    }

    #[test]
    fn precondition_mixed_migrations() {
        use std::sync::{Arc, Mutex};

        let mut conn = Connection::open_in_memory().unwrap();

        // Create one table manually (simulating partial migration state)
        conn.execute("CREATE TABLE table1 (id INTEGER PRIMARY KEY)", [])
            .unwrap();

        // Track which migrations had up() called
        let up_calls = Arc::new(Mutex::new(Vec::new()));
        let up_calls_clone1 = Arc::clone(&up_calls);
        let up_calls_clone2 = Arc::clone(&up_calls);
        let up_calls_clone3 = Arc::clone(&up_calls);

        struct Migration1 {
            up_calls: Arc<Mutex<Vec<u32>>>,
        }
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                self.up_calls.lock().unwrap().push(1);
                tx.execute("CREATE TABLE table1 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
                let mut stmt = tx.prepare(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='table1'",
                )?;
                let count: i64 = stmt.query_row([], |row| row.get(0))?;
                Ok(if count > 0 {
                    Precondition::AlreadySatisfied
                } else {
                    Precondition::NeedsApply
                })
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2 {
            up_calls: Arc<Mutex<Vec<u32>>>,
        }
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                self.up_calls.lock().unwrap().push(2);
                tx.execute("CREATE TABLE table2 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
                let mut stmt = tx.prepare(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='table2'",
                )?;
                let count: i64 = stmt.query_row([], |row| row.get(0))?;
                Ok(if count > 0 {
                    Precondition::AlreadySatisfied
                } else {
                    Precondition::NeedsApply
                })
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3 {
            up_calls: Arc<Mutex<Vec<u32>>>,
        }
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
                self.up_calls.lock().unwrap().push(3);
                tx.execute("CREATE TABLE table3 (id INTEGER PRIMARY KEY)", [])?;
                Ok(())
            }
            fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
                let mut stmt = tx.prepare(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='table3'",
                )?;
                let count: i64 = stmt.query_row([], |row| row.get(0))?;
                Ok(if count > 0 {
                    Precondition::AlreadySatisfied
                } else {
                    Precondition::NeedsApply
                })
            }
            #[cfg(feature = "mysql")]
            fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = SqliteMigrator::new(vec![
            Box::new(Migration1 {
                up_calls: up_calls_clone1,
            }),
            Box::new(Migration2 {
                up_calls: up_calls_clone2,
            }),
            Box::new(Migration3 {
                up_calls: up_calls_clone3,
            }),
        ]);

        let report = migrator.upgrade(&mut conn).unwrap();

        // Verify all migrations were recorded
        assert_eq!(report.migrations_run, vec![1, 2, 3]);
        assert!(report.failing_migration.is_none());

        // Verify only migrations 2 and 3 had up() called (migration 1 was skipped)
        let calls = up_calls.lock().unwrap();
        assert_eq!(*calls, vec![2, 3]);

        // Verify all tables exist
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('table1', 'table2', 'table3')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 3);

        // Verify all migrations are in version table
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 3);
    }
}
