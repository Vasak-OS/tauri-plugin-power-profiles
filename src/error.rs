use serde::Serializer;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("D-Bus error: {0}")]
    Zbus(#[from] zbus::Error),
    #[error("D-Bus error: {0}")]
    Fdo(#[from] zbus::fdo::Error),
    #[error("D-Bus variant error: {0}")]
    Zvariant(#[from] zbus::zvariant::Error),
    /// power-profiles-daemon no está instalado o no está corriendo.
    #[error("power-profiles-daemon is not available")]
    Unavailable,
    /// El perfil pedido no es uno de los que el equipo ofrece.
    #[error("unknown power profile '{0}'")]
    UnknownProfile(String),
}

impl serde::Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn se_serializa_como_texto_para_el_frontend() {
        let json = serde_json::to_string(&Error::UnknownProfile("turbo".into())).unwrap();
        assert_eq!(json, "\"unknown power profile 'turbo'\"");
    }
}
