//! Las claves de `libraryfolders.vdf` y `appmanifest_*.acf` que usa FrameRush.

use keyvalues_parser::{Obj, Value};
use std::path::PathBuf;

/// Datos de un `appmanifest_<appid>.acf`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub appid: u32,
    pub name: String,
    pub installdir: String,
}

fn root(text: &str) -> Result<(String, Obj<'_>), String> {
    let vdf = keyvalues_parser::parse(text).map_err(|e| e.to_string())?;
    let key = vdf.key.to_string();
    match vdf.value {
        Value::Obj(obj) => Ok((key, obj)),
        Value::Str(_) => Err(format!("«{key}» no es un bloque")),
    }
}

fn string<'a>(obj: &'a Obj<'_>, key: &str) -> Option<&'a str> {
    obj.get(key)?.first()?.get_str()
}

/// `path` de cada biblioteca, en el orden de sus claves numéricas.
pub(crate) fn library_paths(text: &str) -> Result<Vec<PathBuf>, String> {
    let (key, obj) = root(text)?;
    if !key.eq_ignore_ascii_case("libraryfolders") {
        return Err(format!("se esperaba «libraryfolders» y hay «{key}»"));
    }
    let mut libraries: Vec<(u32, PathBuf)> = obj
        .iter()
        .filter_map(|(k, values)| {
            let index = k.parse().ok()?;
            let path = string(values.first()?.get_obj()?, "path")?;
            Some((index, PathBuf::from(path)))
        })
        .collect();
    libraries.sort_by_key(|(index, _)| *index);
    Ok(libraries.into_iter().map(|(_, path)| path).collect())
}

/// `appid`, `name` e `installdir` de un manifiesto.
pub(crate) fn manifest(text: &str) -> Result<Manifest, String> {
    let (key, obj) = root(text)?;
    if !key.eq_ignore_ascii_case("AppState") {
        return Err(format!("se esperaba «AppState» y hay «{key}»"));
    }
    let field = |k: &str| string(&obj, k).ok_or_else(|| format!("falta «{k}»"));
    let appid = field("appid")?;
    Ok(Manifest {
        appid: appid
            .parse()
            .map_err(|_| format!("appid no numérico: «{appid}»"))?,
        name: field("name")?.to_owned(),
        installdir: field("installdir")?.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extracto de `libraryfolders.vdf` de este equipo (rutas con `\\` escapadas).
    const LIBRARYFOLDERS: &str = r#""libraryfolders"
{
	"1"
	{
		"path"		"D:\\SteamLibrary"
		"apps"
		{
			"70"		"637620123"
		}
	}
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
		"apps"
		{
			"730"		"71799502864"
		}
	}
}
"#;

    #[test]
    fn library_paths_are_unescaped_and_in_order() {
        assert_eq!(
            library_paths(LIBRARYFOLDERS).unwrap(),
            [
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from(r"D:\SteamLibrary")
            ]
        );
    }

    /// Extracto de `appmanifest_730.acf` de este equipo.
    #[test]
    fn reads_a_manifest() {
        let text = r#""AppState"
{
	"appid"		"730"
	"universe"		"1"
	"LauncherPath"		"C:\\Program Files (x86)\\Steam\\steam.exe"
	"name"		"Counter-Strike 2"
	"StateFlags"		"4"
	"installdir"		"Counter-Strike Global Offensive"
	"InstalledDepots"
	{
		"2347771"
		{
			"manifest"		"4361224155535735834"
		}
	}
}
"#;
        assert_eq!(
            manifest(text).unwrap(),
            Manifest {
                appid: 730,
                name: "Counter-Strike 2".into(),
                installdir: "Counter-Strike Global Offensive".into(),
            }
        );
    }

    #[test]
    fn rejects_other_files() {
        assert!(manifest(LIBRARYFOLDERS).is_err());
        assert!(library_paths("\"AppState\" { \"appid\" \"1\" }").is_err());
        assert!(
            manifest("\"AppState\" { \"appid\" \"x\" \"name\" \"a\" \"installdir\" \"b\" }")
                .is_err()
        );
        assert!(manifest("no es vdf {").is_err());
    }
}
