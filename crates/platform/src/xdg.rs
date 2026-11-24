//! Lectura de `user-dirs.dirs` (xdg-user-dirs, freedesktop.org).
//!
//! Fuentes:
//! - Página de manual user-dirs.dirs(5): líneas `XDG_NAME_DIR=VALUE`, con `VALUE` de la forma
//!   `"$HOME/Path"` o `"/Path"`; «Lines beginning with a # character are ignored»; el formato
//!   está pensado para hacer `source` del archivo desde un script de shell.
//! - Wiki de xdg-user-dirs (<https://www.freedesktop.org/wiki/Software/xdg-user-dirs/>): «To
//!   disable a directory, point it to the homedir».
//! - POSIX Shell Command Language §2.2.3 (comillas dobles): dentro de `"…"` la barra invertida
//!   solo escapa `$`, `` ` ``, `"`, `\` y el salto de línea.

use std::path::PathBuf;

/// Ruta de la entrada `key` (p. ej. `XDG_DOWNLOAD_DIR`) en el texto de `user-dirs.dirs`, con
/// `$HOME` sustituido por `home`. `None` si la entrada falta, no tiene el formato del manual o
/// apunta a la carpeta personal (desactivada según la wiki).
pub(crate) fn user_dir(text: &str, key: &str, home: &str) -> Option<PathBuf> {
    // Si la clave se repite, gana la última asignación, igual que al hacer `source` del archivo.
    let value = text
        .lines()
        .map(str::trim_start)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .next_back()?;
    let raw = unquote(value.trim_end())?;
    let home = home.trim_end_matches('/');
    let path = if let Some(rest) = raw.strip_prefix("$HOME") {
        // Solo `$HOME` solo o seguido de `/`: `$HOMEX` sería otra variable.
        if !rest.is_empty() && !rest.starts_with('/') {
            return None;
        }
        format!("{home}/{}", rest.trim_start_matches('/'))
    } else if raw.starts_with('/') {
        raw
    } else {
        return None;
    };
    if path.trim_end_matches('/') == home {
        return None;
    }
    Some(PathBuf::from(path))
}

/// Carpeta base de la XDG Base Directory Specification: `value` (p. ej. `$XDG_DATA_HOME`) si
/// es una ruta absoluta o, si no está, está vacía o es relativa («should consider the path invalid
/// and ignore it»), `home` + `default` (p. ej. `.local/share`).
pub(crate) fn base_dir(value: Option<&str>, home: &str, default: &str) -> PathBuf {
    match value.filter(|v| v.starts_with('/')) {
        Some(v) => PathBuf::from(v),
        None => PathBuf::from(format!("{}/{default}", home.trim_end_matches('/'))),
    }
}

/// Contenido de un valor entre comillas dobles, sin las barras de escape de POSIX §2.2.3.
fn unquote(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(next @ ('$' | '`' | '"' | '\\')) => out.push(next),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => return None,
            },
            // Una comilla sin escapar cerraría la cadena antes de tiempo: formato no válido.
            '"' => return None,
            _ => out.push(c),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_dir_uses_an_absolute_value_or_the_default() {
        let dir = |v| base_dir(v, "/home/ana/", ".local/share");
        assert_eq!(dir(Some("/datos")), PathBuf::from("/datos"));
        assert_eq!(dir(None), PathBuf::from("/home/ana/.local/share"));
        assert_eq!(dir(Some("")), PathBuf::from("/home/ana/.local/share"));
        assert_eq!(dir(Some("rel/x")), PathBuf::from("/home/ana/.local/share"));
    }

    const KEY: &str = "XDG_DOWNLOAD_DIR";
    const HOME: &str = "/home/ana";

    fn dir(text: &str) -> Option<PathBuf> {
        user_dir(text, KEY, HOME)
    }

    #[test]
    fn expands_home() {
        let text = "XDG_DESKTOP_DIR=\"$HOME/Escritorio\"\nXDG_DOWNLOAD_DIR=\"$HOME/Descargas\"\n";
        assert_eq!(dir(text), Some(PathBuf::from("/home/ana/Descargas")));
    }

    #[test]
    fn keeps_absolute_path() {
        assert_eq!(
            dir("XDG_DOWNLOAD_DIR=\"/datos/descargas\""),
            Some(PathBuf::from("/datos/descargas"))
        );
    }

    #[test]
    fn requires_double_quotes() {
        assert_eq!(dir("XDG_DOWNLOAD_DIR=$HOME/Downloads"), None);
        assert_eq!(dir("XDG_DOWNLOAD_DIR='$HOME/Downloads'"), None);
        assert_eq!(dir("XDG_DOWNLOAD_DIR=\"$HOME/Downloads"), None);
    }

    #[test]
    fn unescapes_shell_quotes() {
        assert_eq!(
            dir(r#"XDG_DOWNLOAD_DIR="$HOME/Mis \"Descargas\" \$x""#),
            Some(PathBuf::from("/home/ana/Mis \"Descargas\" $x"))
        );
        assert_eq!(
            dir(r#"XDG_DOWNLOAD_DIR="$HOME/Mis Descargas""#),
            Some(PathBuf::from("/home/ana/Mis Descargas"))
        );
    }

    #[test]
    fn ignores_comments() {
        let text = "# XDG_DOWNLOAD_DIR=\"$HOME/Viejo\"\n  # otro\nXDG_DOWNLOAD_DIR=\"$HOME/Nuevo\"";
        assert_eq!(dir(text), Some(PathBuf::from("/home/ana/Nuevo")));
        assert_eq!(dir("#XDG_DOWNLOAD_DIR=\"$HOME/Viejo\""), None);
    }

    #[test]
    fn missing_entry_is_none() {
        assert_eq!(dir("XDG_DESKTOP_DIR=\"$HOME/Desktop\"\n"), None);
        assert_eq!(dir(""), None);
        // Otra clave que empieza igual no cuenta.
        assert_eq!(dir("XDG_DOWNLOAD_DIRS=\"$HOME/x\""), None);
    }

    #[test]
    fn home_means_disabled() {
        assert_eq!(dir("XDG_DOWNLOAD_DIR=\"$HOME/\""), None);
        assert_eq!(dir("XDG_DOWNLOAD_DIR=\"$HOME\""), None);
        assert_eq!(dir("XDG_DOWNLOAD_DIR=\"/home/ana\""), None);
    }

    #[test]
    fn rejects_relative_and_other_vars() {
        assert_eq!(dir("XDG_DOWNLOAD_DIR=\"Downloads\""), None);
        assert_eq!(dir("XDG_DOWNLOAD_DIR=\"$HOMEX/Downloads\""), None);
    }

    #[test]
    fn last_assignment_wins() {
        let text = "XDG_DOWNLOAD_DIR=\"$HOME/A\"\nXDG_DOWNLOAD_DIR=\"$HOME/B\"";
        assert_eq!(dir(text), Some(PathBuf::from("/home/ana/B")));
    }
}
