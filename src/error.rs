use thiserror::Error;

#[derive(Error, Debug)]
pub enum TriviumError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),

    #[error("Vector dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },

    #[error("Node not found: {0}")]
    NodeNotFound(u64),

    #[error("Database error: {0}")]
    Generic(String),
}

pub type Result<T> = std::result::Result<T, TriviumError>;
