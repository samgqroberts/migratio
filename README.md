# migratio

`migratio` is a lightweight library for managing database migrations (currently for Sqlite).

## Example

```rust
use migratio::{Migration, SqliteMigrator, MigrationReport, Error};
use rusqlite::{Connection, Transaction};

// define your migrations as structs that implement the Migration trait
struct Migration1;

impl Migration for Migration1 {
    fn version(&self) -> u32 {
        1
    }
    fn up(&self, tx: &Transaction) -> Result<(), Error> {
        tx.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])?;
        Ok(())
    }
}

struct Migration2;

impl Migration for Migration2 {
    fn version(&self) -> u32 {
        2
    }
    fn up(&self, tx: &Transaction) -> Result<(), Error> {
        tx.execute("ALTER TABLE users ADD COLUMN email TEXT", [])?;
        Ok(())
    }
}

// construct a migrator with migrations
let migrator = SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);

// connect to your database and run the migrations, receiving a report of the results.
let mut conn = Connection::open_in_memory().unwrap();
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

// assert the migration logic was applied to the database
let mut stmt = conn.prepare("PRAGMA table_info(users)").unwrap();
let columns = stmt
    .query_map([], |row| Ok(row.get::<_, String>(1).unwrap()))
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();
assert_eq!(columns, vec!["id", "name", "email"]);
```

## Motivation

Most Rust-based migration solutions focus only on using SQL to define migration logic.
Even the ones that support writing migrations in Rust use Rust to construct SQL instructions.
Taking a hint from Alembic, this library allows users to write their migration logic fully in Rust, which allows *querying* live data as part of the migration process.
SeaORM allows this, but this library aims to provide an alternative for developers that don't want to adopt a full ORM solution.

```rust
use migratio::{Migration, SqliteMigrator, MigrationReport, Error};
use rusqlite::{Connection, Transaction};

struct Migration1;

impl Migration for Migration1 {
    fn version(&self) -> u32 {
        1
    }
    fn up(&self, tx: &Transaction) -> Result<(), Error> {
        tx.execute(
            "CREATE TABLE user_preferences (name TEXT PRIMARY KEY, preferences TEXT)",
            [],
        )?;
        Ok(())
    }
}

// run this first migration
let mut conn = Connection::open_in_memory().unwrap();
SqliteMigrator::new(vec![Box::new(Migration1)])
    .upgrade(&mut conn)
    .unwrap();

// simulate actual usage of the database
// here we have a suboptimal database serialization format for user preferences
conn.execute(
    "INSERT INTO user_preferences VALUES ('alice', 'scheme:dark|help:off')",
    [],
)
.unwrap();
conn.execute(
    "INSERT INTO user_preferences VALUES ('bob', 'scheme:light|help:off')",
    [],
)
.unwrap();
conn.execute(
    "INSERT INTO user_preferences VALUES ('charlie', 'scheme:dark|help:on')",
    [],
)
.unwrap();

// define another migration that transforms the user preferences data
// using arbitrary Rust logic.
struct Migration2;

impl Migration for Migration2 {
    fn version(&self) -> u32 {
        2
    }
    fn up(&self, tx: &Transaction) -> Result<(), Error> {
        // read all user preferences
        let mut stmt = tx.prepare("SELECT name, preferences FROM user_preferences")?;
        let rows = stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let preferences: String = row.get(1)?;
            Ok((name, preferences))
        })?;

        // transform the preferences data
        for row in rows {
            let (name, preferences) = row?;
            let key_value_pairs = preferences
                .split("|")
                .map(|x| {
                    let mut split = x.split(":");
                    let key = split.next().unwrap();
                    let value = split.next().unwrap();
                    format!("\"{}\":\"{}\"", key, value)
                })
                .collect::<Vec<String>>();
            let new_preferences = format!("{{{}}}", key_value_pairs.join(","));
            tx.execute(
                "UPDATE user_preferences SET preferences = ? WHERE name = ?",
                [new_preferences, name],
            )?;
        }

        Ok(())
    }
}

// run new migration
SqliteMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)])
    .upgrade(&mut conn)
    .unwrap();

// read all data out of connection
let mut stmt = conn
    .prepare("SELECT name, preferences FROM user_preferences")
    .unwrap();
let rows = stmt
    .query_map([], |row| {
        let id: String = row.get(0)?;
        let preferences: String = row.get(1)?;
        Ok((id, preferences))
    })
    .unwrap()
    .collect::<Result<Vec<(String, String)>, _>>()
    .unwrap();

assert_eq!(
    rows,
    vec![
        (
            "alice".to_string(),
            "{\"scheme\":\"dark\",\"help\":\"off\"}".to_string()
        ),
        (
            "bob".to_string(),
            "{\"scheme\":\"light\",\"help\":\"off\"}".to_string()
        ),
        (
            "charlie".to_string(),
            "{\"scheme\":\"dark\",\"help\":\"on\"}".to_string()
        )
    ]
);
```


License: MIT
