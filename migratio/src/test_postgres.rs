#![allow(dead_code)]

//! PostgreSQL test infrastructure module.
//!
//! This module provides shared infrastructure for PostgreSQL integration tests,
//! including testcontainer management and database setup utilities.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Once;

use postgres::{Client, NoTls};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

/// Global PostgreSQL container port, set once the container is started
static POSTGRES_PORT: AtomicU16 = AtomicU16::new(0);

/// Ensures the container is started only once
static POSTGRES_INIT: Once = Once::new();

/// Tokio runtime for container management (kept alive for container lifecycle)
static mut TOKIO_RT: Option<tokio::runtime::Runtime> = None;

/// Default credentials for testcontainers-modules postgres
const PG_USER: &str = "postgres";
const PG_PASSWORD: &str = "postgres";
const PG_DB: &str = "postgres";

/// Initialize the shared PostgreSQL container.
///
/// This function starts a PostgreSQL container using testcontainers and stores
/// the mapped port for use by tests. The container is kept alive for the duration
/// of the test run.
fn ensure_postgres_started() {
    POSTGRES_INIT.call_once(|| {
        // Create a dedicated tokio runtime for container management
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

        let port = rt.block_on(async {
            let container = Postgres::default()
                .start()
                .await
                .expect("failed to start postgres container");

            let port = container
                .get_host_port_ipv4(5432)
                .await
                .expect("failed to get postgres port");

            // Leak the container to keep it alive for the test duration
            // (the dtor will clean up via the runtime)
            std::mem::forget(container);

            port
        });

        POSTGRES_PORT.store(port, Ordering::SeqCst);

        // Store the runtime to keep it alive (and the container with it)
        // Safety: This is only called once via Once::call_once
        unsafe {
            TOKIO_RT = Some(rt);
        }
    });
}

/// Get the PostgreSQL port for the shared container.
fn get_postgres_port() -> u16 {
    ensure_postgres_started();
    POSTGRES_PORT.load(Ordering::SeqCst)
}

/// Get the base PostgreSQL connection URL for the shared container.
fn postgres_base_url() -> String {
    let port = get_postgres_port();
    format!(
        "postgres://{}:{}@127.0.0.1:{}/{}",
        PG_USER, PG_PASSWORD, port, PG_DB
    )
}

/// Get a PostgreSQL connection URL for a specific database.
fn url_with_db(db: &str) -> String {
    let port = get_postgres_port();
    format!(
        "postgres://{}:{}@127.0.0.1:{}/{}",
        PG_USER, PG_PASSWORD, port, db
    )
}

/// Create a fresh PostgreSQL database with a unique name for isolated testing.
///
/// This function:
/// - Generates a unique database name using UUID
/// - Creates the database
/// - Returns a Client connected to the new database and the database name
///
/// Each test should call this to get an isolated database instance.
pub fn fresh_postgres_db() -> (Client, String) {
    // Connect to default DB as admin
    let admin_url = postgres_base_url();
    let mut admin = Client::connect(&admin_url, NoTls).expect("failed to connect as admin");

    // Create a DB with unique name (PostgreSQL identifiers are case-insensitive, so lowercase is fine)
    let db_name = format!("test_{}", Uuid::new_v4().simple());

    admin
        .execute(&format!("CREATE DATABASE \"{}\"", db_name), &[])
        .expect("failed to create test database");

    // Close admin connection before creating new one
    drop(admin);

    // Connect to the fresh DB
    let test_url = url_with_db(&db_name);
    let client = Client::connect(&test_url, NoTls).expect("failed to connect to test database");

    (client, db_name)
}

/// Get a test database client.
///
/// This is a convenience function that creates a fresh database and returns
/// a Client ready for use in tests.
///
/// # Returns
/// A Client connected to a fresh, isolated database
pub fn get_test_client() -> Client {
    let (client, _db_name) = fresh_postgres_db();
    client
}
