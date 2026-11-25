//! Errores para la UI, en su idioma.

use framerush_core::Lang;
use std::error::Error;

/// `e` con su cadena de causas, como `{:#}`, en `lang`. Los errores de las librerías se
/// traducen; el texto del SO y de las dependencias va tal cual.
pub fn message(e: &anyhow::Error, lang: Lang) -> String {
    let mut parts: Vec<String> = vec![];
    for cause in e.chain() {
        let text = localized(cause, lang);
        // Algunos errores ya llevan su causa en el texto: no se repite.
        if parts.last().is_some_and(|p| p.ends_with(&text)) {
            continue;
        }
        parts.push(text);
    }
    parts.join(": ")
}

fn localized(e: &(dyn Error + 'static), lang: Lang) -> String {
    if let Some(e) = e.downcast_ref::<framerush_platform::PlatformError>() {
        e.message(lang)
    } else if let Some(e) = e.downcast_ref::<framerush_core::EngineError>() {
        e.message(lang)
    } else if let Some(e) = e.downcast_ref::<framerush_core::JournalError>() {
        e.message(lang)
    } else if let Some(e) = e.downcast_ref::<framerush_steam::SteamError>() {
        e.message(lang)
    } else if let Some(e) = e.downcast_ref::<framerush_games::GameError>() {
        e.message(lang)
    } else if let Some(e) = e.downcast_ref::<framerush_rules::RulesError>() {
        e.message(lang)
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use framerush_platform::PlatformError;

    #[test]
    fn translates_the_library_errors_of_the_chain() {
        let e = Err::<(), _>(PlatformError::AccessDenied("WMI".into()))
            .context("could not apply")
            .unwrap_err();
        assert_eq!(message(&e, Lang::En), "could not apply: access denied: WMI");
        assert_eq!(
            message(&e, Lang::Es),
            "could not apply: acceso denegado: WMI"
        );
    }

    #[test]
    fn a_cause_already_in_the_text_is_not_repeated() {
        let io = std::io::Error::other("disco lleno");
        let e = anyhow::Error::new(framerush_core::JournalError::Io {
            path: "session.json".into(),
            source: io,
        });
        assert_eq!(
            message(&e, Lang::En),
            "could not read or write the journal session.json: disco lleno"
        );
    }
}
