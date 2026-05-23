use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("discord: {0}")]
    Discord(#[from] serenity::Error),

    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml de: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("toml ser: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("crypto: {0}")]
    Crypto(String),

    #[error("chunk size {got} exceeds discord limit ({limit})")]
    ChunkTooLarge { got: usize, limit: usize },

    #[error("inode not found: {0}")]
    InodeNotFound(String),

    #[error("integrity check failed for chunk {0}")]
    IntegrityFailure(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
