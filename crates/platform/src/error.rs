use crate::Lang;

/// Error de una operación sobre el SO.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    /// El proceso, servicio o plan ya no existe.
    #[error("no existe: {0}")]
    NotFound(String),
    /// El PID existe pero es otro proceso (PID reutilizado): no se toca (It2.4).
    #[error("el PID {pid} ya pertenece a otro proceso")]
    TargetChanged { pid: u32 },
    /// Hace falta elevar privilegios; la GUI lo delega en el helper (It2.2).
    #[error("acceso denegado: {0}")]
    AccessDenied(String),
    /// Esta plataforma no admite la operación (ver `CapabilitySet`).
    #[error("no admitido en esta plataforma: {0}")]
    Unsupported(&'static str),
    /// Cualquier otro fallo del SO, con su mensaje.
    #[error("error del sistema: {0}")]
    Os(String),
}

impl PlatformError {
    /// El mensaje en `lang`. El detalle (nombres de API y el texto del SO) va tal cual.
    pub fn message(&self, lang: Lang) -> String {
        match (lang, self) {
            (Lang::Es, _) => self.to_string(),
            (Lang::En, PlatformError::NotFound(what)) => format!("not found: {what}"),
            (Lang::En, PlatformError::TargetChanged { pid }) => {
                format!("PID {pid} now belongs to another process")
            }
            (Lang::En, PlatformError::AccessDenied(what)) => format!("access denied: {what}"),
            (Lang::En, PlatformError::Unsupported(what)) => {
                format!("not supported on this platform: {what}")
            }
            (Lang::En, PlatformError::Os(what)) => format!("system error: {what}"),
        }
    }
}

/// Resultado de las operaciones de plataforma.
pub type Result<T> = std::result::Result<T, PlatformError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_follows_the_language_and_keeps_the_detail() {
        let e = PlatformError::AccessDenied("WMI: 0x80041003".into());
        assert_eq!(e.message(Lang::Es), "acceso denegado: WMI: 0x80041003");
        assert_eq!(e.message(Lang::En), "access denied: WMI: 0x80041003");
    }
}
