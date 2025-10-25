//! `migratio` is a lightweight library for managing database migrations.
//!
//! Core concepts:
//! - `migratio` supplies migration definitions with a live connection to the database, allowing more expressive migration logic than just preparing SQL statements.
//! - `migratio` is a code-first library, making embedding it in your application easier than other CLI-first libraries.
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
//! ## Code-first approach
//!
//! A central use case for `migratio` is embedding migration logic within an application, usually in the startup procedure.
//! This way, when the application updates, the next time it starts the database will automatically be migrated to the latest version without any manual intervention.
//!
//! Anywhere you can construct a [sqlite::SqliteMigrator] or [mysql::MysqlMigrator] instance, you can access any feature this library provides.
//!
//! # Features:
//! - Easy adoption from other migration tools, or no migration tool.
//! - Robust error handling and rollback support (when available).
//! - Preview / dry-run support.
//! - Migration history querying.
//! - Observability hooks.
//! - Tracing integration.
//! - Feature-rich testing utilities.
//!
//! Database support:
//! - [`SQLite`](sqlite)
//! - [`MySQL`](mysql)

mod core;
pub use core::{AppliedMigration, Migration, MigrationFailure, MigrationReport, Precondition};

mod error;
pub use error::Error;

#[cfg(feature = "sqlite")]
#[cfg_attr(docsrs, doc(cfg(feature = "sqlite")))]
pub mod sqlite;

#[cfg(feature = "mysql")]
pub mod mysql;

#[cfg(feature = "testing")]
pub mod testing;

#[cfg(all(test, feature = "mysql"))]
pub(crate) mod test_mysql;
