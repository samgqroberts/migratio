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
    /// - Must be contiguous (1, 2, 3, ... with no gaps)
    /// - Must be immutable once the migration is applied to any database
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

/// Shared migrator logic between different database types.
pub(crate) struct GenericMigrator {
    pub migrations: Vec<Box<dyn Migration>>,
    pub schema_version_table_name: String,
    pub on_migration_start: Option<Box<dyn Fn(u32, &str) + Send + Sync>>,
    pub on_migration_complete: Option<Box<dyn Fn(u32, &str, std::time::Duration) + Send + Sync>>,
    pub on_migration_skipped: Option<Box<dyn Fn(u32, &str) + Send + Sync>>,
    pub on_migration_error: Option<Box<dyn Fn(u32, &str, &Error) + Send + Sync>>,
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
    /// This is used to verify that migrations haven't been modified after being applied.
    pub fn calculate_checksum(migration: &Box<dyn Migration>) -> String {
        let mut hasher = Sha256::new();
        hasher.update(migration.version().to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(migration.name().as_bytes());
        format!("{:x}", hasher.finalize())
    }
}
