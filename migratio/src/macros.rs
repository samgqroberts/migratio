//! Convenience macros for defining migrations.

/// Define a simple SQL-only migration.
///
/// This macro reduces boilerplate for migrations that consist of simple SQL statements
/// that work identically across SQLite, MySQL, and PostgreSQL (or with minor dialect differences).
///
/// # Basic Usage
///
/// ```
/// use migratio::sql_migration;
///
/// // Define a migration struct with SQL
/// sql_migration!(CreateUsersTable, 1, "Create users table",
///     up: "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
///     down: "DROP TABLE users"
/// );
/// ```
///
/// This expands to a struct `CreateUsersTable` that implements the [`Migration`](crate::Migration) trait
/// with `sqlite_up`/`sqlite_down`, `mysql_up`/`mysql_down`, and `postgres_up`/`postgres_down` methods.
///
/// # Database-Specific SQL
///
/// When databases require different SQL syntax, provide separate statements:
///
/// ```
/// use migratio::sql_migration;
///
/// sql_migration!(CreateUsersTable, 1, "Create users table",
///     sqlite_up: "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)",
///     sqlite_down: "DROP TABLE users",
///     mysql_up: "CREATE TABLE users (id INT PRIMARY KEY AUTO_INCREMENT, name VARCHAR(255))",
///     mysql_down: "DROP TABLE users",
///     postgres_up: "CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT)",
///     postgres_down: "DROP TABLE users"
/// );
/// ```
///
/// For backwards compatibility, you can also specify just SQLite and MySQL - PostgreSQL will
/// use the SQLite SQL since they share most syntax:
///
/// ```
/// use migratio::sql_migration;
///
/// sql_migration!(CreateUsersTable, 1, "Create users table",
///     sqlite_up: "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
///     sqlite_down: "DROP TABLE users",
///     mysql_up: "CREATE TABLE users (id INT PRIMARY KEY AUTO_INCREMENT, name VARCHAR(255))",
///     mysql_down: "DROP TABLE users"
/// );
/// ```
///
/// # Up-Only Migrations
///
/// If your migration doesn't need a `down` implementation (common for production systems),
/// omit the `down` clauses:
///
/// ```
/// use migratio::sql_migration;
///
/// sql_migration!(CreateUsersTable, 1, "Create users table",
///     up: "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)"
/// );
/// ```
///
/// Calling `downgrade()` on such a migration will panic with a helpful error message.
///
/// # Multiple Statements
///
/// For migrations with multiple SQL statements, use an array:
///
/// ```
/// use migratio::sql_migration;
///
/// sql_migration!(InitialSchema, 1, "Create initial schema",
///     up: [
///         "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
///         "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT)",
///         "CREATE INDEX idx_posts_user ON posts(user_id)"
///     ],
///     down: [
///         "DROP INDEX idx_posts_user",
///         "DROP TABLE posts",
///         "DROP TABLE users"
///     ]
/// );
/// ```
///
/// # When to Use This Macro
///
/// Use `sql_migration!` when:
/// - Your migration is pure SQL with no Rust logic
/// - The SQL works on all databases (or you provide dialect-specific versions)
/// - You don't need to query data and transform it in Rust
///
/// For complex migrations that need to query data, transform it in Rust, and write it back,
/// implement the [`Migration`](crate::Migration) trait directly instead.
#[macro_export]
macro_rules! sql_migration {
    // Pattern 1a: Shared up/down SQL (array of statements)
    ($name:ident, $version:expr, $migration_name:expr,
        up: [$($up_sql:expr),* $(,)?],
        down: [$($down_sql:expr),* $(,)?]
    ) => {
        $crate::__sql_migration_impl!($name, $version, $migration_name,
            sqlite_up: [$($up_sql),*],
            sqlite_down: [$($down_sql),*],
            mysql_up: [$($up_sql),*],
            mysql_down: [$($down_sql),*],
            postgres_up: [$($up_sql),*],
            postgres_down: [$($down_sql),*]
        );
    };

    // Pattern 1b: Shared up/down SQL (single statement)
    ($name:ident, $version:expr, $migration_name:expr,
        up: $up_sql:expr,
        down: $down_sql:expr
    ) => {
        $crate::__sql_migration_impl!($name, $version, $migration_name,
            sqlite_up: [$up_sql],
            sqlite_down: [$down_sql],
            mysql_up: [$up_sql],
            mysql_down: [$down_sql],
            postgres_up: [$up_sql],
            postgres_down: [$down_sql]
        );
    };

    // Pattern 2a: Shared up SQL only (array, no down)
    ($name:ident, $version:expr, $migration_name:expr,
        up: [$($up_sql:expr),* $(,)?]
    ) => {
        $crate::__sql_migration_impl_no_down!($name, $version, $migration_name,
            sqlite_up: [$($up_sql),*],
            mysql_up: [$($up_sql),*],
            postgres_up: [$($up_sql),*]
        );
    };

    // Pattern 2b: Shared up SQL only (single statement, no down)
    ($name:ident, $version:expr, $migration_name:expr,
        up: $up_sql:expr
    ) => {
        $crate::__sql_migration_impl_no_down!($name, $version, $migration_name,
            sqlite_up: [$up_sql],
            mysql_up: [$up_sql],
            postgres_up: [$up_sql]
        );
    };

    // Pattern 3a: Database-specific SQL with down (single statements, all three DBs)
    ($name:ident, $version:expr, $migration_name:expr,
        sqlite_up: $sqlite_up:expr,
        sqlite_down: $sqlite_down:expr,
        mysql_up: $mysql_up:expr,
        mysql_down: $mysql_down:expr,
        postgres_up: $postgres_up:expr,
        postgres_down: $postgres_down:expr
    ) => {
        $crate::__sql_migration_impl!($name, $version, $migration_name,
            sqlite_up: [$sqlite_up],
            sqlite_down: [$sqlite_down],
            mysql_up: [$mysql_up],
            mysql_down: [$mysql_down],
            postgres_up: [$postgres_up],
            postgres_down: [$postgres_down]
        );
    };

    // Pattern 3b: Database-specific SQL with down (single statements, SQLite/MySQL only - backwards compat)
    // Uses SQLite SQL for Postgres since they share most syntax
    ($name:ident, $version:expr, $migration_name:expr,
        sqlite_up: $sqlite_up:expr,
        sqlite_down: $sqlite_down:expr,
        mysql_up: $mysql_up:expr,
        mysql_down: $mysql_down:expr
    ) => {
        $crate::__sql_migration_impl!($name, $version, $migration_name,
            sqlite_up: [$sqlite_up],
            sqlite_down: [$sqlite_down],
            mysql_up: [$mysql_up],
            mysql_down: [$mysql_down],
            postgres_up: [$sqlite_up],
            postgres_down: [$sqlite_down]
        );
    };

    // Pattern 4a: Database-specific SQL without down (single statements, all three DBs)
    ($name:ident, $version:expr, $migration_name:expr,
        sqlite_up: $sqlite_up:expr,
        mysql_up: $mysql_up:expr,
        postgres_up: $postgres_up:expr
    ) => {
        $crate::__sql_migration_impl_no_down!($name, $version, $migration_name,
            sqlite_up: [$sqlite_up],
            mysql_up: [$mysql_up],
            postgres_up: [$postgres_up]
        );
    };

    // Pattern 4b: Database-specific SQL without down (single statements, SQLite/MySQL only - backwards compat)
    // Uses SQLite SQL for Postgres since they share most syntax
    ($name:ident, $version:expr, $migration_name:expr,
        sqlite_up: $sqlite_up:expr,
        mysql_up: $mysql_up:expr
    ) => {
        $crate::__sql_migration_impl_no_down!($name, $version, $migration_name,
            sqlite_up: [$sqlite_up],
            mysql_up: [$mysql_up],
            postgres_up: [$sqlite_up]
        );
    };
}

/// Internal implementation macro with down methods.
#[macro_export]
#[doc(hidden)]
macro_rules! __sql_migration_impl {
    ($name:ident, $version:expr, $migration_name:expr,
        sqlite_up: [$($sqlite_up:expr),*],
        sqlite_down: [$($sqlite_down:expr),*],
        mysql_up: [$($mysql_up:expr),*],
        mysql_down: [$($mysql_down:expr),*],
        postgres_up: [$($postgres_up:expr),*],
        postgres_down: [$($postgres_down:expr),*]
    ) => {
        pub struct $name;

        impl $crate::Migration for $name {
            fn version(&self) -> u32 {
                $version
            }

            fn name(&self) -> String {
                $migration_name.to_string()
            }

            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, tx: &::rusqlite::Transaction) -> Result<(), $crate::Error> {
                $(tx.execute($sqlite_up, [])?;)*
                Ok(())
            }

            #[cfg(feature = "sqlite")]
            fn sqlite_down(&self, tx: &::rusqlite::Transaction) -> Result<(), $crate::Error> {
                $(tx.execute($sqlite_down, [])?;)*
                Ok(())
            }

            #[cfg(feature = "mysql")]
            fn mysql_up(&self, conn: &mut ::mysql::Conn) -> Result<(), $crate::Error> {
                use ::mysql::prelude::Queryable;
                $(conn.query_drop($mysql_up)?;)*
                Ok(())
            }

            #[cfg(feature = "mysql")]
            fn mysql_down(&self, conn: &mut ::mysql::Conn) -> Result<(), $crate::Error> {
                use ::mysql::prelude::Queryable;
                $(conn.query_drop($mysql_down)?;)*
                Ok(())
            }

            #[cfg(feature = "postgres")]
            fn postgres_up(&self, tx: &mut ::postgres::Transaction) -> Result<(), $crate::Error> {
                $(tx.execute($postgres_up, &[])?;)*
                Ok(())
            }

            #[cfg(feature = "postgres")]
            fn postgres_down(&self, tx: &mut ::postgres::Transaction) -> Result<(), $crate::Error> {
                $(tx.execute($postgres_down, &[])?;)*
                Ok(())
            }
        }
    };
}

/// Internal implementation macro without down methods.
#[macro_export]
#[doc(hidden)]
macro_rules! __sql_migration_impl_no_down {
    ($name:ident, $version:expr, $migration_name:expr,
        sqlite_up: [$($sqlite_up:expr),*],
        mysql_up: [$($mysql_up:expr),*],
        postgres_up: [$($postgres_up:expr),*]
    ) => {
        pub struct $name;

        impl $crate::Migration for $name {
            fn version(&self) -> u32 {
                $version
            }

            fn name(&self) -> String {
                $migration_name.to_string()
            }

            #[cfg(feature = "sqlite")]
            fn sqlite_up(&self, tx: &::rusqlite::Transaction) -> Result<(), $crate::Error> {
                $(tx.execute($sqlite_up, [])?;)*
                Ok(())
            }

            #[cfg(feature = "mysql")]
            fn mysql_up(&self, conn: &mut ::mysql::Conn) -> Result<(), $crate::Error> {
                use ::mysql::prelude::Queryable;
                $(conn.query_drop($mysql_up)?;)*
                Ok(())
            }

            #[cfg(feature = "postgres")]
            fn postgres_up(&self, tx: &mut ::postgres::Transaction) -> Result<(), $crate::Error> {
                $(tx.execute($postgres_up, &[])?;)*
                Ok(())
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use crate::Migration;

    #[test]
    fn test_macro_compiles_shared_sql() {
        sql_migration!(TestMigration1, 1, "Test migration",
            up: "CREATE TABLE test (id INTEGER PRIMARY KEY)",
            down: "DROP TABLE test"
        );

        let m = TestMigration1;
        assert_eq!(m.version(), 1);
        assert_eq!(m.name(), "Test migration");
    }

    #[test]
    fn test_macro_compiles_up_only() {
        sql_migration!(TestMigration2, 2, "Test migration 2",
            up: "CREATE TABLE test2 (id INTEGER PRIMARY KEY)"
        );

        let m = TestMigration2;
        assert_eq!(m.version(), 2);
        assert_eq!(m.name(), "Test migration 2");
    }

    #[test]
    fn test_macro_compiles_multi_statement() {
        sql_migration!(TestMigration3, 3, "Multi-statement",
            up: [
                "CREATE TABLE a (id INTEGER PRIMARY KEY)",
                "CREATE TABLE b (id INTEGER PRIMARY KEY)"
            ],
            down: [
                "DROP TABLE b",
                "DROP TABLE a"
            ]
        );

        let m = TestMigration3;
        assert_eq!(m.version(), 3);
    }

    #[cfg(all(feature = "sqlite", feature = "mysql"))]
    #[test]
    fn test_macro_compiles_db_specific() {
        sql_migration!(TestMigration4, 4, "DB-specific",
            sqlite_up: "CREATE TABLE test (id INTEGER PRIMARY KEY AUTOINCREMENT)",
            sqlite_down: "DROP TABLE test",
            mysql_up: "CREATE TABLE test (id INT PRIMARY KEY AUTO_INCREMENT)",
            mysql_down: "DROP TABLE test"
        );

        let m = TestMigration4;
        assert_eq!(m.version(), 4);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn test_macro_sqlite_runtime() {
        use crate::sqlite::SqliteMigrator;
        use rusqlite::Connection;

        sql_migration!(CreateUsers, 1, "Create users",
            up: "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
            down: "DROP TABLE users"
        );

        sql_migration!(CreatePosts, 2, "Create posts",
            up: [
                "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT)",
                "CREATE INDEX idx_posts_user ON posts(user_id)"
            ],
            down: [
                "DROP INDEX idx_posts_user",
                "DROP TABLE posts"
            ]
        );

        let migrator = SqliteMigrator::new(vec![Box::new(CreateUsers), Box::new(CreatePosts)]);

        let mut conn = Connection::open_in_memory().unwrap();

        // Run migrations
        let report = migrator.upgrade(&mut conn).unwrap();
        assert_eq!(report.migrations_run, vec![1, 2]);

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name != '_migratio_version_' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tables, vec!["posts", "users"]);

        // Verify index exists
        let index_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_posts_user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 1);

        // Test downgrade
        let report = migrator.downgrade(&mut conn, 0).unwrap();
        assert_eq!(report.migrations_run, vec![2, 1]);

        // Verify tables are gone
        let table_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('users', 'posts')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 0);
    }
}
