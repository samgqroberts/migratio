/// Error type for the migratio crate.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[cfg(feature = "sqlite")]
    #[error("{0}")]
    Rusqlite(rusqlite::Error),
    #[cfg(feature = "mysql")]
    #[error("{0}")]
    Mysql(String),
    #[cfg(feature = "postgres")]
    #[error("{0}")]
    Postgres(#[from] postgres::Error),
    #[error("{0}")]
    Generic(String),
}

#[cfg(feature = "sqlite")]
impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        Self::Rusqlite(value)
    }
}

#[cfg(feature = "mysql")]
impl From<mysql::Error> for Error {
    fn from(value: mysql::Error) -> Self {
        Self::Mysql(value.to_string())
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Generic(value)
    }
}

// Manual PartialEq implementation because postgres::Error doesn't implement PartialEq
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            #[cfg(feature = "sqlite")]
            (Self::Rusqlite(a), Self::Rusqlite(b)) => a == b,
            #[cfg(feature = "mysql")]
            (Self::Mysql(a), Self::Mysql(b)) => a == b,
            #[cfg(feature = "postgres")]
            (Self::Postgres(a), Self::Postgres(b)) => a.to_string() == b.to_string(),
            (Self::Generic(a), Self::Generic(b)) => a == b,
            _ => false,
        }
    }
}
