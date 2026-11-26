//! Monitores y Hz por DRM/KMS, sin libdrm.
//!
//! `/sys/class/drm` dice qué conectores están conectados y qué modos admiten, pero no cuál está
//! puesto ni a cuántos Hz; eso solo lo da el KMS. Se habla con `/dev/dri/cardN` por `ioctl(2)`
//! usando los structs de `drm.h` y `drm_mode.h` del kernel:
//! - `DRM_IOCTL_MODE_GETRESOURCES` (0xA0): conectores y CRTC de la tarjeta.
//! - `DRM_IOCTL_MODE_GETCONNECTOR` (0xA7): si está conectado (`connection == DRM_MODE_CONNECTED`
//!   = 1), su codificador actual y los modos que admite (`drm_mode_modeinfo`).
//! - `DRM_IOCTL_MODE_GETENCODER` (0xA6) y `DRM_IOCTL_MODE_GETCRTC` (0xA1): el CRTC puesto y su
//!   modo (`mode_valid` + `mode`), que es el modo actual del monitor.
//! - `drm_mode_modeinfo.vrefresh` da los Hz (si viene 0 se calculan con `clock`, `htotal` y
//!   `vtotal`, como hace el kernel).
//!
//! El usuario de la sesión activa puede abrir esos nodos sin ser root: systemd-logind le pone una
//! ACL en `/dev/dri/card*` (drivers/gpu/drm/drm_sysfs.c los crea como `root:video` 0660 y logind
//! añade el usuario). Sin ese permiso, la tarjeta se salta y la lista queda vacía.
//!
//! DRM no dice cuál es el monitor «principal» del escritorio (lo decide el compositor), así que
//! ninguno se marca como tal: quien lo necesite usa el primero, como hace `--resolution`.

#![allow(unsafe_code)]

use crate::{Display, Mode, PlatformError, Result};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

const GETRESOURCES: u32 = 0xA0;
const GETCRTC: u32 = 0xA1;
const GETENCODER: u32 = 0xA6;
const GETCONNECTOR: u32 = 0xA7;

/// `DRM_MODE_CONNECTED` de `drm_mode.h`.
const CONNECTED: u32 = 1;

#[repr(C)]
#[derive(Default)]
struct CardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

/// `struct drm_mode_modeinfo` (`DRM_DISPLAY_MODE_LEN` = 32).
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ModeInfo {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    type_: u32,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Default)]
struct Connector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct Encoder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
#[derive(Default)]
struct Crtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: ModeInfo,
}

/// `ioctl(2)` con el número que arma `drm.h`: `_IOWR('d', nr, T)` (el tamaño sale de `T`).
fn request<T>(fd: libc::c_int, nr: u32, arg: &mut T) -> std::io::Result<()> {
    // SAFETY: `arg` es un struct `#[repr(C)]` que vive durante la llamada y del tamaño que el
    // ioctl espera (`libc::_IOWR::<T>` usa `size_of::<T>()`).
    let r = unsafe { libc::ioctl(fd, libc::_IOWR::<T>(u32::from(b'd'), nr), arg as *mut T) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Monitores conectados de todas las tarjetas DRM, ordenados por tarjeta y conector.
pub fn displays() -> Result<Vec<Display>> {
    let mut out = vec![];
    for (n, card) in cards() {
        let Some(file) = open(&card) else {
            // Sin permisos (sesión no activa, contenedor) esa tarjeta no cuenta.
            continue;
        };
        let fd = file.as_raw_fd();
        // Una tarjeta sin KMS (vgem, simpledrm…) no invalida las demás.
        let Ok(ids) = connector_ids(fd) else {
            continue;
        };
        card_displays(fd, n, ids, &mut out)
            .map_err(|e| PlatformError::Os(format!("{}: {e}", card.display())))?;
    }
    Ok(out)
}

/// `/dev/dri/cardN` (no los nodos `renderD*`), con su número.
fn cards() -> Vec<(u32, PathBuf)> {
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return vec![];
    };
    let mut cards: Vec<(u32, PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let n = entry
                .file_name()
                .to_str()?
                .strip_prefix("card")?
                .parse()
                .ok()?;
            Some((n, entry.path()))
        })
        .collect();
    cards.sort();
    cards
}

fn open(card: &std::path::Path) -> Option<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(card)
        .ok()
}

/// Conectores (`DRM_IOCTL_MODE_GETRESOURCES`) de la tarjeta.
fn connector_ids(fd: libc::c_int) -> std::io::Result<Vec<u32>> {
    let mut res = CardRes::default();
    request(fd, GETRESOURCES, &mut res)?;
    let mut connectors = vec![0u32; res.count_connectors as usize];
    let mut crtcs = vec![0u32; res.count_crtcs as usize];
    let mut res = CardRes {
        crtc_id_ptr: crtcs.as_mut_ptr() as u64,
        connector_id_ptr: connectors.as_mut_ptr() as u64,
        count_crtcs: res.count_crtcs,
        count_connectors: res.count_connectors,
        ..CardRes::default()
    };
    request(fd, GETRESOURCES, &mut res)?;
    Ok(connectors)
}

/// Conector con sus modos (`DRM_IOCTL_MODE_GETCONNECTOR`): dos llamadas, la primera para saber
/// cuántos modos hay y la segunda para traerlos.
fn connector(fd: libc::c_int, id: u32) -> std::io::Result<(Connector, Vec<ModeInfo>)> {
    let mut conn = Connector {
        connector_id: id,
        ..Connector::default()
    };
    request(fd, GETCONNECTOR, &mut conn)?;
    // La segunda llamada necesita memoria para los modos y para los codificadores (el kernel
    // copia los dos con `memdup_user`); las propiedades no se piden.
    let mut modes = vec![ModeInfo::default(); conn.count_modes as usize];
    let mut encoders = vec![0u32; conn.count_encoders as usize];
    conn.modes_ptr = modes.as_mut_ptr() as u64;
    conn.encoders_ptr = encoders.as_mut_ptr() as u64;
    conn.count_props = 0;
    conn.props_ptr = 0;
    conn.prop_values_ptr = 0;
    request(fd, GETCONNECTOR, &mut conn)?;
    Ok((conn, modes))
}

/// Modo puesto en el CRTC del codificador actual (`GETENCODER` + `GETCRTC`).
fn current(fd: libc::c_int, encoder_id: u32) -> std::io::Result<Option<ModeInfo>> {
    if encoder_id == 0 {
        return Ok(None);
    }
    let mut encoder = Encoder {
        encoder_id,
        ..Encoder::default()
    };
    request(fd, GETENCODER, &mut encoder)?;
    if encoder.crtc_id == 0 {
        return Ok(None);
    }
    let mut crtc = Crtc {
        crtc_id: encoder.crtc_id,
        ..Crtc::default()
    };
    request(fd, GETCRTC, &mut crtc)?;
    Ok((crtc.mode_valid != 0).then_some(crtc.mode))
}

fn card_displays(
    fd: libc::c_int,
    card: u32,
    ids: Vec<u32>,
    out: &mut Vec<Display>,
) -> std::io::Result<()> {
    for id in ids {
        let (conn, modes) = connector(fd, id)?;
        if conn.connection != CONNECTED {
            continue;
        }
        let Some(current) = current(fd, conn.encoder_id)?.map(|m| mode(&m)) else {
            continue;
        };
        let mut modes: Vec<Mode> = modes.iter().map(mode).collect();
        modes.dedup();
        out.push(Display {
            id: format!(
                "card{card}-{}",
                connector_name(conn.connector_type, conn.connector_type_id)
            ),
            current,
            modes,
            // DRM no dice cuál es el principal del escritorio.
            primary: false,
        });
    }
    Ok(())
}

/// Nombre del conector como lo llama el kernel en `/sys/class/drm` (`HDMI-A-1`, `eDP-1`…):
/// `drm_connector_enum_list` de `drivers/gpu/drm/drm_connector.c`.
fn connector_name(kind: u32, index: u32) -> String {
    let name = match kind {
        1 => "VGA",
        2 => "DVI-I",
        3 => "DVI-D",
        4 => "DVI-A",
        5 => "Composite",
        6 => "SVIDEO",
        7 => "LVDS",
        8 => "Component",
        9 => "DIN",
        10 => "DP",
        11 => "HDMI-A",
        12 => "HDMI-B",
        13 => "TV",
        14 => "eDP",
        15 => "Virtual",
        16 => "DSI",
        17 => "DPI",
        18 => "Writeback",
        19 => "SPI",
        20 => "USB",
        _ => "Unknown",
    };
    format!("{name}-{index}")
}

/// `Mode` de un `drm_mode_modeinfo`, con los Hz que da el kernel o, si vienen a 0, los que salen
/// de `clock / (htotal * vtotal)` (el reloj va en kHz).
fn mode(info: &ModeInfo) -> Mode {
    let pixels = u64::from(info.htotal) * u64::from(info.vtotal);
    let hz = if info.vrefresh > 0 || pixels == 0 {
        info.vrefresh
    } else {
        (u64::from(info.clock) * 1000 / pixels) as u32
    };
    Mode {
        width: u32::from(info.hdisplay),
        height: u32::from(info.vdisplay),
        hz,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(hdisplay: u16, vdisplay: u16, vrefresh: u32, clock: u32) -> ModeInfo {
        ModeInfo {
            hdisplay,
            vdisplay,
            vrefresh,
            clock,
            htotal: 2200,
            vtotal: 1125,
            ..ModeInfo::default()
        }
    }

    #[test]
    fn names_connectors_like_sysfs() {
        assert_eq!(connector_name(11, 1), "HDMI-A-1");
        assert_eq!(connector_name(14, 1), "eDP-1");
        assert_eq!(connector_name(10, 2), "DP-2");
        assert_eq!(connector_name(99, 3), "Unknown-3");
    }

    #[test]
    fn the_refresh_rate_comes_from_the_kernel_or_from_the_clock() {
        assert_eq!(mode(&info(1920, 1080, 144, 0)).hz, 144);
        // 148 500 kHz / (2200 × 1125) = 60 Hz.
        assert_eq!(mode(&info(1920, 1080, 0, 148_500)).hz, 60);
        assert_eq!(mode(&ModeInfo::default()).hz, 0);
        assert_eq!(mode(&info(1920, 1080, 0, 0)).hz, 0);
    }

    /// En este equipo: los monitores que haya, con su modo actual (informativo).
    #[test]
    fn reads_this_machine() {
        for d in displays().unwrap() {
            println!(
                "{}: {}x{} @ {} Hz ({} modos)",
                d.id,
                d.current.width,
                d.current.height,
                d.current.hz,
                d.modes.len()
            );
        }
    }
}
