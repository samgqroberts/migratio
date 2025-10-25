//! MySQL test infrastructure module.
//!
//! This module provides shared infrastructure for MySQL integration tests,
//! including testcontainer management and database setup utilities.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard};
use std::time::Duration;

use mysql::prelude::*;
use mysql::{Conn, Opts, Pool, PooledConn};
use testcontainers::core::logs::LogFrame;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use uuid::Uuid;

/// Global MySQL container instance shared across all tests
static MYSQL_INNER: RwLock<Option<ContainerAsync<GenericImage>>> = RwLock::new(None);

/// Get or create the shared MySQL container instance.
///
/// This function ensures only one MySQL container is created and shared across all tests
/// in the test run, improving test performance.
async fn mysql() -> RwLockReadGuard<'static, Option<ContainerAsync<GenericImage>>> {
    {
        let mut mysql = MYSQL_INNER.write().unwrap();
        if mysql.is_none() {
            let created = create_mysql_image_async().await;
            *mysql = Some(created);
        }
    }
    MYSQL_INNER.read().unwrap()
}

/// Create and start a MySQL Docker container using testcontainers.
///
/// This function:
/// - Starts a MySQL 8.4 container
/// - Waits for MySQL to be fully ready
/// - Configures max_connections to 1000 to support concurrent tests
async fn create_mysql_image_async() -> ContainerAsync<GenericImage> {
    let temporary_server_started = Arc::new(AtomicBool::new(false));
    let mysql_ready = Arc::new(AtomicBool::new(false));
    let temp_clone = Arc::clone(&temporary_server_started);
    let mysql_clone = Arc::clone(&mysql_ready);

    let log_consumer = move |log: &LogFrame| {
        let msg = format!("{:?}", log);
        if msg.contains("Temporary server started") {
            temp_clone.store(true, Ordering::SeqCst);
        } else if temp_clone.load(Ordering::SeqCst)
            && msg.contains("/usr/sbin/mysqld: ready for connections")
        {
            mysql_clone.store(true, Ordering::SeqCst);
        }
    };

    let image = GenericImage::new("mysql", "8.4")
        .with_log_consumer(log_consumer)
        .with_env_var("MYSQL_ROOT_PASSWORD", "rootpw")
        .with_env_var("MYSQL_DATABASE", "bootstrap");

    let started = AsyncRunner::start(image)
        .await
        .expect("failed to start mysql docker image");

    // Wait for MySQL to be fully ready
    while !mysql_ready.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Configure max connections to support concurrent tests
    let port = started.get_host_port_ipv4(3306).await.unwrap();
    let admin_url = format!("mysql://root:rootpw@127.0.0.1:{port}");
    let admin_pool =
        Pool::new(Opts::from_url(&admin_url).expect("parse admin url")).expect("create admin pool");
    let mut admin: PooledConn = admin_pool.get_conn().expect("failed to get admin conn");
    admin
        .query_drop("SET GLOBAL max_connections = 1000")
        .expect("failed to set max connections");

    started
}

/// Get the base MySQL connection URL for the shared container.
async fn mysql_base_url() -> String {
    let port = mysql()
        .await
        .as_ref()
        .unwrap()
        .get_host_port_ipv4(3306)
        .await
        .unwrap();
    format!("mysql://root:rootpw@127.0.0.1:{port}")
}

/// Get a MySQL connection URL for a specific database.
async fn url_with_db(db: &str) -> String {
    format!("{}/{}", mysql_base_url().await, db)
}

/// Create a fresh MySQL database with a unique name for isolated testing.
///
/// This function:
/// - Generates a unique database name using UUID
/// - Creates the database with utf8mb4 encoding
/// - Returns a Pool connected to the new database and the database name
///
/// Each test should call this to get an isolated database instance.
pub async fn fresh_mysql_db() -> (Pool, String) {
    // Connect to bootstrap DB as admin
    let admin_url = url_with_db("bootstrap").await;
    let admin_pool =
        Pool::new(Opts::from_url(&admin_url).expect("parse admin url")).expect("create admin pool");
    let mut admin: PooledConn = admin_pool.get_conn().expect("failed to get admin conn");

    // Create a DB with utf8mb4; use a safe name
    let db_name = format!("test_{}", Uuid::new_v4().simple());

    // Try MySQL 8 collation first, fall back to MariaDB-compatible collation
    let create_stmt = format!(
        "CREATE DATABASE `{}` CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci",
        db_name
    );
    if admin.exec_drop(&create_stmt, ()).is_err() {
        // Fallback for MariaDB
        admin
            .exec_drop(
                format!(
                    "CREATE DATABASE `{}` CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci",
                    db_name
                ),
                (),
            )
            .expect("create db (fallback)");
    }

    // Connect pool to the fresh DB
    let url = url_with_db(&db_name).await;
    let pool = Pool::new(Opts::from_url(&url).expect("parse test url")).expect("create test pool");

    (pool, db_name)
}

/// Get a test database connection.
///
/// This is a convenience function that creates a fresh database and returns
/// a Pool and connection ready for use in tests.
///
/// # Returns
/// A tuple of (Pool, Conn) where:
/// - Pool keeps the connection pool alive
/// - Conn is ready to use for migrations
pub async fn get_test_conn() -> (Pool, Conn) {
    let (pool, _db_name) = fresh_mysql_db().await;
    let conn = pool.get_conn().unwrap().unwrap();
    (pool, conn)
}

#[ctor::dtor]
fn stop_shared_mysql() {
    let Ok(rt) = tokio::runtime::Runtime::new() else {
        return;
    };
    rt.block_on(async {
        if let Some(c) = MYSQL_INNER.write().unwrap().take() {
            drop(c);
        }
    })
}
