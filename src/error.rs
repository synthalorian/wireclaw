//! Custom error types for ledger.

use std::fmt;

#[derive(Debug)]
pub enum LedgerError {
    Proxy(String),
    Database(String),
    InvalidAddress(String),
    SessionNotFound(String),
    RequestNotFound(String),
    Export(String),
    Config(String),
    Io(std::io::Error),
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proxy(msg) => write!(f, "proxy error: {msg}"),
            Self::Database(msg) => write!(f, "database error: {msg}"),
            Self::InvalidAddress(addr) => write!(f, "invalid address format: {addr}"),
            Self::SessionNotFound(name) => write!(f, "session not found: {name}"),
            Self::RequestNotFound(id) => write!(f, "request not found: {id}"),
            Self::Export(msg) => write!(f, "export failed: {msg}"),
            Self::Config(msg) => write!(f, "config error: {msg}"),
            Self::Io(err) => write!(f, "IO error: {err}"),
        }
    }
}

impl std::error::Error for LedgerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LedgerError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<sqlx::Error> for LedgerError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err.to_string())
    }
}
