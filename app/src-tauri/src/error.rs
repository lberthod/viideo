use serde::{Serialize, Serializer};

/// Erreur renvoyée au frontend : toujours sérialisée en message lisible.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Msg(String),
    #[error("Erreur de fichier : {0}")]
    Io(#[from] std::io::Error),
    #[error("Erreur JSON : {0}")]
    Json(#[from] serde_json::Error),
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

pub fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(AppError::Msg(msg.into()))
}
