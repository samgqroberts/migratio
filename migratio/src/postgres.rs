//!
//! # PostgreSQL migration support
//!
//! This module provides PostgreSQL migration support using the [`postgres`](https://crates.io/crates/postgres) crate.
//!
//! ## Transaction Safety
//!
//! PostgreSQL fully supports transactional DDL (unlike MySQL). Each migration runs within
//! its own transaction. If a migration fails (returns an error or panics), the transaction
//! is automatically rolled back, leaving the database in the state it was in after the
//! last successful migration.
//!
//! ## Comparison with SQLite and MySQL
//!
//! | Behavior | SQLite | MySQL | PostgreSQL |
//! |----------|--------|-------|------------|
//! | DDL in transactions | Fully supported | Causes implicit commit | Fully supported |
//! | Migration failure | Complete rollback | Partial DDL may persist | Complete rollback |
//! | Method parameter | `&Transaction` | `&mut Conn` | `&mut Transaction` |
//! | Automatic cleanup | Yes | Manual intervention may be needed | Yes |
//!
//! ## Exceptions
//!
//! The following operations cannot be rolled back even in PostgreSQL:
//! - `CREATE DATABASE` / `DROP DATABASE`
//! - `CREATE TABLESPACE` / `DROP TABLESPACE`
//!
//! Avoid these operations in migrations if possible.
//!
//! ## Example
//!
//! ```ignore
//! use migratio::{Migration, MigrationReport, Error};
//! use migratio::postgres::PostgresMigrator;
//! use postgres::{Client, NoTls, Transaction};
//!
//! // Define your migrations as structs that implement the Migration trait
//! struct Migration1;
//! impl Migration for Migration1 {
//!     fn version(&self) -> u32 {
//!         1
//!     }
//!     fn postgres_up(&self, tx: &mut Transaction) -> Result<(), Error> {
//!         tx.execute("CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT)", &[])?;
//!         Ok(())
//!     }
//! }
//!
//! struct Migration2;
//! impl Migration for Migration2 {
//!     fn version(&self) -> u32 {
//!         2
//!     }
//!     fn postgres_up(&self, tx: &mut Transaction) -> Result<(), Error> {
//!         tx.execute("ALTER TABLE users ADD COLUMN email TEXT", &[])?;
//!         Ok(())
//!     }
//! }
//!
//! // Construct a migrator with migrations
//! let migrator = PostgresMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
//!
//! // Connect to your database and run the migrations
//! let mut client = Client::connect("postgres://user:password@localhost/mydb", NoTls).unwrap();
//! let report = migrator.upgrade(&mut client).unwrap();
//! ```

use crate::core::GenericMigrator;
use crate::error::Error;
use crate::AppliedMigration;
use crate::Migration;
use crate::MigrationFailure;
use crate::MigrationReport;
use crate::Precondition;
use chrono::Utc;
use postgres::Client;
use std::time::Instant;

// Re-export postgres types for use in migrations
pub use postgres::Client as PostgresClient;
pub use postgres::Transaction as PostgresTransaction;

/// The entrypoint for running a sequence of [Migration]s on a PostgreSQL database.
/// Construct this struct with the list of all [Migration]s to be applied.
/// [Migration::version]s must be contiguous, greater than zero, and unique.
///
/// ## Transaction Safety
///
/// Each migration runs within its own PostgreSQL transaction. If a migration fails,
/// all changes from that migration are automatically rolled back, leaving the database
/// in a consistent state.
#[derive(Debug)]
pub struct PostgresMigrator {
    migrator: GenericMigrator,
}

impl PostgresMigrator {
    /// Create a new PostgresMigrator, validating migration invariants.
    /// Returns an error if migrations are invalid.
    pub fn try_new(migrations: Vec<Box<dyn Migration>>) -> Result<Self, String> {
        Ok(Self {
            migrator: GenericMigrator::try_new(migrations)?,
        })
    }

    /// Create a new PostgresMigrator, panicking if migration metadata is invalid.
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
        self.migrator.set_schema_version_table_name(name);
        self
    }

    /// Set a callback to be invoked when a migration starts.
    /// The callback receives the migration version and name.
    pub fn on_migration_start<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str) + Send + Sync + 'static,
    {
        self.migrator.set_on_migration_start(callback);
        self
    }

    /// Set a callback to be invoked when a migration completes successfully.
    /// The callback receives the migration version, name, and duration.
    pub fn on_migration_complete<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str, std::time::Duration) + Send + Sync + 'static,
    {
        self.migrator.set_on_migration_complete(callback);
        self
    }

    /// Set a callback to be invoked when a migration is skipped because its precondition
    /// returned [`Precondition::AlreadySatisfied`].
    /// The callback receives the migration version and name.
    pub fn on_migration_skipped<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str) + Send + Sync + 'static,
    {
        self.migrator.set_on_migration_skipped(callback);
        self
    }

    /// Set a callback to be invoked when a migration fails.
    /// The callback receives the migration version, name, and error.
    pub fn on_migration_error<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str, &Error) + Send + Sync + 'static,
    {
        self.migrator.set_on_migration_error(callback);
        self
    }

    /// Get a reference to all migrations in this migrator.
    pub fn migrations(&self) -> &[Box<dyn Migration>] {
        &self.migrator.migrations
    }

    pub fn schema_version_table_name(&self) -> &str {
        &self.migrator.schema_version_table_name
    }

    /// Get the current migration version from the database.
    /// Returns 0 if no migrations have been applied.
    pub fn get_current_version(&self, client: &mut Client) -> Result<u32, Error> {
        // Check if schema version table exists
        let table_exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = $1)",
                &[&self.schema_version_table_name()],
            )?
            .get(0);

        if !table_exists {
            return Ok(0);
        }

        // Get current version (highest version number)
        let row = client.query_one(
            &format!(
                "SELECT COALESCE(MAX(version), 0) FROM {}",
                self.schema_version_table_name()
            ),
            &[],
        )?;
        let version: i32 = row.get(0);
        Ok(version as u32)
    }

    /// Get the history of all migrations that have been applied to the database.
    /// Returns migrations in the order they were applied (by version number).
    /// Returns an empty vector if no migrations have been applied.
    pub fn get_migration_history(
        &self,
        client: &mut Client,
    ) -> Result<Vec<AppliedMigration>, Error> {
        // Check if schema version table exists
        let table_exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = $1)",
                &[&self.schema_version_table_name()],
            )?
            .get(0);

        if !table_exists {
            return Ok(vec![]);
        }

        // Query all applied migrations, ordered by version
        let rows = client.query(
            &format!(
                "SELECT version, name, applied_at, checksum FROM {} ORDER BY version",
                self.schema_version_table_name()
            ),
            &[],
        )?;

        let migrations: Result<Vec<AppliedMigration>, Error> = rows
            .into_iter()
            .map(|row| {
                let version: i32 = row.get(0);
                let name: String = row.get(1);
                let applied_at_str: String = row.get(2);
                let checksum: String = row.get(3);

                let applied_at = chrono::DateTime::parse_from_rfc3339(&applied_at_str)
                    .map_err(|e| Error::Generic(format!("Failed to parse datetime: {}", e)))?
                    .with_timezone(&Utc);

                Ok(AppliedMigration {
                    version: version as u32,
                    name,
                    applied_at,
                    checksum,
                })
            })
            .collect();

        migrations
    }

    /// Preview which migrations would be applied by `upgrade()` without actually running them.
    /// Returns a list of migrations that would be executed, in the order they would run.
    pub fn preview_upgrade(&self, client: &mut Client) -> Result<Vec<&Box<dyn Migration>>, Error> {
        let current_version = self.get_current_version(client)?;

        let mut pending_migrations = self
            .migrations()
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
        client: &mut Client,
        target_version: u32,
    ) -> Result<Vec<&Box<dyn Migration>>, Error> {
        let current_version = self.get_current_version(client)?;

        // Validate target version
        if target_version > current_version {
            return Err(Error::Generic(format!(
                "Cannot downgrade to version {} when current version is {}. Target must be <= current version.",
                target_version, current_version
            )));
        }

        let mut migrations_to_rollback = self
            .migrations()
            .iter()
            .filter(|m| m.version() > target_version && m.version() <= current_version)
            .collect::<Vec<_>>();
        migrations_to_rollback.sort_by_key(|m| std::cmp::Reverse(m.version())); // Reverse order

        Ok(migrations_to_rollback)
    }

    /// Upgrade the database to a specific target version.
    ///
    /// This runs all pending migrations up to and including the target version.
    /// If the database is already at or beyond the target version, no migrations are run.
    pub fn upgrade_to(
        &self,
        client: &mut Client,
        target_version: u32,
    ) -> Result<MigrationReport<'_>, Error> {
        // Validate target version exists
        if target_version > 0
            && !self
                .migrations()
                .iter()
                .any(|m| m.version() == target_version)
        {
            return Err(Error::Generic(format!(
                "Target version {} does not exist in migration list",
                target_version
            )));
        }

        self.upgrade_internal(client, Some(target_version))
    }

    /// Upgrade the database by running all pending migrations.
    pub fn upgrade(&self, client: &mut Client) -> Result<MigrationReport<'_>, Error> {
        self.upgrade_internal(client, None)
    }

    fn upgrade_internal(
        &self,
        client: &mut Client,
        target_version: Option<u32>,
    ) -> Result<MigrationReport<'_>, Error> {
        // Check if schema version tracking table exists
        let schema_version_table_existed: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = $1)",
                &[&self.schema_version_table_name()],
            )?
            .get(0);

        if !schema_version_table_existed {
            #[cfg(feature = "tracing")]
            tracing::info!(
                "Creating migration tracking table: {}",
                self.schema_version_table_name()
            );

            // Create table with name and checksum columns
            client.execute(
                &format!(
                    "CREATE TABLE IF NOT EXISTS {} (
                        version INTEGER PRIMARY KEY NOT NULL,
                        name TEXT NOT NULL,
                        applied_at TEXT NOT NULL,
                        checksum TEXT NOT NULL
                    )",
                    self.schema_version_table_name()
                ),
                &[],
            )?;
        }

        // Validate checksums of previously-applied migrations
        if schema_version_table_existed {
            // Check if the checksum column exists (for backwards compatibility)
            let has_checksum_column: bool = client
                .query_one(
                    "SELECT EXISTS (SELECT FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1 AND column_name = 'checksum')",
                    &[&self.schema_version_table_name()],
                )?
                .get(0);

            if has_checksum_column {
                // Get all applied migrations with their checksums
                let applied_migrations = client.query(
                    &format!(
                        "SELECT version, name, checksum FROM {}",
                        self.schema_version_table_name()
                    ),
                    &[],
                )?;

                // Verify checksums match for migrations that were already applied
                // Also detect missing migrations (in DB but not in code)
                for row in &applied_migrations {
                    let applied_version: i32 = row.get(0);
                    let applied_name: String = row.get(1);
                    let applied_checksum: String = row.get(2);

                    if let Some(migration) = self
                        .migrations()
                        .iter()
                        .find(|m| m.version() == applied_version as u32)
                    {
                        let current_checksum = GenericMigrator::calculate_checksum(migration);
                        if current_checksum != applied_checksum {
                            return Err(Error::Generic(format!(
                                "Migration {} checksum mismatch. Expected '{}' but found '{}'. \
                                Migration name in DB: '{}', current name: '{}'. \
                                This indicates the migration was modified after being applied.",
                                applied_version,
                                applied_checksum,
                                current_checksum,
                                applied_name,
                                migration.name()
                            )));
                        }
                    } else {
                        // Migration exists in database but not in code - this is a missing migration
                        return Err(Error::Generic(format!(
                            "Migration {} ('{}') was previously applied but is no longer present in the migration list. \
                            Applied migrations cannot be removed from the codebase.",
                            applied_version,
                            applied_name
                        )));
                    }
                }

                // Detect orphaned migrations (in code but applied migrations are not contiguous)
                let applied_versions: Vec<i32> =
                    applied_migrations.iter().map(|row| row.get(0)).collect();
                if !applied_versions.is_empty() {
                    let max_applied = *applied_versions.iter().max().unwrap();

                    // Check if all migrations up to max_applied exist in the database
                    for expected_version in 1..=max_applied {
                        if !applied_versions.contains(&expected_version) {
                            // There's a gap - check if this migration exists in our code
                            if let Some(missing_migration) = self
                                .migrations()
                                .iter()
                                .find(|m| m.version() == expected_version as u32)
                            {
                                return Err(Error::Generic(format!(
                                    "Migration {} ('{}') exists in code but was not applied, yet later migrations are already applied. \
                                    This likely means migration {} was added after migration {} was already applied. \
                                    Applied migrations: {:?}",
                                    expected_version,
                                    missing_migration.name(),
                                    expected_version,
                                    max_applied,
                                    applied_versions
                                )));
                            }
                        }
                    }
                }
            }
        }

        // Get current migration version (highest version number)
        let current_version = self.get_current_version(client)?;

        // Iterate through migrations, run those that haven't been run
        let mut migrations_run: Vec<u32> = Vec::new();
        let mut migrations_sorted = self
            .migrations()
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
                    "postgres_migration_up",
                    version = migration_version,
                    name = %migration.name()
                )
                .entered();

                #[cfg(feature = "tracing")]
                tracing::info!("Starting migration");

                // Call on_migration_start hook
                if let Some(ref callback) = self.migrator.on_migration_start {
                    callback(migration_version, &migration.name());
                }

                let migration_start = Instant::now();

                // Start a transaction for this migration
                let mut tx = client.transaction()?;

                // Check precondition
                let precondition = match migration.postgres_precondition(&mut tx) {
                    Ok(p) => p,
                    Err(error) => {
                        #[cfg(feature = "tracing")]
                        tracing::error!(
                            error = %error,
                            "Precondition check failed"
                        );

                        // Call on_migration_error hook
                        if let Some(ref callback) = self.migrator.on_migration_error {
                            callback(migration_version, &migration.name(), &error);
                        }

                        failing_migration = Some(MigrationFailure { migration, error });
                        break;
                    }
                };

                // Run migration or stamp if precondition is satisfied
                let migration_result = match precondition {
                    Precondition::AlreadySatisfied => {
                        #[cfg(feature = "tracing")]
                        tracing::info!("Precondition already satisfied, stamping migration without running up()");

                        // Call on_migration_skipped hook
                        if let Some(ref callback) = self.migrator.on_migration_skipped {
                            callback(migration_version, &migration.name());
                        }

                        Ok(())
                    }
                    Precondition::NeedsApply => {
                        // Run the migration within the transaction
                        migration.postgres_up(&mut tx)
                    }
                };

                match migration_result {
                    Ok(_) => {
                        // Calculate checksum for this migration
                        let checksum = GenericMigrator::calculate_checksum(migration);

                        // Insert a row for this migration with its name, timestamp, and checksum
                        tx.execute(
                            &format!(
                                "INSERT INTO {} (version, name, applied_at, checksum) VALUES($1, $2, $3, $4)",
                                self.schema_version_table_name()
                            ),
                            &[&(migration_version as i32), &migration.name(), &batch_applied_at, &checksum],
                        )?;

                        // Commit the transaction
                        tx.commit()?;

                        let migration_duration = migration_start.elapsed();

                        #[cfg(feature = "tracing")]
                        tracing::info!(
                            duration_ms = migration_duration.as_millis(),
                            "Migration completed successfully"
                        );

                        // Record migration as run
                        migrations_run.push(migration_version);
                        // Also, since any migration succeeded, if schema version table had not originally existed,
                        // we can mark that it was created
                        schema_version_table_created = true;

                        // Call on_migration_complete hook
                        if let Some(ref callback) = self.migrator.on_migration_complete {
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
                        if let Some(ref callback) = self.migrator.on_migration_error {
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

        // Return report
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
    pub fn downgrade(
        &self,
        client: &mut Client,
        target_version: u32,
    ) -> Result<MigrationReport<'_>, Error> {
        // Check if schema version table exists
        let schema_version_table_existed: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = $1)",
                &[&self.schema_version_table_name()],
            )?
            .get(0);

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
        let has_checksum_column: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1 AND column_name = 'checksum')",
                &[&self.schema_version_table_name()],
            )?
            .get(0);

        if has_checksum_column {
            let applied_migrations = client.query(
                &format!(
                    "SELECT version, name, checksum FROM {}",
                    self.schema_version_table_name()
                ),
                &[],
            )?;

            // Verify checksums and detect missing migrations
            for row in &applied_migrations {
                let applied_version: i32 = row.get(0);
                let applied_name: String = row.get(1);
                let applied_checksum: String = row.get(2);

                if let Some(migration) = self
                    .migrations()
                    .iter()
                    .find(|m| m.version() == applied_version as u32)
                {
                    let current_checksum = GenericMigrator::calculate_checksum(migration);
                    if current_checksum != applied_checksum {
                        return Err(Error::Generic(format!(
                            "Migration {} checksum mismatch. Expected '{}' but found '{}'. \
                            Migration name in DB: '{}', current name: '{}'. \
                            This indicates the migration was modified after being applied.",
                            applied_version,
                            applied_checksum,
                            current_checksum,
                            applied_name,
                            migration.name()
                        )));
                    }
                } else {
                    // Migration exists in database but not in code
                    return Err(Error::Generic(format!(
                        "Migration {} ('{}') was previously applied but is no longer present in the migration list. \
                        Applied migrations cannot be removed from the codebase.",
                        applied_version,
                        applied_name
                    )));
                }
            }

            // Detect orphaned migrations (gaps in applied migrations with code present)
            let applied_versions: Vec<i32> =
                applied_migrations.iter().map(|row| row.get(0)).collect();
            if !applied_versions.is_empty() {
                let max_applied = *applied_versions.iter().max().unwrap();

                for expected_version in 1..=max_applied {
                    if !applied_versions.contains(&expected_version) {
                        if let Some(missing_migration) = self
                            .migrations()
                            .iter()
                            .find(|m| m.version() == expected_version as u32)
                        {
                            return Err(Error::Generic(format!(
                                "Migration {} ('{}') exists in code but was not applied, yet later migrations are already applied. \
                                This likely means migration {} was added after migration {} was already applied. \
                                Applied migrations: {:?}",
                                expected_version,
                                missing_migration.name(),
                                expected_version,
                                max_applied,
                                applied_versions
                            )));
                        }
                    }
                }
            }
        }

        // Get current version
        let current_version = self.get_current_version(client)?;

        // Validate target version
        if target_version > current_version {
            return Err(Error::Generic(format!(
                "Cannot downgrade to version {} when current version is {}. Target must be <= current version.",
                target_version, current_version
            )));
        }

        // Get migrations to rollback (in reverse order)
        let mut migrations_to_rollback = self
            .migrations()
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
                "postgres_migration_down",
                version = migration_version,
                name = %migration.name()
            )
            .entered();

            #[cfg(feature = "tracing")]
            tracing::info!("Rolling back migration");

            // Call on_migration_start hook
            if let Some(ref callback) = self.migrator.on_migration_start {
                callback(migration_version, &migration.name());
            }

            let migration_start = Instant::now();

            // Start a transaction for this migration rollback
            let mut tx = client.transaction()?;

            // Run the downgrade within the transaction
            let migration_result = migration.postgres_down(&mut tx);

            match migration_result {
                Ok(_) => {
                    // Delete this migration from the tracking table
                    tx.execute(
                        &format!(
                            "DELETE FROM {} WHERE version = $1",
                            self.schema_version_table_name()
                        ),
                        &[&(migration_version as i32)],
                    )?;

                    // Commit the transaction
                    tx.commit()?;

                    let migration_duration = migration_start.elapsed();

                    #[cfg(feature = "tracing")]
                    tracing::info!(
                        duration_ms = migration_duration.as_millis(),
                        "Migration rolled back successfully"
                    );

                    // Record migration as rolled back
                    migrations_run.push(migration_version);

                    // Call on_migration_complete hook
                    if let Some(ref callback) = self.migrator.on_migration_complete {
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
                    if let Some(ref callback) = self.migrator.on_migration_error {
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
    use super::*;
    use crate::test_postgres::get_test_client;

    #[test]
    fn single_successful_from_clean() {
        use chrono::{DateTime, FixedOffset};

        let mut client = get_test_client();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE test (id SERIAL PRIMARY KEY)", &[])?;
                Ok(())
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

        let migrator = PostgresMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut client).unwrap();

        assert_eq!(
            report,
            MigrationReport {
                schema_version_table_existed: false,
                schema_version_table_created: true,
                migrations_run: vec![1],
                failing_migration: None,
            }
        );

        // Verify schema version table exists and has recorded version 1
        let rows = client
            .query(
                "SELECT version, name, applied_at FROM _migratio_version_",
                &[],
            )
            .unwrap();

        assert_eq!(rows.len(), 1);
        let version: i32 = rows[0].get(0);
        let name: String = rows[0].get(1);
        let applied_at_str: String = rows[0].get(2);

        assert_eq!(version, 1);
        assert_eq!(name, "Migration 1"); // default name

        let date = DateTime::parse_from_rfc3339(&applied_at_str).unwrap();
        assert_eq!(date.timezone(), FixedOffset::east_opt(0).unwrap());

        // Ensure that the date is within 5 seconds of now
        let now = Utc::now();
        let diff = now.timestamp() - date.timestamp();
        assert!(diff < 5);
    }

    #[test]
    fn single_unsuccessful_from_clean_with_rollback() {
        // This test verifies PostgreSQL's transactional DDL - the table should NOT exist after failure
        let mut client = get_test_client();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                // Create table first
                tx.execute("CREATE TABLE test (id SERIAL PRIMARY KEY, value INT)", &[])?;
                // Then do something that fails
                tx.execute("THIS IS NOT VALID SQL", &[])?;
                Ok(())
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

        let migrator = PostgresMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut client).unwrap();

        // Verify migration failed
        assert_eq!(report.migrations_run, Vec::<u32>::new());
        assert!(report.failing_migration.is_some());

        // CRITICAL: Verify table was rolled back (unlike MySQL where it would persist)
        let table_exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'test')",
                &[],
            )
            .unwrap()
            .get(0);
        assert!(
            !table_exists,
            "Table should NOT exist due to transaction rollback"
        );
    }

    #[test]
    fn upgrade_to_specific_version() {
        let mut client = get_test_client();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE users (id SERIAL PRIMARY KEY)", &[])?;
                Ok(())
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

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE posts (id SERIAL PRIMARY KEY)", &[])?;
                Ok(())
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

        struct Migration3;
        impl Migration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE comments (id SERIAL PRIMARY KEY)", &[])?;
                Ok(())
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

        let migrator = PostgresMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);

        // Upgrade to version 2
        let report = migrator.upgrade_to(&mut client, 2).unwrap();
        assert_eq!(report.migrations_run, vec![1, 2]);

        // Verify only migrations 1 and 2 ran
        let version = migrator.get_current_version(&mut client).unwrap();
        assert_eq!(version, 2);

        // Verify users and posts tables exist
        let users_exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'users')",
                &[],
            )
            .unwrap()
            .get(0);
        assert!(users_exists);

        let posts_exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'posts')",
                &[],
            )
            .unwrap()
            .get(0);
        assert!(posts_exists);

        // Verify comments table does not exist
        let comments_exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'comments')",
                &[],
            )
            .unwrap()
            .get(0);
        assert!(!comments_exists);
    }

    #[test]
    fn success_then_failure_from_clean() {
        let mut client = get_test_client();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE users (id SERIAL PRIMARY KEY)", &[])?;
                tx.execute("INSERT INTO users DEFAULT VALUES", &[])?;
                tx.execute("INSERT INTO users DEFAULT VALUES", &[])?;
                Ok(())
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

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                tx.execute("INVALID SQL", &[])?;
                Ok(())
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

        let migrator = PostgresMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut client).unwrap();

        assert_eq!(report.migrations_run, vec![1]);
        assert!(report.failing_migration.is_some());

        // Verify users table exists with data (from successful migration 1)
        let count: i64 = client
            .query_one("SELECT COUNT(*) FROM users", &[])
            .unwrap()
            .get(0);
        assert_eq!(count, 2);
    }

    #[test]
    #[should_panic(expected = "Migration version must be greater than 0")]
    fn new_rejects_zero_version() {
        struct Migration0;
        impl Migration for Migration0 {
            fn version(&self) -> u32 {
                0
            }
            fn postgres_up(&self, _tx: &mut postgres::Transaction) -> Result<(), Error> {
                Ok(())
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
        PostgresMigrator::new(vec![Box::new(Migration0)]);
    }

    #[test]
    #[should_panic(expected = "Duplicate migration version found: 2")]
    fn new_rejects_duplicate_versions() {
        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn postgres_up(&self, _tx: &mut postgres::Transaction) -> Result<(), Error> {
                Ok(())
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

        struct Migration2a;
        impl Migration for Migration2a {
            fn version(&self) -> u32 {
                2
            }
            fn postgres_up(&self, _tx: &mut postgres::Transaction) -> Result<(), Error> {
                Ok(())
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

        struct Migration2b;
        impl Migration for Migration2b {
            fn version(&self) -> u32 {
                2
            }
            fn postgres_up(&self, _tx: &mut postgres::Transaction) -> Result<(), Error> {
                Ok(())
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

        PostgresMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2a),
            Box::new(Migration2b),
        ]);
    }

    #[test]
    fn downgrade_works() {
        let mut client = get_test_client();

        struct Migration1;
        impl Migration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE users (id SERIAL PRIMARY KEY)", &[])?;
                Ok(())
            }
            fn postgres_down(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE users", &[])?;
                Ok(())
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

        struct Migration2;
        impl Migration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                tx.execute("CREATE TABLE posts (id SERIAL PRIMARY KEY)", &[])?;
                Ok(())
            }
            fn postgres_down(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
                tx.execute("DROP TABLE posts", &[])?;
                Ok(())
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

        let migrator = PostgresMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);

        // Upgrade to version 2
        migrator.upgrade(&mut client).unwrap();
        assert_eq!(migrator.get_current_version(&mut client).unwrap(), 2);

        // Downgrade to version 1
        let report = migrator.downgrade(&mut client, 1).unwrap();
        assert_eq!(report.migrations_run, vec![2]);
        assert_eq!(migrator.get_current_version(&mut client).unwrap(), 1);

        // Verify posts table no longer exists
        let posts_exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'posts')",
                &[],
            )
            .unwrap()
            .get(0);
        assert!(!posts_exists);

        // Verify users table still exists
        let users_exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'users')",
                &[],
            )
            .unwrap()
            .get(0);
        assert!(users_exists);

        // Downgrade to version 0
        let report = migrator.downgrade(&mut client, 0).unwrap();
        assert_eq!(report.migrations_run, vec![1]);
        assert_eq!(migrator.get_current_version(&mut client).unwrap(), 0);

        // Verify users table no longer exists
        let users_exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'users')",
                &[],
            )
            .unwrap()
            .get(0);
        assert!(!users_exists);
    }
}
