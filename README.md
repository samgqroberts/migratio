# migratio

Write database migrations in Rust (similar to Alembic), currently for SQLite.

## Features

- Define migrations programmatically in Rust
- Automatic migration tracking and versioning
- SQLite support with rusqlite integration

## Usage

```rust
use migratio::{Migration, SqliteMigrator};

// define your migrations
struct Migration1;

impl Migration for Migration1 {
    fn version(&self) -> u32 {
        1
    }
    fn up(&self, conn: &mut Connection) -> Result<(), Error> {
        conn.execute("CREATE TABLE table_1 (id INTEGER PRIMARY KEY)", [])?;
        Ok(())
    }
}

struct Migration2;

impl Migration for Migration2 {
    fn version(&self) -> u32 {
        2
    }
    fn up(&self, conn: &mut Connection) -> Result<(), Error> {
        conn.execute("CREATE TABLE table_2 (id INTEGER PRIMARY KEY)", [])?;
        Ok(())
    }
}

// construct a migrator with migrations
let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);

// connect to your database and run the migrations, receiving a report of the results.
let mut conn = Connection::open_in_memory()?;
let report = migrator.upgrade(&mut conn).unwrap();

assert_eq!(
    report,
    MigrationReport {
        schema_version_table_existed: false,
        schema_version_table_created: true,
        migrations_run: vec![1, 2],
        failing_migration: None
    }
);
```

## Motivation

Most Rust-based migration solutions focus only on using SQL to define migration logic.
Even the ones that support writing migrations in Rust use Rust to construct SQL instructions.
Taking a hint from Alembic, this library allows users to write their migration logic fully in Rust, which allows *querying* live data as part of the migration process.
SeaORM allows this, but this library aims to provide an alternative for developers that don't want to adopt a full ORM solution.

## License

MIT
