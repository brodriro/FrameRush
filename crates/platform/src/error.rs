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

/// Resultado de las operaciones de plataforma.
pub type Result<T> = std::result::Result<T, PlatformError>;
