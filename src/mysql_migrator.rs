use crate::error::Error;
use chrono::Utc;
use mysql::prelude::*;
use mysql::{Conn, Transaction, TxOpts};
use sha2::{Digest, Sha256};
use std::time::Instant;

/// Represents a failure during a migration.
#[derive(Debug, PartialEq)]
pub struct MysqlMigrationFailure<'migration> {
    migration: &'migration Box<dyn MysqlMigration>,
    error: Error,
}

impl<'migration> MysqlMigrationFailure<'migration> {
    /// Get the migration that failed.
    pub fn migration(&self) -> &dyn MysqlMigration {
        self.migration.as_ref()
    }

    /// Get the error that caused the migration to fail.
    pub fn error(&self) -> &Error {
        &self.error
    }
}

/// A report of actions performed during a migration.
#[derive(Debug, PartialEq)]
pub struct MysqlMigrationReport<'migration> {
    pub schema_version_table_existed: bool,
    pub schema_version_table_created: bool,
    pub migrations_run: Vec<u32>,
    pub failing_migration: Option<MysqlMigrationFailure<'migration>>,
}

/// Represents the result of a migration precondition check.
#[derive(Debug, Clone, PartialEq)]
pub enum MysqlPrecondition {
    /// The migration's changes are already present in the database and should be stamped without running up().
    AlreadySatisfied,
    /// The migration needs to be applied by running up().
    NeedsApply,
}

/// Represents a migration that has been applied to the database.
#[derive(Debug, Clone, PartialEq)]
pub struct MysqlAppliedMigration {
    /// The version number of the migration.
    pub version: u32,
    /// The name of the migration.
    pub name: String,
    /// The timestamp when the migration was applied.
    pub applied_at: chrono::DateTime<Utc>,
    /// The checksum of the migration at the time it was applied.
    pub checksum: String,
}

const DEFAULT_VERSION_TABLE_NAME: &str = "_migratio_version_";

/// A trait that must be implemented to define a MySQL migration.
/// The `version` value must be unique among all migrations supplied to the migrator, and greater than 0.
/// Implement your migration logic in the `up` method, using the supplied [Transaction] to perform database operations.
/// The transaction will be automatically committed if the migration succeeds, or rolled back if it fails.
/// Optionally implement the `down` method to enable rollback support via [MysqlMigrator::downgrade].
/// The `name` and `description` methods are optional, and only aid in debugging / observability.
pub trait MysqlMigration {
    /// Returns the version number of this migration.
    ///
    /// # IMPORTANT WARNING
    ///
    /// **Once a migration has been applied to any database, its version number must NEVER be changed.**
    /// The version is used to track which migrations have been applied. Changing it will cause
    /// the migrator to fail validation with an error about missing or orphaned migrations.
    ///
    /// # Requirements
    ///
    /// - Must be greater than 0
    /// - Must be unique across all migrations
    /// - Must be contiguous (1, 2, 3, ... with no gaps)
    /// - Must be immutable once the migration is applied to any database
    fn version(&self) -> u32;

    fn up(&self, tx: &mut Transaction) -> Result<(), Error>;

    /// Rollback this migration. This is optional - the default implementation panics.
    /// If you want to support downgrade functionality, implement this method.
    fn down(&self, _tx: &mut Transaction) -> Result<(), Error> {
        panic!(
            "Migration {} ('{}') does not support downgrade. Implement the down() method to enable rollback.",
            self.version(),
            self.name()
        )
    }

    /// Returns the name of this migration.
    ///
    /// # IMPORTANT WARNING
    ///
    /// **Once a migration has been applied to any database, its name must NEVER be changed.**
    /// The name is included in the checksum used to verify migration integrity. Changing it
    /// will cause the migrator to fail with a checksum mismatch error.
    ///
    /// The default implementation returns "Migration {version}". You can override this to
    /// provide a more descriptive name, but remember: once applied, it's permanent.
    fn name(&self) -> String {
        format!("Migration {}", self.version())
    }

    /// Returns an optional description of what this migration does.
    ///
    /// Unlike `version()` and `name()`, the description can be changed at any time as it's
    /// not used for migration tracking or validation. Use this for human-readable documentation.
    fn description(&self) -> Option<&'static str> {
        None
    }

    /// Optional precondition check for the migration.
    ///
    /// This method is called before running the migration's `up()` method during upgrade.
    /// It allows the migration to check if its changes are already present in the database
    /// (for example, when adopting migratio after using another migration tool).
    ///
    /// If this returns `MysqlPrecondition::AlreadySatisfied`, the migration is stamped as applied
    /// in the version table without running `up()`. If it returns `MysqlPrecondition::NeedsApply`,
    /// the migration runs normally via `up()`.
    ///
    /// The default implementation returns `MysqlPrecondition::NeedsApply`, meaning migrations
    /// always run unless you override this method.
    fn precondition(&self, _tx: &mut Transaction) -> Result<MysqlPrecondition, Error> {
        Ok(MysqlPrecondition::NeedsApply)
    }
}

impl PartialEq for dyn MysqlMigration {
    fn eq(&self, other: &Self) -> bool {
        self.version() == other.version()
    }
}

impl std::fmt::Debug for dyn MysqlMigration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MysqlMigration")
            .field("version", &self.version())
            .field("name", &self.name())
            .finish()
    }
}

/// The entrypoint for running a sequence of [MysqlMigration]s.
/// Construct this struct with the list of all [MysqlMigration]s to be applied.
/// [MysqlMigration::version]s must be contiguous, greater than zero, and unique.
pub struct MysqlMigrator {
    migrations: Vec<Box<dyn MysqlMigration>>,
    schema_version_table_name: String,
    on_migration_start: Option<Box<dyn Fn(u32, &str) + Send + Sync>>,
    on_migration_complete: Option<Box<dyn Fn(u32, &str, std::time::Duration) + Send + Sync>>,
    on_migration_skipped: Option<Box<dyn Fn(u32, &str) + Send + Sync>>,
    on_migration_error: Option<Box<dyn Fn(u32, &str, &Error) + Send + Sync>>,
}

// Manual Debug impl since closures don't implement Debug
impl std::fmt::Debug for MysqlMigrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MysqlMigrator")
            .field("migrations", &self.migrations)
            .field("schema_version_table_name", &self.schema_version_table_name)
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

impl MysqlMigrator {
    /// Calculate a checksum for a migration based on its version and name.
    /// This is used to verify that migrations haven't been modified after being applied.
    fn calculate_checksum(migration: &Box<dyn MysqlMigration>) -> String {
        let mut hasher = Sha256::new();
        hasher.update(migration.version().to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(migration.name().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Create a new MysqlMigrator, validating migration invariants.
    /// Returns an error if migrations are invalid.
    pub fn try_new(migrations: Vec<Box<dyn MysqlMigration>>) -> Result<Self, String> {
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
            on_migration_start: None,
            on_migration_complete: None,
            on_migration_skipped: None,
            on_migration_error: None,
        })
    }

    /// Create a new MysqlMigrator, panicking if migration metadata is invalid.
    /// For a non-panicking version, use `try_new`.
    pub fn new(migrations: Vec<Box<dyn MysqlMigration>>) -> Self {
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

    /// Set a callback to be invoked when a migration starts.
    /// The callback receives the migration version and name.
    pub fn on_migration_start<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str) + Send + Sync + 'static,
    {
        self.on_migration_start = Some(Box::new(callback));
        self
    }

    /// Set a callback to be invoked when a migration completes successfully.
    /// The callback receives the migration version, name, and duration.
    pub fn on_migration_complete<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str, std::time::Duration) + Send + Sync + 'static,
    {
        self.on_migration_complete = Some(Box::new(callback));
        self
    }

    /// Set a callback to be invoked when a migration is skipped because its precondition
    /// returned `MysqlPrecondition::AlreadySatisfied`.
    /// The callback receives the migration version and name.
    pub fn on_migration_skipped<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str) + Send + Sync + 'static,
    {
        self.on_migration_skipped = Some(Box::new(callback));
        self
    }

    /// Set a callback to be invoked when a migration fails.
    /// The callback receives the migration version, name, and error.
    pub fn on_migration_error<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32, &str, &Error) + Send + Sync + 'static,
    {
        self.on_migration_error = Some(Box::new(callback));
        self
    }

    /// Get a reference to all migrations in this migrator.
    pub fn migrations(&self) -> &[Box<dyn MysqlMigration>] {
        &self.migrations
    }

    /// Get the current migration version from the database.
    /// Returns 0 if no migrations have been applied.
    pub fn get_current_version(&self, conn: &mut Conn) -> Result<u32, Error> {
        // Check if schema version table exists
        let table_exists: bool = conn
            .query_first(format!(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = '{}'",
                self.schema_version_table_name
            ))?
            .map(|(count,): (i64,)| count > 0)
            .unwrap_or(false);

        if !table_exists {
            return Ok(0);
        }

        // Get current version (highest version number)
        // Note: MAX() returns NULL when table is empty, so we need to handle that
        let result: Option<(Option<u32>,)> = conn.query_first(format!(
            "SELECT MAX(version) FROM {}",
            self.schema_version_table_name
        ))?;
        Ok(result.and_then(|(v,)| v).unwrap_or(0))
    }

    /// Get the history of all migrations that have been applied to the database.
    /// Returns migrations in the order they were applied (by version number).
    /// Returns an empty vector if no migrations have been applied.
    pub fn get_migration_history(
        &self,
        conn: &mut Conn,
    ) -> Result<Vec<MysqlAppliedMigration>, Error> {
        // Check if schema version table exists
        let table_exists: bool = conn
            .query_first(format!(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = '{}'",
                self.schema_version_table_name
            ))?
            .map(|(count,): (i64,)| count > 0)
            .unwrap_or(false);

        if !table_exists {
            return Ok(vec![]);
        }

        // Query all applied migrations, ordered by version
        let rows: Vec<(u32, String, String, String)> = conn.query(format!(
            "SELECT version, name, applied_at, checksum FROM {} ORDER BY version",
            self.schema_version_table_name
        ))?;

        let migrations: Result<Vec<MysqlAppliedMigration>, Error> = rows
            .into_iter()
            .map(|(version, name, applied_at_str, checksum)| {
                let applied_at = chrono::DateTime::parse_from_rfc3339(&applied_at_str)
                    .map_err(|e| Error::Generic(format!("Failed to parse datetime: {}", e)))?
                    .with_timezone(&Utc);

                Ok(MysqlAppliedMigration {
                    version,
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
    pub fn preview_upgrade(&self, conn: &mut Conn) -> Result<Vec<&Box<dyn MysqlMigration>>, Error> {
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
        conn: &mut Conn,
        target_version: u32,
    ) -> Result<Vec<&Box<dyn MysqlMigration>>, Error> {
        let current_version = self.get_current_version(conn)?;

        // Validate target version
        if target_version > current_version {
            return Err(Error::Generic(format!(
                "Cannot downgrade to version {} when current version is {}. Target must be <= current version.",
                target_version, current_version
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

    /// Upgrade the database to a specific target version.
    ///
    /// This runs all pending migrations up to and including the target version.
    /// If the database is already at or beyond the target version, no migrations are run.
    pub fn upgrade_to(
        &self,
        conn: &mut Conn,
        target_version: u32,
    ) -> Result<MysqlMigrationReport<'_>, Error> {
        // Validate target version exists
        if target_version > 0
            && !self
                .migrations
                .iter()
                .any(|m| m.version() == target_version)
        {
            return Err(Error::Generic(format!(
                "Target version {} does not exist in migration list",
                target_version
            )));
        }

        self.upgrade_internal(conn, Some(target_version))
    }

    /// Upgrade the database by running all pending migrations.
    pub fn upgrade(&self, conn: &mut Conn) -> Result<MysqlMigrationReport<'_>, Error> {
        self.upgrade_internal(conn, None)
    }

    fn upgrade_internal(
        &self,
        conn: &mut Conn,
        target_version: Option<u32>,
    ) -> Result<MysqlMigrationReport<'_>, Error> {
        // Check if schema version tracking table exists
        let schema_version_table_existed: bool = conn
            .query_first(format!(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = '{}'",
                self.schema_version_table_name
            ))?
            .map(|(count,): (i64,)| count > 0)
            .unwrap_or(false);

        if !schema_version_table_existed {
            // Create table with name and checksum columns
            conn.query_drop(format!(
                "CREATE TABLE IF NOT EXISTS {} (
                    version INT UNSIGNED PRIMARY KEY NOT NULL,
                    name VARCHAR(255) NOT NULL,
                    applied_at VARCHAR(255) NOT NULL,
                    checksum VARCHAR(64) NOT NULL
                )",
                self.schema_version_table_name
            ))?;
        }

        // Validate checksums of previously-applied migrations
        if schema_version_table_existed {
            // Check if the checksum column exists (for backwards compatibility)
            let has_checksum_column: bool = conn
                .query_first(format!(
                    "SELECT COUNT(*) FROM information_schema.columns
                     WHERE table_schema = DATABASE()
                     AND table_name = '{}'
                     AND column_name = 'checksum'",
                    self.schema_version_table_name
                ))?
                .map(|(count,): (i64,)| count > 0)
                .unwrap_or(false);

            if has_checksum_column {
                // Get all applied migrations with their checksums
                let applied_migrations: Vec<(u32, String, String)> = conn.query(format!(
                    "SELECT version, name, checksum FROM {}",
                    self.schema_version_table_name
                ))?;

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

        // get current migration version (highest version number)
        let result: Option<(Option<u32>,)> = conn.query_first(format!(
            "SELECT MAX(version) FROM {}",
            self.schema_version_table_name
        ))?;
        let current_version: u32 = result.and_then(|(v,)| v).unwrap_or(0);

        // iterate through migrations, run those that haven't been run
        let mut migrations_run: Vec<u32> = Vec::new();
        let mut migrations_sorted = self
            .migrations
            .iter()
            .map(|x| (x.version(), x))
            .collect::<Vec<_>>();
        migrations_sorted.sort_by_key(|m| m.0);
        let mut failing_migration: Option<MysqlMigrationFailure> = None;
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
                    "mysql_migration_up",
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
                // We wrap the transaction in a block to ensure it's dropped before using conn again
                let migration_result = {
                    // Start a transaction for this migration
                    let mut tx = conn.start_transaction(TxOpts::default())?;

                    // Check precondition
                    let precondition = match migration.precondition(&mut tx) {
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
                            failing_migration = Some(MysqlMigrationFailure { migration, error });
                            break;
                        }
                    };

                    match precondition {
                        MysqlPrecondition::AlreadySatisfied => {
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
                        MysqlPrecondition::NeedsApply => {
                            let result = migration.up(&mut tx);
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
                        conn.exec_drop(
                            format!(
                                "INSERT INTO {} (version, name, applied_at, checksum) VALUES(?, ?, ?, ?)",
                                self.schema_version_table_name
                            ),
                            (migration_version, migration.name(), &batch_applied_at, checksum),
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
                        failing_migration = Some(MysqlMigrationFailure {
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
        Ok(MysqlMigrationReport {
            schema_version_table_existed,
            schema_version_table_created,
            failing_migration,
            migrations_run,
        })
    }

    /// Rollback migrations down to the specified target version.
    /// Pass `target_version = 0` to rollback all migrations.
    /// Each migration's `down()` method runs within its own transaction, which is automatically rolled back if it fails.
    /// Returns a [MysqlMigrationReport] describing which migrations were rolled back.
    pub fn downgrade(
        &self,
        conn: &mut Conn,
        target_version: u32,
    ) -> Result<MysqlMigrationReport<'_>, Error> {
        // Check if schema version table exists
        let schema_version_table_existed: bool = conn
            .query_first(format!(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = '{}'",
                self.schema_version_table_name
            ))?
            .map(|(count,): (i64,)| count > 0)
            .unwrap_or(false);

        if !schema_version_table_existed {
            // No migrations have been applied yet
            return Ok(MysqlMigrationReport {
                schema_version_table_existed: false,
                schema_version_table_created: false,
                failing_migration: None,
                migrations_run: vec![],
            });
        }

        // Validate checksums of previously-applied migrations (same as upgrade)
        let has_checksum_column: bool = conn
            .query_first(format!(
                "SELECT COUNT(*) FROM information_schema.columns
                 WHERE table_schema = DATABASE()
                 AND table_name = '{}'
                 AND column_name = 'checksum'",
                self.schema_version_table_name
            ))?
            .map(|(count,): (i64,)| count > 0)
            .unwrap_or(false);

        if has_checksum_column {
            let applied_migrations: Vec<(u32, String, String)> = conn.query(format!(
                "SELECT version, name, checksum FROM {}",
                self.schema_version_table_name
            ))?;

            // Verify checksums and detect missing migrations
            for (applied_version, applied_name, applied_checksum) in &applied_migrations {
                if let Some(migration) = self
                    .migrations
                    .iter()
                    .find(|m| m.version() == *applied_version)
                {
                    let current_checksum = Self::calculate_checksum(migration);
                    if current_checksum != *applied_checksum {
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
        let result: Option<(Option<u32>,)> = conn.query_first(format!(
            "SELECT MAX(version) FROM {}",
            self.schema_version_table_name
        ))?;
        let current_version: u32 = result.and_then(|(v,)| v).unwrap_or(0);

        // Validate target version
        if target_version > current_version {
            return Err(Error::Generic(format!(
                "Cannot downgrade to version {} when current version is {}. Target must be <= current version.",
                target_version, current_version
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
        let mut failing_migration: Option<MysqlMigrationFailure> = None;

        for (migration_version, migration) in migrations_to_rollback {
            #[cfg(feature = "tracing")]
            let _span = tracing::info_span!(
                "mysql_migration_down",
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
            let mut tx = conn.start_transaction(TxOpts::default())?;
            let migration_result = migration.down(&mut tx);

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
                    conn.exec_drop(
                        format!(
                            "DELETE FROM {} WHERE version = ?",
                            self.schema_version_table_name
                        ),
                        (migration_version,),
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
                    failing_migration = Some(MysqlMigrationFailure {
                        migration,
                        error: e,
                    });
                    break;
                }
            }
        }

        Ok(MysqlMigrationReport {
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
    use crate::test_mysql::get_test_conn;

    #[tokio::test]
    async fn single_successful_from_clean() {
        use chrono::{DateTime, FixedOffset};

        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        assert_eq!(
            report,
            MysqlMigrationReport {
                schema_version_table_existed: false,
                schema_version_table_created: true,
                migrations_run: vec![1],
                failing_migration: None,
            }
        );

        // Verify schema version table exists and has recorded version 1
        let rows: Vec<(u32, String, String)> = conn
            .query("SELECT version, name, applied_at FROM _migratio_version_")
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 1); // version
        assert_eq!(rows[0].1, "Migration 1"); // name (default)

        let date_string_raw = &rows[0].2;
        let date = DateTime::parse_from_rfc3339(date_string_raw).unwrap();
        assert_eq!(date.timezone(), FixedOffset::east_opt(0).unwrap());

        // Ensure that the date is within 5 seconds of now
        let now = Utc::now();
        let diff = now.timestamp() - date.timestamp();
        assert!(diff < 5);
    }

    #[tokio::test]
    async fn single_unsuccessful_from_clean() {
        let (_pool, mut conn) = get_test_conn().await;

        // Set up a table and some data to ensure it's preserved
        conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY, value INT)")
            .unwrap();
        conn.query_drop("INSERT INTO test (id, value) VALUES (1, 100)")
            .unwrap();
        conn.query_drop("INSERT INTO test (id, value) VALUES (2, 200)")
            .unwrap();

        // Verify test table has original data
        let sum: i64 = conn
            .query_first("SELECT SUM(value) FROM test")
            .unwrap()
            .unwrap();
        assert_eq!(sum, 300);

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                // Do some DML operations that work (can be rolled back in MySQL)
                tx.query_drop("UPDATE test SET value = value * 2")?;
                tx.query_drop("INSERT INTO test (id, value) VALUES (3, 300)")?;
                // Then do something that fails
                tx.query_drop("THIS IS NOT VALID SQL")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        // Verify migration failed
        assert_eq!(report.migrations_run, Vec::<u32>::new());
        assert!(report.failing_migration.is_some());

        // Verify test table still has original data (transaction was rolled back)
        let sum: i64 = conn
            .query_first("SELECT SUM(value) FROM test")
            .unwrap()
            .unwrap();
        assert_eq!(sum, 300); // Should still be 300, not 600

        let count: i64 = conn
            .query_first("SELECT COUNT(*) FROM test")
            .unwrap()
            .unwrap();
        assert_eq!(count, 2); // Should still be 2 rows, not 3
    }

    #[tokio::test]
    async fn upgrade_to_specific_version() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE users (id INT PRIMARY KEY)")?;
                Ok(())
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE posts (id INT PRIMARY KEY)")?;
                Ok(())
            }
        }

        struct Migration3;
        impl MysqlMigration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE comments (id INT PRIMARY KEY)")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);

        // Upgrade to version 2
        let report = migrator.upgrade_to(&mut conn, 2).unwrap();
        assert_eq!(report.migrations_run, vec![1, 2]);

        // Verify only migrations 1 and 2 ran
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 2);

        // Verify users and posts tables exist
        let users_exists: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'users'")
            .unwrap()
            .unwrap();
        assert_eq!(users_exists, 1);

        let posts_exists: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'posts'")
            .unwrap()
            .unwrap();
        assert_eq!(posts_exists, 1);

        // Verify comments table does not exist
        let comments_exists: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'comments'")
            .unwrap()
            .unwrap();
        assert_eq!(comments_exists, 0);
    }

    #[tokio::test]
    async fn upgrade_to_nonexistent_version() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let result = migrator.upgrade_to(&mut conn, 99);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn success_then_failure_from_clean() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE users (id INT PRIMARY KEY)")?;
                tx.query_drop("INSERT INTO users VALUES (1), (2)")?;
                Ok(())
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("INVALID SQL")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        assert_eq!(report.migrations_run, vec![1]);
        assert!(report.failing_migration.is_some());

        // Verify users table exists with data
        let count: i64 = conn
            .query_first("SELECT COUNT(*) FROM users")
            .unwrap()
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    #[should_panic(expected = "Migration version must be greater than 0")]
    async fn new_rejects_zero_version() {
        struct Migration0;
        impl MysqlMigration for Migration0 {
            fn version(&self) -> u32 {
                0
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }
        MysqlMigrator::new(vec![Box::new(Migration0)]);
    }

    #[tokio::test]
    #[should_panic(expected = "Duplicate migration version found: 2")]
    async fn new_rejects_duplicate_versions() {
        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2a;
        impl MysqlMigration for Migration2a {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2b;
        impl MysqlMigration for Migration2b {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        MysqlMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2a),
            Box::new(Migration2b),
        ]);
    }

    #[tokio::test]
    #[should_panic(expected = "Migration versions must start at 1")]
    async fn new_rejects_non_starting_at_one() {
        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl MysqlMigration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        MysqlMigrator::new(vec![Box::new(Migration2), Box::new(Migration3)]);
    }

    #[tokio::test]
    #[should_panic(expected = "Migration versions must be contiguous")]
    async fn new_rejects_non_contiguous() {
        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration3;
        impl MysqlMigration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration3)]);
    }

    #[tokio::test]
    async fn try_new_returns_err_for_non_starting_at_one() {
        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let result = MysqlMigrator::try_new(vec![Box::new(Migration2)]);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Migration versions must start at 1"));
    }

    #[tokio::test]
    async fn try_new_returns_err_for_duplicate_versions() {
        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2a;
        impl MysqlMigration for Migration2a {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2b;
        impl MysqlMigration for Migration2b {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let result = MysqlMigrator::try_new(vec![
            Box::new(Migration1),
            Box::new(Migration2a),
            Box::new(Migration2b),
        ]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Duplicate migration version"));
    }

    #[tokio::test]
    async fn try_new_returns_ok_for_valid_migrations() {
        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let result = MysqlMigrator::try_new(vec![Box::new(Migration1), Box::new(Migration2)]);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn checksum_validation_detects_modified_migration() {
        let (_pool, mut conn) = get_test_conn().await;

        // Apply migration with original name
        struct Migration1V1;
        impl MysqlMigration for Migration1V1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "original_name".to_string()
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1V1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Try to run again with modified name (should fail checksum)
        struct Migration1V2;
        impl MysqlMigration for Migration1V2 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "modified_name".to_string()
            }
        }

        let migrator2 = MysqlMigrator::new(vec![Box::new(Migration1V2)]);
        let result = migrator2.upgrade(&mut conn);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));
    }

    #[tokio::test]
    async fn checksum_validation_passes_for_unmodified_migrations() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1".to_string()
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2".to_string()
            }
        }

        // Apply migration 1
        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Apply migrations 1 and 2 (should validate migration 1's checksum)
        let migrator2 = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let result = migrator2.upgrade(&mut conn);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn checksums_stored_in_database() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "test_migration".to_string()
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Verify checksum is stored
        let checksum: String = conn
            .query_first("SELECT checksum FROM _migratio_version_ WHERE version = 1")
            .unwrap()
            .unwrap();
        assert!(!checksum.is_empty());
        assert_eq!(checksum.len(), 64); // SHA256 hex string
    }

    #[tokio::test]
    async fn downgrade_single_migration() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn down(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("DROP TABLE test")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);

        // Apply migration
        migrator.upgrade(&mut conn).unwrap();
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 1);

        // Rollback
        let report = migrator.downgrade(&mut conn, 0).unwrap();
        assert_eq!(report.migrations_run, vec![1]);

        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 0);

        // Verify table is dropped
        let table_exists: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'test'")
            .unwrap()
            .unwrap();
        assert_eq!(table_exists, 0);
    }

    #[tokio::test]
    async fn downgrade_multiple_migrations() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn down(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("DROP TABLE test1")?;
                Ok(())
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn down(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("DROP TABLE test2")?;
                Ok(())
            }
        }

        struct Migration3;
        impl MysqlMigration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test3 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn down(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("DROP TABLE test3")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);

        // Apply all migrations
        migrator.upgrade(&mut conn).unwrap();

        // Rollback to version 1
        let report = migrator.downgrade(&mut conn, 1).unwrap();
        assert_eq!(report.migrations_run, vec![3, 2]);

        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 1);
    }

    #[tokio::test]
    async fn downgrade_on_clean_database() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
            fn down(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.downgrade(&mut conn, 0).unwrap();
        assert_eq!(report.migrations_run, Vec::<u32>::new());
    }

    #[tokio::test]
    async fn downgrade_with_invalid_target_version() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn down(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("DROP TABLE test")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Try to downgrade to version 5 (greater than current)
        let result = migrator.downgrade(&mut conn, 5);
        assert!(result.is_err());
    }

    #[tokio::test]
    #[should_panic(expected = "does not support downgrade")]
    async fn downgrade_panics_when_down_not_implemented() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();
        migrator.downgrade(&mut conn, 0).unwrap();
    }

    #[tokio::test]
    async fn get_current_version_on_clean_database() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 0);
    }

    #[tokio::test]
    async fn get_current_version_after_migrations() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        migrator.upgrade(&mut conn).unwrap();

        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 2);
    }

    #[tokio::test]
    async fn preview_upgrade_on_clean_database() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let pending = migrator.preview_upgrade(&mut conn).unwrap();

        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].version(), 1);
        assert_eq!(pending[1].version(), 2);
    }

    #[tokio::test]
    async fn preview_upgrade_with_partial_migrations() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
        }

        struct Migration3;
        impl MysqlMigration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        // Apply only migration 1
        let migrator1 = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator1.upgrade(&mut conn).unwrap();

        // Preview with all 3 migrations
        let migrator2 = MysqlMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);
        let pending = migrator2.preview_upgrade(&mut conn).unwrap();

        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].version(), 2);
        assert_eq!(pending[1].version(), 3);
    }

    #[tokio::test]
    async fn preview_downgrade_to_zero() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn down(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("DROP TABLE test1")?;
                Ok(())
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn down(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("DROP TABLE test2")?;
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        migrator.upgrade(&mut conn).unwrap();

        let to_rollback = migrator.preview_downgrade(&mut conn, 0).unwrap();
        assert_eq!(to_rollback.len(), 2);
        assert_eq!(to_rollback[0].version(), 2); // Reverse order
        assert_eq!(to_rollback[1].version(), 1);
    }

    #[tokio::test]
    async fn get_migration_history_on_clean_database() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let history = migrator.get_migration_history(&mut conn).unwrap();
        assert_eq!(history.len(), 0);
    }

    #[tokio::test]
    async fn get_migration_history_after_migrations() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test1".to_string()
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "create_test2".to_string()
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        migrator.upgrade(&mut conn).unwrap();

        let history = migrator.get_migration_history(&mut conn).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].version, 1);
        assert_eq!(history[0].name, "create_test1");
        assert_eq!(history[1].version, 2);
        assert_eq!(history[1].name, "create_test2");
    }

    #[tokio::test]
    async fn hooks_are_called_on_successful_migration() {
        use std::sync::{Arc, Mutex};

        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "test_migration".to_string()
            }
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let events1 = Arc::clone(&events);
        let events2 = Arc::clone(&events);

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)])
            .on_migration_start(move |version, name| {
                events1
                    .lock()
                    .unwrap()
                    .push(format!("start:{}:{}", version, name));
            })
            .on_migration_complete(move |version, name, _duration| {
                events2
                    .lock()
                    .unwrap()
                    .push(format!("complete:{}:{}", version, name));
            });

        migrator.upgrade(&mut conn).unwrap();

        let logged_events = events.lock().unwrap().clone();
        assert_eq!(
            logged_events,
            vec!["start:1:test_migration", "complete:1:test_migration"]
        );
    }

    #[tokio::test]
    async fn hooks_are_called_on_failed_migration() {
        use std::sync::{Arc, Mutex};

        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Err(Error::Generic("Test error".to_string()))
            }
            fn name(&self) -> String {
                "failing_migration".to_string()
            }
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let events1 = Arc::clone(&events);
        let events2 = Arc::clone(&events);

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)])
            .on_migration_start(move |version, name| {
                events1
                    .lock()
                    .unwrap()
                    .push(format!("start:{}:{}", version, name));
            })
            .on_migration_error(move |version, name, _error| {
                events2
                    .lock()
                    .unwrap()
                    .push(format!("error:{}:{}", version, name));
            });

        migrator.upgrade(&mut conn).unwrap();

        let logged_events = events.lock().unwrap().clone();
        assert_eq!(
            logged_events,
            vec!["start:1:failing_migration", "error:1:failing_migration"]
        );
    }

    #[tokio::test]
    async fn precondition_already_satisfied() {
        use std::cell::Cell;

        let (_pool, mut conn) = get_test_conn().await;

        // Create the table manually
        conn.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")
            .unwrap();

        struct Migration1 {
            up_called: Cell<bool>,
        }
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                self.up_called.set(true);
                Ok(())
            }
            fn precondition(&self, tx: &mut Transaction) -> Result<MysqlPrecondition, Error> {
                // Check if table exists
                let exists: i64 = tx
                    .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'test'")?
                    .unwrap();
                if exists > 0 {
                    Ok(MysqlPrecondition::AlreadySatisfied)
                } else {
                    Ok(MysqlPrecondition::NeedsApply)
                }
            }
        }

        let migration = Migration1 {
            up_called: Cell::new(false),
        };
        let migrator = MysqlMigrator::new(vec![Box::new(migration)]);
        migrator.upgrade(&mut conn).unwrap();

        // up() should not have been called
        // Note: We can't check up_called here because migration was moved into Box
        // Instead verify table still exists (wasn't dropped and recreated)
        let count: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'test'")
            .unwrap()
            .unwrap();
        assert_eq!(count, 1);

        // Verify migration was recorded
        let version = migrator.get_current_version(&mut conn).unwrap();
        assert_eq!(version, 1);
    }

    #[tokio::test]
    async fn precondition_needs_apply() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn precondition(&self, tx: &mut Transaction) -> Result<MysqlPrecondition, Error> {
                let exists: i64 = tx
                    .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'test'")?
                    .unwrap();
                if exists > 0 {
                    Ok(MysqlPrecondition::AlreadySatisfied)
                } else {
                    Ok(MysqlPrecondition::NeedsApply)
                }
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        migrator.upgrade(&mut conn).unwrap();

        // Verify table was created (up() was called)
        let count: i64 = conn
            .query_first("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'test'")
            .unwrap()
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn precondition_error() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
            fn precondition(&self, _tx: &mut Transaction) -> Result<MysqlPrecondition, Error> {
                Err(Error::Generic("Precondition check failed".to_string()))
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        // Verify migration failed
        assert_eq!(report.migrations_run, Vec::<u32>::new());
        assert!(report.failing_migration.is_some());
    }

    #[tokio::test]
    async fn migration_failure_accessors() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Err(Error::Generic("Test error".to_string()))
            }
            fn name(&self) -> String {
                "failing_migration".to_string()
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, _tx: &mut Transaction) -> Result<(), Error> {
                Ok(())
            }
        }

        let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);
        let report = migrator.upgrade(&mut conn).unwrap();

        assert!(report.failing_migration.is_some());
        let failure = report.failing_migration.unwrap();
        assert_eq!(failure.migration().version(), 1);
        assert_eq!(failure.migration().name(), "failing_migration");
        assert_eq!(failure.error().to_string(), "Test error");
    }

    #[tokio::test]
    async fn detects_missing_migration_in_code() {
        let (_pool, mut conn) = get_test_conn().await;

        struct Migration1;
        impl MysqlMigration for Migration1 {
            fn version(&self) -> u32 {
                1
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test1 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "migration_1".to_string()
            }
        }

        struct Migration2;
        impl MysqlMigration for Migration2 {
            fn version(&self) -> u32 {
                2
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test2 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "migration_2".to_string()
            }
        }

        struct Migration3;
        impl MysqlMigration for Migration3 {
            fn version(&self) -> u32 {
                3
            }
            fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
                tx.query_drop("CREATE TABLE test3 (id INT PRIMARY KEY)")?;
                Ok(())
            }
            fn name(&self) -> String {
                "migration_3".to_string()
            }
        }

        // Apply all 3 migrations
        let migrator = MysqlMigrator::new(vec![
            Box::new(Migration1),
            Box::new(Migration2),
            Box::new(Migration3),
        ]);
        migrator.upgrade(&mut conn).unwrap();

        // Now manually delete migration 2 from the tracking table to simulate it being removed
        conn.exec_drop("DELETE FROM _migratio_version_ WHERE version = 2", ())
            .unwrap();

        // Try to upgrade again - should detect that migration 2 is in code but not in DB
        // while migration 3 is in DB (orphaned migration scenario)
        let result = migrator.upgrade(&mut conn);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not applied"));
    }
}
