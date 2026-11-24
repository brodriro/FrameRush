//! Recomendaciones manuales (`manual-recommendations`, It5.8): lo que FrameRush no puede hacer
//! por el usuario de forma segura y le pide que haga a mano. Salen de
//! `rules/recommendations.toml` (`framerush-rules` las carga y valida); aquí viven el tipo y el
//! filtro por equipo. Una condición que el equipo no permite comprobar (dato `None`) no se
//! cumple: la recomendación no se muestra (P15).

use framerush_device::DeviceProfile;
use framerush_platform::{GpuVendor, Os};
use serde::{Deserialize, Serialize};

/// Cuándo se muestra una recomendación. Un campo vacío no restringe nada.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// SO en los que aplica; vacío: en todos.
    #[serde(default)]
    pub os: Vec<Os>,
    /// Hay al menos una GPU de este fabricante.
    pub gpu_vendor: Option<GpuVendor>,
    /// El equipo es (o no es) un portátil.
    pub laptop: Option<bool>,
    /// Solo con este juego de Steam elegido.
    pub appid: Option<u32>,
}

/// Una recomendación manual: un título y los pasos a seguir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recommendation {
    pub id: String,
    pub title: String,
    pub steps: Vec<String>,
    #[serde(default)]
    pub when: Condition,
    /// De dónde sale (no inventar): se muestra en la UI.
    pub source: String,
}

impl Condition {
    fn holds(&self, device: &DeviceProfile, appid: Option<u32>) -> bool {
        (self.os.is_empty() || self.os.contains(&device.os))
            && self
                .gpu_vendor
                .is_none_or(|v| device.gpus.iter().any(|g| g.vendor == v))
            && self.laptop.is_none_or(|l| device.laptop == Some(l))
            && self.appid.is_none_or(|a| appid == Some(a))
    }
}

/// Las recomendaciones que aplican a `device` con el juego `appid` (si hay uno elegido), en
/// el orden del archivo.
pub fn recommendations_for<'a>(
    all: &'a [Recommendation],
    device: &DeviceProfile,
    appid: Option<u32>,
) -> Vec<&'a Recommendation> {
    all.iter().filter(|r| r.when.holds(device, appid)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use framerush_device::samples;

    fn rec(id: &str, when: Condition) -> Recommendation {
        Recommendation {
            id: id.into(),
            title: id.into(),
            steps: vec!["x".into()],
            when,
            source: "test".into(),
        }
    }

    fn ids(v: Vec<&Recommendation>) -> Vec<&str> {
        v.into_iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn filters_by_gpu_laptop_os_and_game() {
        let all = [
            rec("always", Condition::default()),
            rec(
                "nvidia",
                Condition {
                    gpu_vendor: Some(GpuVendor::Nvidia),
                    ..Default::default()
                },
            ),
            rec(
                "amd",
                Condition {
                    gpu_vendor: Some(GpuVendor::Amd),
                    ..Default::default()
                },
            ),
            rec(
                "laptop",
                Condition {
                    laptop: Some(true),
                    ..Default::default()
                },
            ),
            rec(
                "linux",
                Condition {
                    os: vec![Os::Linux],
                    ..Default::default()
                },
            ),
            rec(
                "cs2",
                Condition {
                    appid: Some(730),
                    ..Default::default()
                },
            ),
        ];
        let mut d = samples::hybrid_laptop();
        d.os = Os::Windows;
        d.laptop = Some(true);
        let vendors: Vec<GpuVendor> = d.gpus.iter().map(|g| g.vendor).collect();
        assert!(vendors.contains(&GpuVendor::Nvidia) && !vendors.contains(&GpuVendor::Amd));

        assert_eq!(
            ids(recommendations_for(&all, &d, None)),
            ["always", "nvidia", "laptop"]
        );
        assert_eq!(
            ids(recommendations_for(&all, &d, Some(730))),
            ["always", "nvidia", "laptop", "cs2"]
        );
    }

    #[test]
    fn unknown_data_never_matches() {
        let all = [rec(
            "laptop",
            Condition {
                laptop: Some(true),
                ..Default::default()
            },
        )];
        let d = DeviceProfile::undetected(Os::Windows);
        assert!(recommendations_for(&all, &d, None).is_empty());
    }
}
