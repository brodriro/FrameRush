//! Base de reglas en TOML (embebida + override del usuario) y su validación (PLAN.md §3).
//!
//! Añadir procesos es un cambio de datos en `rules/*.toml`, no de código. Un archivo del
//! usuario con el mismo `id` sustituye a la regla embebida, y un `id` nuevo se añade. El
//! resultado se valida entero: la lista de protegidos y los invariantes de `core` no se pueden
//! anular desde TOML.

use framerush_core::{Action, Category, Level, Recommendation, Risk, Rule, is_protected};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

/// Archivos de `rules/` embebidos en el binario: `(nombre, contenido)`.
const EMBEDDED: &[(&str, &str)] = &[
    ("browser.toml", include_str!("../../../rules/browser.toml")),
    (
        "launcher.toml",
        include_str!("../../../rules/launcher.toml"),
    ),
    ("overlay.toml", include_str!("../../../rules/overlay.toml")),
    ("service.toml", include_str!("../../../rules/service.toml")),
    ("sync.toml", include_str!("../../../rules/sync.toml")),
];

/// Recomendaciones manuales (`manual-recommendations`, It5.8), embebidas aparte: no son
/// reglas del checklist.
const RECOMMENDATIONS: &str = include_str!("../../../rules/recommendations.toml");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecommendationFile {
    #[serde(default)]
    recommendation: Vec<Recommendation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    #[serde(default)]
    rule: Vec<Rule>,
}

/// Un problema de validación en una regla.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub rule_id: String,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.rule_id, self.message)
    }
}

/// Error al cargar las reglas.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RulesError {
    #[error("{file}: {message}")]
    Parse { file: String, message: String },
    #[error("{}: {message}", path.display())]
    Io { path: PathBuf, message: String },
    #[error("reglas inválidas:\n{}", .0.iter().map(|p| format!("  - {p}")).collect::<Vec<_>>().join("\n"))]
    Invalid(Vec<Problem>),
}

/// Lee un archivo de reglas.
pub fn parse(file: &str, text: &str) -> Result<Vec<Rule>, RulesError> {
    toml::from_str::<RuleFile>(text)
        .map(|f| f.rule)
        .map_err(|e| RulesError::Parse {
            file: file.to_owned(),
            message: e.to_string(),
        })
}

/// Aplica los overrides: el mismo `id` sustituye a la regla base y uno nuevo se añade al final.
pub fn merge(mut base: Vec<Rule>, overrides: Vec<Rule>) -> Vec<Rule> {
    for rule in overrides {
        match base.iter_mut().find(|r| r.id == rule.id) {
            Some(slot) => *slot = rule,
            None => base.push(rule),
        }
    }
    base
}

const ID_RULE: &str = "el id solo admite minúsculas, dígitos, '-' y '_'";

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Comprueba los invariantes de todas las reglas y devuelve todos los problemas juntos.
pub fn validate(rules: &[Rule]) -> Result<(), RulesError> {
    let mut problems = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for r in rules {
        let mut bad = |m: &str| {
            problems.push(Problem {
                rule_id: r.id.clone(),
                message: m.to_owned(),
            })
        };
        if !seen.insert(r.id.as_str()) {
            bad("id repetido");
        }
        if !valid_id(&r.id) {
            bad(ID_RULE);
        }
        if r.description.trim().is_empty() {
            bad("falta la descripción");
        }
        if r.matches.all().next().is_none() {
            bad("no coincide con nada en ningún SO");
        }
        if r.matches.all().any(|n| n.trim().is_empty()) {
            bad("hay un nombre vacío en match");
        }
        if r.action == Action::Kill && (r.min_level != Level::Extremo || r.risk != Risk::Low) {
            bad("kill solo se admite con min_level = extremo y risk = low");
        }
        if (r.action == Action::StopService) != (r.category == Category::Service) {
            bad("stop_service y la categoría service van siempre juntos");
        }
        if r.action != Action::StopService {
            for n in r.matches.all().filter(|n| is_protected(n)) {
                bad(&format!("{n} está en la lista de protegidos"));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(RulesError::Invalid(problems))
    }
}

fn validate_recommendations(recs: &[Recommendation]) -> Result<(), RulesError> {
    let mut problems = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for r in recs {
        let mut bad = |m: &str| {
            problems.push(Problem {
                rule_id: r.id.clone(),
                message: m.to_owned(),
            })
        };
        if !seen.insert(r.id.as_str()) {
            bad("id repetido");
        }
        if !valid_id(&r.id) {
            bad(ID_RULE);
        }
        if r.title.trim().is_empty() {
            bad("falta el título");
        }
        if r.steps.is_empty() || r.steps.iter().any(|s| s.trim().is_empty()) {
            bad("faltan pasos o hay uno vacío");
        }
        if r.source.trim().is_empty() {
            bad("falta la fuente");
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(RulesError::Invalid(problems))
    }
}

/// Lee y valida un archivo de recomendaciones manuales.
pub fn parse_recommendations(file: &str, text: &str) -> Result<Vec<Recommendation>, RulesError> {
    let recs = toml::from_str::<RecommendationFile>(text)
        .map(|f| f.recommendation)
        .map_err(|e| RulesError::Parse {
            file: file.to_owned(),
            message: e.to_string(),
        })?;
    validate_recommendations(&recs)?;
    Ok(recs)
}

/// Recomendaciones manuales embebidas, ya validadas.
pub fn recommendations() -> Result<Vec<Recommendation>, RulesError> {
    parse_recommendations("recommendations.toml", RECOMMENDATIONS)
}

/// Reglas embebidas, ya validadas.
pub fn embedded() -> Result<Vec<Rule>, RulesError> {
    let rules = embedded_unvalidated()?;
    validate(&rules)?;
    Ok(rules)
}

fn embedded_unvalidated() -> Result<Vec<Rule>, RulesError> {
    let mut rules = vec![];
    for (file, text) in EMBEDDED {
        rules.extend(parse(file, text)?);
    }
    Ok(rules)
}

/// Reglas embebidas más los `*.toml` de `user_dir` (en orden alfabético), validadas juntas.
/// Si `user_dir` no existe, no hay overrides.
pub fn load(user_dir: Option<&Path>) -> Result<Vec<Rule>, RulesError> {
    let mut rules = embedded_unvalidated()?;
    if let Some(dir) = user_dir.filter(|d| d.is_dir()) {
        let io = |path: &Path, e: std::io::Error| RulesError::Io {
            path: path.to_owned(),
            message: e.to_string(),
        };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| io(dir, e))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        for path in files {
            let text = std::fs::read_to_string(&path).map_err(|e| io(&path, e))?;
            rules = merge(rules, parse(&path.to_string_lossy(), &text)?);
        }
    }
    validate(&rules)?;
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[rule]]
id = "onedrive"
match = { windows = ["OneDrive"], macos = ["OneDrive"] }
category = "sync"
action = "suspend"
min_level = "agresivo"
risk = "low"
description = "Sincronización en la nube."

[[rule]]
id = "wsearch"
match = { windows = ["WSearch"] }
category = "service"
action = "stop_service"
min_level = "agresivo"
risk = "low"
description = "Indexador de búsqueda."
"#;

    fn sample() -> Vec<Rule> {
        parse("sample.toml", SAMPLE).unwrap()
    }

    fn problems(rules: &[Rule]) -> Vec<String> {
        match validate(rules) {
            Ok(()) => vec![],
            Err(RulesError::Invalid(ps)) => ps.into_iter().map(|p| p.message).collect(),
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn parses_the_documented_schema() {
        let rules = sample();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].min_level, Level::Agresivo);
        assert!(rules[0].matches.linux.is_empty());
        assert!(!rules[0].optional && !rules[0].elevated);
        validate(&rules).unwrap();
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let err = parse("x.toml", "[[rule]]\nid = \"a\"\nbogus = 1\n").unwrap_err();
        assert!(matches!(err, RulesError::Parse { .. }));
    }

    #[test]
    fn overrides_replace_by_id_and_append_new_ones() {
        let mut changed = sample()[0].clone();
        changed.min_level = Level::Normal;
        let mut new = changed.clone();
        new.id = "dropbox".into();
        let merged = merge(sample(), vec![changed, new]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].min_level, Level::Normal);
        assert_eq!(merged[2].id, "dropbox");
    }

    #[test]
    fn kill_needs_extremo_and_low_risk() {
        let mut r = sample()[0].clone();
        r.action = Action::Kill;
        assert_eq!(problems(&[r.clone()]).len(), 1);
        r.min_level = Level::Extremo;
        assert!(problems(&[r]).is_empty());
    }

    #[test]
    fn protected_processes_cannot_be_targeted() {
        let mut r = sample()[0].clone();
        r.matches.windows = vec!["csrss.exe".into(), "EasyAntiCheat".into()];
        assert_eq!(problems(&[r]).len(), 2);
    }

    #[test]
    fn structural_problems_are_all_reported() {
        let mut r = sample()[1].clone();
        r.id = "Bad Id".into();
        r.category = Category::Sync;
        r.description = " ".into();
        r.matches = Default::default();
        let dup = r.clone();
        let ps = problems(&[r, dup]);
        assert!(ps.len() >= 5, "{ps:?}");
    }

    #[test]
    fn embedded_rules_are_valid() {
        let rules = embedded().unwrap();
        assert_eq!(rules.len(), 58);
        // Los navegadores vienen desmarcados (PLAN.md §3).
        assert!(
            rules
                .iter()
                .filter(|r| ["chrome", "firefox", "edge"].contains(&r.id.as_str()))
                .all(|r| r.optional)
        );
    }

    #[test]
    fn embedded_recommendations_are_valid_and_filtered_by_device() {
        use framerush_core::recommendations_for;
        use framerush_device::samples;
        let recs = recommendations().unwrap();
        assert_eq!(recs.len(), 3);
        let mut d = samples::hybrid_laptop();
        d.os = framerush_platform::Os::Windows;
        d.laptop = Some(true);
        let ids = |appid| -> Vec<String> {
            recommendations_for(&recs, &d, appid)
                .into_iter()
                .map(|r| r.id.clone())
                .collect()
        };
        assert_eq!(ids(None), ["play-plugged-in", "nvidia-driver-update"]);
        assert!(ids(Some(730)).contains(&"nvidia-panel-cs2".to_owned()));
    }

    #[test]
    fn recommendations_without_steps_or_source_are_rejected() {
        let text = "[[recommendation]]
id = \"A\"
title = \"\"
steps = []
source = \" \"
";
        match parse_recommendations("x.toml", text) {
            Err(RulesError::Invalid(ps)) => assert_eq!(ps.len(), 4, "{ps:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn user_dir_overrides_embedded_rules() {
        let dir = std::env::temp_dir().join(format!("framerush-rules-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mine.toml"), SAMPLE).unwrap();
        std::fs::write(dir.join("notes.txt"), "no es toml").unwrap();
        let rules = load(Some(&dir)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(rules.iter().any(|r| r.id == "onedrive"));
        assert_eq!(load(Some(Path::new("no-existe"))), embedded());
    }
}
