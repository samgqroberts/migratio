# MySQL Support for Migratio

This is a first-cut implementation of MySQL support for migratio. The MySQL implementation closely mirrors the SQLite implementation.

## Installation

Add migratio to your `Cargo.toml` with the `mysql` feature:

```toml
[dependencies]
migratio = { version = "0.1", features = ["mysql"] }
mysql = "25.0"
```

## Usage

The MySQL API is very similar to the SQLite API. Here's a basic example:

```rust
use migratio::{Error, MysqlMigration, MysqlMigrator};
use mysql::prelude::*;
use mysql::{Conn, OptsBuilder, Transaction};

// Define your migrations
struct Migration1;
impl MysqlMigration for Migration1 {
    fn version(&self) -> u32 {
        1
    }
    fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
        tx.query_drop("CREATE TABLE users (id INT PRIMARY KEY AUTO_INCREMENT, name VARCHAR(255))")?;
        Ok(())
    }
    fn name(&self) -> String {
        "create_users_table".to_string()
    }
}

// Create migrator
let migrator = MysqlMigrator::new(vec![Box::new(Migration1)]);

// Connect to MySQL
let opts = OptsBuilder::new()
    .ip_or_hostname(Some("127.0.0.1"))
    .tcp_port(3306)
    .user(Some("root"))
    .pass(Some("password"))
    .db_name(Some("mydb"));

let mut conn = Conn::new(opts).unwrap();

// Run migrations
let report = migrator.upgrade(&mut conn).unwrap();
```

## Key Differences from SQLite

1. **Transaction Type**: MySQL migrations receive a `&mut mysql::Transaction` instead of `&rusqlite::Transaction`
2. **Connection Type**: Use `mysql::Conn` instead of `rusqlite::Connection`
3. **Trait Import**: You'll need to import `mysql::prelude::*` to use query methods like `query_drop()`
4. **SQL Syntax**: Use MySQL-specific SQL syntax (e.g., `AUTO_INCREMENT` instead of `AUTOINCREMENT`)

## Feature Parity

The MySQL implementation supports all the same features as the SQLite version:

- ✅ Basic migrations with `up()` and `down()`
- ✅ Migration versioning and tracking
- ✅ Checksum validation
- ✅ Preconditions with `precondition()`
- ✅ Migration history with `get_migration_history()`
- ✅ Preview migrations with `preview_upgrade()` and `preview_downgrade()`
- ✅ Rollback support with `downgrade()`
- ✅ Observability hooks (`on_migration_start`, etc.)
- ✅ Tracing integration (when `tracing` feature is enabled)

## Example

See `examples/mysql_basic.rs` for a complete working example.

To run the example:

1. Start a MySQL server (e.g., with Docker):
```bash
docker run --name mysql-test -e MYSQL_ROOT_PASSWORD=password -e MYSQL_DATABASE=testdb -p 3306:3306 -d mysql:8
```

2. Run the example:
```bash
cargo run --example mysql_basic --features mysql
```

## Implementation Notes

This is a spike/first-cut implementation that:

- **Duplicates code** from the SQLite implementation rather than trying to abstract it
- Uses separate types: `MysqlMigration`, `MysqlMigrator`, `MysqlMigrationReport`, etc.
- Keeps the implementations independent for easier maintenance and evolution

The code duplication is intentional at this stage to allow both implementations to evolve independently. Future refactoring can extract common patterns if needed.

## Testing

The MySQL implementation follows the same patterns as SQLite, but currently lacks comprehensive test coverage. To test your migrations:

1. Set up a test MySQL database
2. Run your migrations against it
3. Verify the schema and data transformations

Future work could include adding MySQL support to the `testing` feature for better migration testing utilities.
