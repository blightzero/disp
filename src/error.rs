use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("EDID parsing error: {0}")]
    EdidParsing(String),

    #[error("Display configuration error: {0}")]
    DisplayConfig(String),

    #[error("No matching configuration found")]
    NoMatchingConfig,

    #[error("Platform-specific error: {0}")]
    PlatformSpecific(String),

    #[error("TOML parsing error: {0}")]
    TomlParsing(#[from] toml::de::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
