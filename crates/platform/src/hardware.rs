//! Qué cambió en el equipo (PLAN.md Iteración 6.1), común a los SO.
//!
//! Cada SO lo detecta a su manera: Windows con los mensajes difundidos a las ventanas de nivel
//! superior (`WM_DISPLAYCHANGE`, `WM_POWERBROADCAST`, `WM_DEVICECHANGE`) y Linux con los uevent
//! del kernel por netlink. La GUI relee el equipo cuando llega el aviso.

/// Qué cambió.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HardwareChange {
    /// Resolución, frecuencia o monitores.
    Display,
    /// Corriente alterna, batería o su carga.
    Power,
    /// Se conectó o desconectó un dispositivo.
    Devices,
}
