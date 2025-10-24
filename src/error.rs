/// Error type for the migratio crate.
#[derive(thiserror::Error, Debug, PartialEq)]
pub enum Error {
    #[error("{0}")]
    Rusqlite(rusqlite::Error),
    #[cfg(feature = "mysql")]
    #[error("{0}")]
    Mysql(String),
    #[error("{0}")]
    Generic(String),
}

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
