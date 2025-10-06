//! `migratio` is a lightweight library for managing database migrations (currently for Sqlite).
//!
//! Core concepts:
//! - `migratio` supplies migration definitions with a live connection to the database, allowing more expressive migration logic than just preparing SQL statements.
//! - `migratio` is a code-first library, making embedding it in your application easier than other CLI-first libraries.
//!
//! # Example
//!
//! ```
//! use migratio::{Migration, SqliteMigrator, MigrationReport, Error};
//! use rusqlite::{Connection, Transaction};
//!
//! // define your migrations as structs that implement the Migration trait
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
//!         Ok(())
//!     }
//! }
//!
//! struct Migration2;
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 {
//!         2
//!     }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("ALTER TABLE users ADD COLUMN email TEXT", [])?;
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
//! use migratio::{Migration, SqliteMigrator, MigrationReport, Error};
//! use rusqlite::{Connection, Transaction};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute(
//!             "CREATE TABLE user_preferences (name TEXT PRIMARY KEY, preferences TEXT)",
//!             [],
//!         )?;
//!         Ok(())
//!     }
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
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
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
//! ## Code-first approach
//!
//! A central use case for `migratio` is embedding migration logic within an application, usually in the startup procedure.
//! This way, when the application updates, the next time it starts the database will automatically be migrated to the latest version without any manual intervention.
//!
//! Anywhere you can construct a [SqliteMigrator] instance, you can access any feature this library provides.
//!
//! Consider this terse example of incorporating `migratio` into an application startup procedure:
//!
//! ```
//! # use migratio::{SqliteMigrator, Migration, Error};
//! # use rusqlite::{Connection, Transaction};
//! // migrations.rs
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
//!         Ok(())
//!     }
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
//! # use migratio::{SqliteMigrator, Migration, Error};
//! # use rusqlite::{Connection, Transaction};
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
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
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         // logic copied / translated from the other migration tool usage
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)", [])?;
//!         Ok(())
//!     }
//!     fn precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
//!         let mut stmt = tx.prepare(
//!             "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
//!         )?;
//!         let count: i64 = stmt.query_row([], |row| row.get(0))?;
//!         Ok(if count == 0 { Precondition::NeedsApply } else { Precondition::AlreadySatisfied })
//!     }
//! }
//!
//! struct CreatePreferencesTable;
//! impl Migration for CreatePreferencesTable {
//!     fn version(&self) -> u32 {
//!         2
//!     }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
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
//!     fn precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
//!         let mut stmt = tx.prepare(
//!             "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='preferences'",
//!         )?;
//!         let count: i64 = stmt.query_row([], |row| row.get(0))?;
//!         Ok(if count == 0 { Precondition::NeedsApply } else { Precondition::AlreadySatisfied })
//!     }
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
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("DROP TABLE IF EXISTS alembic_version", [])?;
//!         Ok(())
//!     }
//! }
//! ```
//!
//! # Error Handling
//!
//! When migrations fail, you can inspect the failure details:
//!
//! ```
//! use migratio::{Migration, SqliteMigrator, Error};
//! use rusqlite::{Connection, Transaction};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
//!         Ok(())
//!     }
//! }
//!
//! struct Migration2;
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 { 2 }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         // This will fail
//!         tx.execute("INVALID SQL", [])?;
//!         Ok(())
//!     }
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
//! assert_eq!(failure.error().to_string(), "near \"INVALID\": syntax error in INVALID SQL at offset 0");
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
//! Migrations can optionally implement the `down()` method to enable rollback via `downgrade()`:
//!
//! ```
//! use migratio::{Migration, SqliteMigrator, Error};
//! use rusqlite::{Connection, Transaction};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
//!         Ok(())
//!     }
//!     fn down(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("DROP TABLE users", [])?;
//!         Ok(())
//!     }
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
//! use migratio::{Migration, SqliteMigrator, Error};
//! use rusqlite::{Connection, Transaction};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
//!         Ok(())
//!     }
//! }
//!
//! struct Migration2;
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 { 2 }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("ALTER TABLE users ADD COLUMN email TEXT", [])?;
//!         Ok(())
//!     }
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
//! use migratio::{Migration, SqliteMigrator, Error};
//! use rusqlite::{Connection, Transaction};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
//!         Ok(())
//!     }
//!     fn name(&self) -> String {
//!         "create_users_table".to_string()
//!     }
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
//! use migratio::{Migration, SqliteMigrator, Error};
//! use rusqlite::{Connection, Transaction};
//! use std::sync::{Arc, Mutex};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
//!         Ok(())
//!     }
//!     fn name(&self) -> String {
//!         "create_users".to_string()
//!     }
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
//! use migratio::{Migration, SqliteMigrator, Error};
//! use rusqlite::{Connection, Transaction};
//! use tracing_subscriber;
//! use std::sync::{Arc, Mutex};
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
//!         Ok(())
//!     }
//!     fn name(&self) -> String {
//!         "create_users".to_string()
//!     }
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
//! The `MigrationTestHarness` provides controlled migration state management, query helpers,
//! and schema assertions:
//!
//! ```rust
//! # #[cfg(not(feature = "testing"))]
//! # fn main() {}
//!
//! # #[cfg(feature = "testing")]
//! # fn main() {
//! use migratio::testing::MigrationTestHarness;
//! use migratio::{Migration, Error};
//! use rusqlite::Transaction;
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 { 1 }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
//!         Ok(())
//!     }
//!     fn down(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute("DROP TABLE users", [])?;
//!         Ok(())
//!     }
//! }
//!
//! // in legitimate cases, this function would be a #[test]
//! fn test_migration_1() {
//!     let mut harness = MigrationTestHarness::new(vec![Box::new(Migration1)]);
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
//! use migratio::{Migration, SqliteMigrator, MigrationReport, Error, testing::MigrationTestHarness};
//! use rusqlite::Transaction;
//!
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
//!         tx.execute(
//!             "CREATE TABLE user_preferences (name TEXT PRIMARY KEY, preferences TEXT)",
//!             [],
//!         )?;
//!         Ok(())
//!     }
//! }
//!
//! struct Migration2;
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 {
//!         2
//!     }
//!     fn up(&self, tx: &Transaction) -> Result<(), Error> {
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
//! }
//!
//! // in legitimate cases, this function would be a #[test]
//! fn test_data_transformation_migration() {
//!     let mut harness = MigrationTestHarness::new(vec![Box::new(Migration1), Box::new(Migration2)]);
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

mod error;
mod migrator;

#[cfg(feature = "testing")]
pub mod testing;

pub use error::Error;
pub use migrator::{
    AppliedMigration, Migration, MigrationFailure, MigrationReport, Precondition, SqliteMigrator,
};
