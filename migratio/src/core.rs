use crate::error::Error;
use chrono::Utc;
use sha2::{Digest, Sha256};

/// Represents a failure during a migration.
#[derive(Debug, PartialEq)]
pub struct MigrationFailure<'migration> {
    pub(crate) migration: &'migration Box<dyn Migration>,
    pub(crate) error: Error,
}

impl<'migration> MigrationFailure<'migration> {
    /// Get the migration that failed.
    pub fn migration(&self) -> &dyn Migration {
        self.migration.as_ref()
    }

    /// Get the error that caused the migration to fail.
    pub fn error(&self) -> &Error {
        &self.error
    }
}

/// A report of actions performed during a migration.
#[derive(Debug, PartialEq)]
pub struct MigrationReport<'migration> {
    pub schema_version_table_existed: bool,
    pub schema_version_table_created: bool,
    pub migrations_run: Vec<u32>,
    pub failing_migration: Option<MigrationFailure<'migration>>,
}

/// Represents the result of a migration precondition check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Precondition {
    /// The migration's changes are already present in the database and should be stamped without running up().
    AlreadySatisfied,
    /// The migration needs to be applied by running up().
    NeedsApply,
}

/// Represents the type of a migration entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationType {
    Migration,
    Baseline,
}

impl std::fmt::Display for MigrationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationType::Migration => write!(f, "migration"),
            MigrationType::Baseline => write!(f, "baseline"),
        }
    }
}

/// Represents a migration that has been applied to the database.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedMigration {
    /// The version number of the migration.
    pub version: u32,
    /// The name of the migration.
    pub name: String,
    /// The timestamp when the migration was applied.
    pub applied_at: chrono::DateTime<Utc>,
    /// The checksum of the migration at the time it was applied.
    pub checksum: String,
    /// The type of this migration entry.
    pub migration_type: MigrationType,
}

/// A row read from the version table before full parsing (no applied_at parsing needed for validation).
pub(crate) struct AppliedMigrationRow {
    pub version: u32,
    pub name: String,
    pub checksum: String,
}

#[allow(dead_code)]
pub(crate) const DEFAULT_VERSION_TABLE_NAME: &str = "_migratio_version_";

/// A trait that must be implemented to define a migration.
/// The `version` value must be unique among all migrations supplied to a migrator, and greater than 0.
/// The `name` and `description` methods are optional, and only aid in debugging / observability.
pub trait Migration {
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
    /// - Must be contiguous with other migrations (N, N+1, N+2, ...) but N does not have to be 1
    /// - Must be immutable once the migration is applied to any database
    ///
    /// # Baseline migrations
    ///
    /// If the first migration's version is greater than 1, it is treated as a **baseline**
    /// migration. A baseline represents a compacted snapshot of all schema changes prior to that
    /// version. When applied to a fresh database, it is recorded with `MigrationType::Baseline`.
    /// Downgrading below the baseline version is not permitted.
    fn version(&self) -> u32;

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

    #[cfg(feature = "sqlite")]
    /// Execute the migration's "up" logic for SQLite.
    fn sqlite_up(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
        panic!(
            "Migration {} ('{}') does not implement sqlite_up(). Implement this method to use SQLite.",
            self.version(),
            self.name()
        )
    }

    #[cfg(feature = "sqlite")]
    /// Rollback this migration. This is optional - the default implementation panics.
    /// If you want to support downgrade functionality, implement this method.
    fn sqlite_down(&self, _tx: &rusqlite::Transaction) -> Result<(), Error> {
        panic!(
            "Migration {} ('{}') does not support downgrade. Implement the sqlite_down() method to enable rollback.",
            self.version(),
            self.name()
        )
    }

    #[cfg(feature = "sqlite")]
    /// Optional precondition check for the migration.
    ///
    /// This method is called before running the migration's `up()` method during upgrade.
    /// It allows the migration to check if its changes are already present in the database
    /// (for example, when adopting migratio after using another migration tool).
    ///
    /// If this returns `Precondition::AlreadySatisfied`, the migration is stamped as applied
    /// in the version table without running `up()`. If it returns `Precondition::NeedsApply`,
    /// the migration runs normally via `up()`.
    ///
    /// The default implementation returns `Precondition::NeedsApply`, meaning migrations
    /// always run unless you override this method.
    ///
    /// # Example
    /// ```
    /// use migratio::{Migration, Precondition, Error};
    /// use rusqlite::Transaction;
    ///
    /// struct Migration1;
    /// impl Migration for Migration1 {
    ///     fn version(&self) -> u32 { 1 }
    ///
    ///     fn sqlite_up(&self, tx: &Transaction) -> Result<(), Error> {
    ///         tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", [])?;
    ///         Ok(())
    ///     }
    ///
    ///     fn sqlite_precondition(&self, tx: &Transaction) -> Result<Precondition, Error> {
    ///         // Check if the table already exists
    ///         let mut stmt = tx.prepare(
    ///             "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'"
    ///         )?;
    ///         let count: i64 = stmt.query_row([], |row| row.get(0))?;
    ///
    ///         if count > 0 {
    ///             Ok(Precondition::AlreadySatisfied)
    ///         } else {
    ///             Ok(Precondition::NeedsApply)
    ///         }
    ///     }
    ///
    /// #   #[cfg(feature = "mysql")]
    /// #   fn mysql_up(&self, conn: &mut mysql::Conn) -> Result<(), Error> { Ok(()) }
    /// }
    /// ```
    fn sqlite_precondition(&self, _tx: &rusqlite::Transaction) -> Result<Precondition, Error> {
        Ok(Precondition::NeedsApply)
    }

    #[cfg(feature = "mysql")]
    /// Execute the migration's "up" logic.
    ///
    /// # MySQL DDL Behavior
    ///
    /// **IMPORTANT**: In MySQL, DDL statements (CREATE TABLE, ALTER TABLE, DROP TABLE, etc.)
    /// cause an implicit commit and cannot be rolled back. This migration receives a direct
    /// connection rather than a transaction to make this behavior explicit.
    ///
    /// If a migration fails partway through, any DDL statements that executed successfully
    /// will remain applied to the database. The migration version will NOT be recorded,
    /// allowing you to fix the issue and re-run.
    ///
    /// # Best Practices for MySQL Migrations
    ///
    /// 1. **Keep migrations small and focused** - Fewer statements mean less partial state on failure
    /// 2. **Make migrations idempotent** - Use `IF EXISTS` / `IF NOT EXISTS` where possible
    /// 3. **Test thoroughly** - DDL can't be rolled back automatically
    /// 4. **Put risky DML before DDL** - Data changes can be manually rolled back if needed
    ///
    /// # Example
    ///
    /// ```ignore
    /// fn mysql_up(&self, conn: &mut Conn) -> Result<(), Error> {
    ///     // Idempotent DDL using IF NOT EXISTS
    ///     conn.query_drop(
    ///         "CREATE TABLE IF NOT EXISTS users (
    ///             id INT PRIMARY KEY AUTO_INCREMENT,
    ///             name VARCHAR(255) NOT NULL
    ///         )"
    ///     )?;
    ///     Ok(())
    /// }
    /// ```
    fn mysql_up(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
        panic!(
            "Migration {} ('{}') does not implement mysql_up(). Implement this method to use MySQL.",
            self.version(),
            self.name()
        )
    }

    #[cfg(feature = "mysql")]
    /// Rollback this migration. This is optional - the default implementation panics.
    /// If you want to support downgrade functionality, implement this method.
    ///
    /// # MySQL DDL Behavior
    ///
    /// Same as `up()`, DDL statements in `down()` cannot be rolled back. Each statement
    /// will be committed immediately.
    fn mysql_down(&self, _conn: &mut mysql::Conn) -> Result<(), Error> {
        panic!(
            "Migration {} ('{}') does not support downgrade. Implement the mysql_down() method to enable rollback.",
            self.version(),
            self.name()
        )
    }

    #[cfg(feature = "mysql")]
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
    fn mysql_precondition(&self, _conn: &mut mysql::Conn) -> Result<Precondition, Error> {
        Ok(Precondition::NeedsApply)
    }

    #[cfg(feature = "postgres")]
    /// Execute the migration's "up" logic for PostgreSQL.
    ///
    /// # PostgreSQL Transaction Safety
    ///
    /// Unlike MySQL, PostgreSQL fully supports transactional DDL. This means:
    /// - DDL statements (CREATE TABLE, ALTER TABLE, DROP TABLE, etc.) are part of the transaction
    /// - If a migration fails, all changes are automatically rolled back
    /// - The database remains in a consistent state even if errors occur
    ///
    /// # Exceptions
    ///
    /// The following operations cannot be rolled back even in PostgreSQL:
    /// - `CREATE DATABASE` / `DROP DATABASE`
    /// - `CREATE TABLESPACE` / `DROP TABLESPACE`
    ///
    /// Avoid these operations in migrations if possible.
    ///
    /// # Example
    ///
    /// ```ignore
    /// fn postgres_up(&self, tx: &mut postgres::Transaction) -> Result<(), Error> {
    ///     tx.execute(
    ///         "CREATE TABLE users (
    ///             id SERIAL PRIMARY KEY,
    ///             name TEXT NOT NULL
    ///         )",
    ///         &[],
    ///     )?;
    ///     Ok(())
    /// }
    /// ```
    fn postgres_up(&self, _tx: &mut postgres::Transaction) -> Result<(), Error> {
        panic!(
            "Migration {} ('{}') does not implement postgres_up(). Implement this method to use PostgreSQL.",
            self.version(),
            self.name()
        )
    }

    #[cfg(feature = "postgres")]
    /// Rollback this migration for PostgreSQL. This is optional - the default implementation panics.
    /// If you want to support downgrade functionality, implement this method.
    ///
    /// PostgreSQL fully supports transactional DDL, so rollbacks are atomic and safe.
    fn postgres_down(&self, _tx: &mut postgres::Transaction) -> Result<(), Error> {
        panic!(
            "Migration {} ('{}') does not support downgrade. Implement the postgres_down() method to enable rollback.",
            self.version(),
            self.name()
        )
    }

    #[cfg(feature = "postgres")]
    /// Optional precondition check for the migration.
    ///
    /// This method is called before running the migration's `up()` method during upgrade.
    /// It allows the migration to check if its changes are already present in the database
    /// (for example, when adopting migratio after using another migration tool).
    ///
    /// If this returns `Precondition::AlreadySatisfied`, the migration is stamped as applied
    /// in the version table without running `up()`. If it returns `Precondition::NeedsApply`,
    /// the migration runs normally via `up()`.
    ///
    /// The default implementation returns `Precondition::NeedsApply`, meaning migrations
    /// always run unless you override this method.
    ///
    /// # Example
    /// ```ignore
    /// fn postgres_precondition(&self, tx: &mut postgres::Transaction) -> Result<Precondition, Error> {
    ///     let row = tx.query_one(
    ///         "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_name = 'users')",
    ///         &[],
    ///     )?;
    ///     let exists: bool = row.get(0);
    ///     if exists {
    ///         Ok(Precondition::AlreadySatisfied)
    ///     } else {
    ///         Ok(Precondition::NeedsApply)
    ///     }
    /// }
    /// ```
    fn postgres_precondition(
        &self,
        _tx: &mut postgres::Transaction,
    ) -> Result<Precondition, Error> {
        Ok(Precondition::NeedsApply)
    }
}

impl PartialEq for dyn Migration {
    fn eq(&self, other: &Self) -> bool {
        self.version() == other.version()
    }
}

impl std::fmt::Debug for dyn Migration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Migration")
            .field("version", &self.version())
            .field("name", &self.name())
            .finish()
    }
}

/// Backend trait for database-specific migration operations.
/// Each backend handles its own connection type and transaction semantics.
pub(crate) trait MigrationBackend {
    type Conn;

    // Table management
    fn version_table_exists(conn: &mut Self::Conn, table_name: &str) -> Result<bool, Error>;
    fn create_version_table(conn: &mut Self::Conn, table_name: &str) -> Result<(), Error>;
    fn column_exists(
        conn: &mut Self::Conn,
        table_name: &str,
        column_name: &str,
    ) -> Result<bool, Error>;
    fn add_column(
        conn: &mut Self::Conn,
        table_name: &str,
        column_name: &str,
        column_def: &str,
    ) -> Result<(), Error>;

    // Queries
    fn get_applied_migration_rows(
        conn: &mut Self::Conn,
        table_name: &str,
    ) -> Result<Vec<AppliedMigrationRow>, Error>;
    fn get_max_version(conn: &mut Self::Conn, table_name: &str) -> Result<u32, Error>;
    fn get_migration_history_rows(
        conn: &mut Self::Conn,
        table_name: &str,
    ) -> Result<Vec<AppliedMigration>, Error>;

    // High-level migration operations — each backend handles its own transaction semantics.
    // Returns Ok(true) if migration was executed, Ok(false) if precondition was AlreadySatisfied.
    fn execute_migration_up(
        conn: &mut Self::Conn,
        migration: &Box<dyn Migration>,
        table_name: &str,
        applied_at: &str,
        checksum: &str,
        migration_type: MigrationType,
    ) -> Result<bool, Error>;

    fn execute_migration_down(
        conn: &mut Self::Conn,
        migration: &Box<dyn Migration>,
        table_name: &str,
    ) -> Result<(), Error>;
}

/// Shared migrator logic between different database types.
pub(crate) struct GenericMigrator {
    pub migrations: Vec<Box<dyn Migration>>,
    pub schema_version_table_name: String,
    pub on_migration_start: Option<Box<dyn Fn(u32, &str) + Send + Sync>>,
    pub on_migration_complete: Option<Box<dyn Fn(u32, &str, std::time::Duration) + Send + Sync>>,
    pub on_migration_skipped: Option<Box<dyn Fn(u32, &str) + Send + Sync>>,
    pub on_migration_error: Option<Box<dyn Fn(u32, &str, &Error) + Send + Sync>>,
    pub baseline_version: Option<u32>,
}

// Manual Debug impl since closures don't implement Debug
impl std::fmt::Debug for GenericMigrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericMigrator")
            .field("migrations", &self.migrations)
            .field("schema_version_table_name", &self.schema_version_table_name)
            .field("on_migration_start", &self.on_migration_start.is_some())
            .field(
                "on_migration_complete",
                &self.on_migration_complete.is_some(),
            )
            .field("on_migration_skipped", &self.on_migration_skipped.is_some())
            .field("on_migration_error", &self.on_migration_error.is_some())
            .field("baseline_version", &self.baseline_version)
            .finish()
    }
}

#[allow(dead_code)]
impl GenericMigrator {
    /// Create a new MysqlMigrator, validating migration invariants.
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

        // Check for contiguity: versions must be N, N+1, N+2, ... for some N >= 1
        if !versions.is_empty() {
            for i in 1..versions.len() {
                if versions[i] != versions[i - 1] + 1 {
                    return Err(format!(
                        "Migration versions must be contiguous. Found gap between version {} and {}",
                        versions[i - 1], versions[i]
                    ));
                }
            }
        }

        // Derive baseline_version: if the first migration version is > 1, it is a baseline
        let baseline_version = if !versions.is_empty() && versions[0] > 1 {
            Some(versions[0])
        } else {
            None
        };

        Ok(Self {
            migrations,
            schema_version_table_name: DEFAULT_VERSION_TABLE_NAME.to_string(),
            on_migration_start: None,
            on_migration_complete: None,
            on_migration_skipped: None,
            on_migration_error: None,
            baseline_version,
        })
    }

    /// Set a custom name for the schema version tracking table.
    /// Defaults to "_migratio_version_".
    pub fn set_schema_version_table_name(&mut self, name: impl Into<String>) -> () {
        self.schema_version_table_name = name.into();
    }

    pub fn set_on_migration_start(
        &mut self,
        callback: impl Fn(u32, &str) + Send + Sync + 'static,
    ) -> &mut Self {
        self.on_migration_start = Some(Box::new(callback));
        self
    }

    pub fn set_on_migration_complete(
        &mut self,
        callback: impl Fn(u32, &str, std::time::Duration) + Send + Sync + 'static,
    ) -> &mut Self {
        self.on_migration_complete = Some(Box::new(callback));
        self
    }

    pub fn set_on_migration_skipped(
        &mut self,
        callback: impl Fn(u32, &str) + Send + Sync + 'static,
    ) -> &mut Self {
        self.on_migration_skipped = Some(Box::new(callback));
        self
    }

    pub fn set_on_migration_error(
        &mut self,
        callback: impl Fn(u32, &str, &Error) + Send + Sync + 'static,
    ) -> &mut Self {
        self.on_migration_error = Some(Box::new(callback));
        self
    }

    /// Calculate a checksum for a migration based on its version and name.
    ///
    /// This checksum is stored when a migration is applied and verified on subsequent runs
    /// to detect if a migration's identity has changed after being applied.
    ///
    /// # What the checksum covers
    ///
    /// The checksum is calculated from:
    /// - The migration's `version()` number
    /// - The migration's `name()` string
    ///
    /// # What the checksum does NOT cover
    ///
    /// The checksum intentionally does **not** include:
    /// - The migration's `up()` or `down()` logic
    /// - The migration's `description()`
    /// - Any other runtime behavior
    ///
    /// This is a deliberate design choice: Rust closures and trait method implementations
    /// cannot be meaningfully hashed at runtime. The checksum serves to detect accidental
    /// renaming or version number changes, not to verify that migration logic is unchanged.
    ///
    /// # Implications
    ///
    /// - Changing a migration's `version()` or `name()` after it has been applied will cause
    ///   a checksum mismatch error on subsequent runs.
    /// - Changing a migration's `up()` logic after it has been applied will **not** be detected.
    ///   This is the user's responsibility to avoid.
    /// - The `description()` can be freely changed at any time.
    pub fn calculate_checksum(migration: &Box<dyn Migration>) -> String {
        let mut hasher = Sha256::new();
        hasher.update(migration.version().to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(migration.name().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Generic upgrade logic shared across all backends.
    pub(crate) fn generic_upgrade<B: MigrationBackend>(
        &self,
        conn: &mut B::Conn,
        target_version: Option<u32>,
    ) -> Result<MigrationReport<'_>, Error> {
        let table_name = &self.schema_version_table_name;

        // Check if the schema version table exists and create it if not
        let schema_version_table_existed = B::version_table_exists(conn, table_name)?;
        if !schema_version_table_existed {
            #[cfg(feature = "tracing")]
            tracing::info!("Creating migration tracking table: {}", table_name);
            B::create_version_table(conn, table_name)?;
        }
        #[cfg(feature = "tracing")]
        if schema_version_table_existed {
            tracing::debug!("Migration tracking table '{}' exists", table_name);
        }

        // For backwards compatibility: add checksum column if it doesn't exist
        if schema_version_table_existed {
            let has_checksum = B::column_exists(conn, table_name, "checksum")?;
            if !has_checksum {
                B::add_column(conn, table_name, "checksum", "text not null default ''")?;
            }

            // For backwards compatibility: add migration_type column if it doesn't exist
            let has_migration_type = B::column_exists(conn, table_name, "migration_type")?;
            if !has_migration_type {
                B::add_column(
                    conn,
                    table_name,
                    "migration_type",
                    "text not null default 'migration'",
                )?;
            }
        }

        // Validate checksums and detect missing/orphaned migrations
        if schema_version_table_existed {
            let has_checksum = B::column_exists(conn, table_name, "checksum")?;
            if has_checksum {
                let applied_rows = B::get_applied_migration_rows(conn, table_name)?;

                // Check if the DB is below the baseline version (if baseline is set)
                if let Some(bv) = self.baseline_version {
                    let applied_versions: Vec<u32> =
                        applied_rows.iter().map(|r| r.version).collect();
                    if !applied_versions.is_empty() {
                        let max_applied = *applied_versions.iter().max().unwrap();
                        if max_applied > 0 && max_applied < bv {
                            return Err(Error::Generic(format!(
                                "Database is at version {} which is below baseline version {}. Manual intervention required.",
                                max_applied, bv
                            )));
                        }
                    }
                }

                // Verify checksums and detect missing migrations
                // Skip versions at or below the baseline — those are pre-compaction history
                for row in &applied_rows {
                    if let Some(bv) = self.baseline_version {
                        if row.version <= bv {
                            continue;
                        }
                    }

                    if let Some(migration) = self
                        .migrations
                        .iter()
                        .find(|m| m.version() == row.version)
                    {
                        let current_checksum = Self::calculate_checksum(migration);
                        if current_checksum != row.checksum {
                            return Err(Error::Generic(format!(
                                "Migration {} checksum mismatch. Expected '{}' but found '{}' in database. \
                                Current name: '{}', name in DB: '{}'. \
                                This indicates the migration was modified after being applied.",
                                row.version,
                                current_checksum,
                                row.checksum,
                                migration.name(),
                                row.name
                            )));
                        }
                    } else {
                        return Err(Error::Generic(format!(
                            "Migration {} ('{}') was previously applied but is no longer present in the migration list. \
                            Applied migrations cannot be removed from the codebase.",
                            row.version,
                            row.name
                        )));
                    }
                }

                // Detect orphaned migrations (gaps in applied versions with code present)
                // Only check from the first code-defined version onwards; versions below
                // baseline are expected leftovers from before compaction.
                let applied_versions: Vec<u32> = applied_rows.iter().map(|r| r.version).collect();
                if !applied_versions.is_empty() {
                    let max_applied = *applied_versions.iter().max().unwrap();
                    let first_code_version = self
                        .migrations
                        .iter()
                        .map(|m| m.version())
                        .min()
                        .unwrap_or(1);
                    for expected_version in first_code_version..=max_applied {
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
        }

        // Get current migration version (highest version number)
        let current_version = B::get_max_version(conn, table_name)?;

        // Iterate through migrations, run those that haven't been run
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

                let migration_start = std::time::Instant::now();

                // Use Baseline migration type when running the baseline migration on a fresh DB
                let migration_type =
                    if self.baseline_version == Some(migration_version) && current_version == 0 {
                        MigrationType::Baseline
                    } else {
                        MigrationType::Migration
                    };

                let migration_result = B::execute_migration_up(
                    conn,
                    migration,
                    table_name,
                    &batch_applied_at,
                    &Self::calculate_checksum(migration),
                    migration_type,
                );

                match migration_result {
                    Ok(was_applied) => {
                        let migration_duration = migration_start.elapsed();

                        // Record migration as run regardless of whether it was applied or skipped
                        migrations_run.push(migration_version);
                        // Since any migration succeeded, if schema version table had not originally existed,
                        // we can mark that it was created
                        schema_version_table_created = true;

                        if was_applied {
                            #[cfg(feature = "tracing")]
                            tracing::info!(
                                duration_ms = migration_duration.as_millis(),
                                "Migration completed successfully"
                            );

                            // Call on_migration_complete hook
                            if let Some(ref callback) = self.on_migration_complete {
                                callback(migration_version, &migration.name(), migration_duration);
                            }
                        } else {
                            // Precondition was AlreadySatisfied
                            #[cfg(feature = "tracing")]
                            tracing::info!("Precondition already satisfied, stamping migration without running up()");

                            // Call on_migration_skipped hook
                            if let Some(ref callback) = self.on_migration_skipped {
                                callback(migration_version, &migration.name());
                            }
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

        Ok(MigrationReport {
            schema_version_table_existed,
            schema_version_table_created,
            failing_migration,
            migrations_run,
        })
    }

    /// Generic downgrade logic shared across all backends.
    pub(crate) fn generic_downgrade<B: MigrationBackend>(
        &self,
        conn: &mut B::Conn,
        target_version: u32,
    ) -> Result<MigrationReport<'_>, Error> {
        let table_name = &self.schema_version_table_name;

        // Check if schema version table exists
        let schema_version_table_existed = B::version_table_exists(conn, table_name)?;

        if !schema_version_table_existed {
            return Ok(MigrationReport {
                schema_version_table_existed: false,
                schema_version_table_created: false,
                failing_migration: None,
                migrations_run: vec![],
            });
        }

        // Validate checksums of previously-applied migrations (same as upgrade)
        let has_checksum = B::column_exists(conn, table_name, "checksum")?;
        if has_checksum {
            let applied_rows = B::get_applied_migration_rows(conn, table_name)?;

            // Verify checksums and detect missing migrations
            // Skip versions at or below the baseline — those are pre-compaction history
            for row in &applied_rows {
                if let Some(bv) = self.baseline_version {
                    if row.version <= bv {
                        continue;
                    }
                }

                if let Some(migration) = self
                    .migrations
                    .iter()
                    .find(|m| m.version() == row.version)
                {
                    let current_checksum = Self::calculate_checksum(migration);
                    if current_checksum != row.checksum {
                        return Err(Error::Generic(format!(
                            "Migration {} checksum mismatch. Expected '{}' but found '{}' in database. \
                            Current name: '{}', name in DB: '{}'. \
                            This indicates the migration was modified after being applied.",
                            row.version,
                            current_checksum,
                            row.checksum,
                            migration.name(),
                            row.name
                        )));
                    }
                } else {
                    return Err(Error::Generic(format!(
                        "Migration {} ('{}') was previously applied but is no longer present in the migration list. \
                        Applied migrations cannot be removed from the codebase.",
                        row.version,
                        row.name
                    )));
                }
            }

            // Detect orphaned migrations
            // Only check from the first code-defined version onwards
            let applied_versions: Vec<u32> = applied_rows.iter().map(|r| r.version).collect();
            if !applied_versions.is_empty() {
                let max_applied = *applied_versions.iter().max().unwrap();
                let first_code_version = self
                    .migrations
                    .iter()
                    .map(|m| m.version())
                    .min()
                    .unwrap_or(1);
                for expected_version in first_code_version..=max_applied {
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
        let current_version = B::get_max_version(conn, table_name)?;

        // Cannot downgrade below the baseline version
        if let Some(bv) = self.baseline_version {
            if target_version < bv {
                return Err(Error::Generic(format!(
                    "Cannot downgrade below baseline version {}. The baseline migration represents a compacted state that cannot be rolled back.",
                    bv
                )));
            }
        }

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
        migrations_to_rollback.sort_by_key(|m| std::cmp::Reverse(m.0));

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

            let migration_start = std::time::Instant::now();

            let migration_result = B::execute_migration_down(conn, migration, table_name);

            match migration_result {
                Ok(_) => {
                    let migration_duration = migration_start.elapsed();

                    #[cfg(feature = "tracing")]
                    tracing::info!(
                        duration_ms = migration_duration.as_millis(),
                        "Migration rolled back successfully"
                    );

                    // Record migration as rolled back
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

    /// Generic get_current_version logic.
    pub(crate) fn generic_get_current_version<B: MigrationBackend>(
        &self,
        conn: &mut B::Conn,
    ) -> Result<u32, Error> {
        let table_name = &self.schema_version_table_name;
        let table_exists = B::version_table_exists(conn, table_name)?;
        if !table_exists {
            return Ok(0);
        }
        B::get_max_version(conn, table_name)
    }

    /// Generic get_migration_history logic.
    pub(crate) fn generic_get_migration_history<B: MigrationBackend>(
        &self,
        conn: &mut B::Conn,
    ) -> Result<Vec<AppliedMigration>, Error> {
        let table_name = &self.schema_version_table_name;
        let table_exists = B::version_table_exists(conn, table_name)?;
        if !table_exists {
            return Ok(vec![]);
        }
        B::get_migration_history_rows(conn, table_name)
    }

    /// Generic preview_upgrade logic.
    pub(crate) fn generic_preview_upgrade<B: MigrationBackend>(
        &self,
        conn: &mut B::Conn,
    ) -> Result<Vec<&Box<dyn Migration>>, Error> {
        let current_version = self.generic_get_current_version::<B>(conn)?;
        let mut pending_migrations = self
            .migrations
            .iter()
            .filter(|m| m.version() > current_version)
            .collect::<Vec<_>>();
        pending_migrations.sort_by_key(|m| m.version());
        Ok(pending_migrations)
    }

    /// Generic preview_downgrade logic.
    pub(crate) fn generic_preview_downgrade<B: MigrationBackend>(
        &self,
        conn: &mut B::Conn,
        target_version: u32,
    ) -> Result<Vec<&Box<dyn Migration>>, Error> {
        // Cannot preview downgrade below the baseline version
        if let Some(bv) = self.baseline_version {
            if target_version < bv {
                return Err(Error::Generic(format!(
                    "Cannot downgrade below baseline version {}.",
                    bv
                )));
            }
        }

        let current_version = self.generic_get_current_version::<B>(conn)?;

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
        migrations_to_rollback.sort_by_key(|m| std::cmp::Reverse(m.version()));
        Ok(migrations_to_rollback)
    }
}
