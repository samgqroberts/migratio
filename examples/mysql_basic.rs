// This example demonstrates basic MySQL migration usage.
// To run this example, you'll need a MySQL server running.
//
// You can start one with Docker:
// docker run --name mysql-test -e MYSQL_ROOT_PASSWORD=password -e MYSQL_DATABASE=testdb -p 3306:3306 -d mysql:8
//
// Then run:
// cargo run --example mysql_basic --features mysql

#[cfg(feature = "mysql")]
fn main() {
    use migratio::{Error, MysqlMigration, MysqlMigrator};
    use mysql::prelude::*;
    use mysql::{Conn, OptsBuilder, Transaction};

    // Define your migrations as structs that implement the MysqlMigration trait
    struct Migration1;
    impl MysqlMigration for Migration1 {
        fn version(&self) -> u32 {
            1
        }
        fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
            tx.query_drop(
                "CREATE TABLE users (id INT PRIMARY KEY AUTO_INCREMENT, name VARCHAR(255))",
            )?;
            Ok(())
        }
        fn down(&self, tx: &mut Transaction) -> Result<(), Error> {
            tx.query_drop("DROP TABLE users")?;
            Ok(())
        }
        fn name(&self) -> String {
            "create_users_table".to_string()
        }
    }

    struct Migration2;
    impl MysqlMigration for Migration2 {
        fn version(&self) -> u32 {
            2
        }
        fn up(&self, tx: &mut Transaction) -> Result<(), Error> {
            tx.query_drop("ALTER TABLE users ADD COLUMN email VARCHAR(255)")?;
            Ok(())
        }
        fn down(&self, tx: &mut Transaction) -> Result<(), Error> {
            tx.query_drop("ALTER TABLE users DROP COLUMN email")?;
            Ok(())
        }
        fn name(&self) -> String {
            "add_email_to_users".to_string()
        }
    }

    // Construct a migrator with migrations
    let migrator = MysqlMigrator::new(vec![Box::new(Migration1), Box::new(Migration2)]);

    // Connect to your database
    let opts = OptsBuilder::new()
        .ip_or_hostname(Some("127.0.0.1"))
        .tcp_port(3306)
        .user(Some("root"))
        .pass(Some("password"))
        .db_name(Some("testdb"));

    let mut conn = match Conn::new(opts) {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("Failed to connect to MySQL: {}", e);
            eprintln!("Make sure you have MySQL running. See the comment at the top of this file.");
            return;
        }
    };

    // Run the migrations, receiving a report of the results
    println!("Running migrations...");
    let report = migrator.upgrade(&mut conn).expect("Migration failed");

    println!("Migration report:");
    println!(
        "  Schema version table existed: {}",
        report.schema_version_table_existed
    );
    println!(
        "  Schema version table created: {}",
        report.schema_version_table_created
    );
    println!("  Migrations run: {:?}", report.migrations_run);
    if let Some(failure) = report.failing_migration {
        println!("  Failing migration: {:?}", failure.migration().version());
        println!("  Error: {}", failure.error());
    } else {
        println!("  All migrations completed successfully!");
    }

    // Get current version
    let current_version = migrator
        .get_current_version(&mut conn)
        .expect("Failed to get version");
    println!("\nCurrent database version: {}", current_version);

    // Get migration history
    let history = migrator
        .get_migration_history(&mut conn)
        .expect("Failed to get history");
    println!("\nMigration history:");
    for applied in history {
        println!(
            "  Version {}: {} (applied at {})",
            applied.version, applied.name, applied.applied_at
        );
    }

    println!("\nExample completed successfully!");
}

#[cfg(not(feature = "mysql"))]
fn main() {
    println!("This example requires the 'mysql' feature to be enabled.");
    println!("Run with: cargo run --example mysql_basic --features mysql");
}
