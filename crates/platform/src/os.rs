use serde::{Deserialize, Serialize};

/// Sistema operativo. Los nombres serializados son los del `match` de las reglas TOML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Windows,
    Linux,
    Macos,
}

impl Os {
    /// SO para el que se compiló, o `None` si no es ninguno de los tres admitidos.
    pub const fn current() -> Option<Os> {
        if cfg!(target_os = "windows") {
            Some(Os::Windows)
        } else if cfg!(target_os = "linux") {
            Some(Os::Linux)
        } else if cfg!(target_os = "macos") {
            Some(Os::Macos)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_os_is_supported_on_ci_targets() {
        if cfg!(any(
            target_os = "windows",
            target_os = "linux",
            target_os = "macos"
        )) {
            assert!(Os::current().is_some());
        }
    }
}
