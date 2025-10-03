/// Error type for the migratio crate.
#[derive(thiserror::Error, Debug, PartialEq)]
pub enum Error {
    #[error("{0}")]
    Rusqlite(rusqlite::Error),
    #[error("{0}")]
    Generic(String),
}

impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        Self::Rusqlite(value)
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Generic(value)
    }
}
