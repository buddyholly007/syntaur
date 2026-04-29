use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("registry parse error: {0}")]
    RegistryParse(String),

    #[error("registry I/O error: {0}")]
    RegistryIo(#[from] std::io::Error),

    #[error("registry yaml error: {0}")]
    RegistryYaml(#[from] serde_yaml::Error),

    #[error("invalid glob pattern '{pattern}': {source}")]
    InvalidGlob {
        pattern: String,
        #[source]
        source: globset::Error,
    },

    #[error("dns server error: {0}")]
    DnsServer(String),

    #[error("dns resolver error: {0}")]
    DnsResolver(String),
}

pub type Result<T> = std::result::Result<T, Error>;
