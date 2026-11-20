//! Fluent-based localisation. Italian and English are embedded; a file at
//! `<exe dir>/locales/<lang>/main.ftl` overrides or adds a language.

use std::path::Path;

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

const FALLBACK: &str = "en";
const EMBEDDED: &[(&str, &str)] = &[
    ("en", include_str!("../../../locales/en/main.ftl")),
    ("it", include_str!("../../../locales/it/main.ftl")),
];

pub struct Localizer {
    lang: String,
    primary: FluentBundle<FluentResource>,
    fallback: FluentBundle<FluentResource>,
}

impl std::fmt::Debug for Localizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Localizer")
            .field("lang", &self.lang)
            .finish()
    }
}

impl Localizer {
    /// `preference` is a language tag or `"auto"` (Windows UI language).
    pub fn new(preference: &str, exe_dir: Option<&Path>) -> Self {
        let wanted = if preference.eq_ignore_ascii_case("auto") {
            system_language()
        } else {
            preference.to_owned()
        };
        let lang = primary_subtag(&wanted);
        let primary = load_bundle(&lang, exe_dir)
            .or_else(|| load_bundle(FALLBACK, exe_dir))
            .unwrap_or_else(|| empty_bundle(FALLBACK));
        let fallback = load_bundle(FALLBACK, None).unwrap_or_else(|| empty_bundle(FALLBACK));
        let lang = if has_language(&lang, exe_dir) {
            lang
        } else {
            FALLBACK.to_owned()
        };
        Self {
            lang,
            primary,
            fallback,
        }
    }

    pub fn language(&self) -> &str {
        &self.lang
    }

    pub fn tr(&self, id: &str) -> String {
        self.format(id, None)
    }

    pub fn tr_args(&self, id: &str, args: &[(&str, &str)]) -> String {
        let mut fa = FluentArgs::new();
        for (k, v) in args {
            fa.set(*k, FluentValue::from(*v));
        }
        self.format(id, Some(&fa))
    }

    fn format(&self, id: &str, args: Option<&FluentArgs>) -> String {
        for bundle in [&self.primary, &self.fallback] {
            if let Some(pattern) = bundle.get_message(id).and_then(|m| m.value()) {
                let mut errors = Vec::new();
                return bundle
                    .format_pattern(pattern, args, &mut errors)
                    .into_owned();
            }
        }
        // Visible but harmless: makes missing keys easy to spot.
        id.to_owned()
    }
}

/// Languages available (embedded plus external overrides), sorted.
pub fn available_languages(exe_dir: Option<&Path>) -> Vec<String> {
    let mut langs: Vec<String> = EMBEDDED.iter().map(|(l, _)| (*l).to_owned()).collect();
    if let Some(dir) = exe_dir
        && let Ok(entries) = std::fs::read_dir(dir.join("locales"))
    {
        for e in entries.flatten() {
            if e.path().join("main.ftl").is_file()
                && let Some(name) = e.file_name().to_str()
            {
                langs.push(name.to_owned());
            }
        }
    }
    langs.sort();
    langs.dedup();
    langs
}

fn has_language(lang: &str, exe_dir: Option<&Path>) -> bool {
    available_languages(exe_dir).iter().any(|l| l == lang)
}

fn primary_subtag(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or(FALLBACK)
        .to_ascii_lowercase()
}

fn load_bundle(lang: &str, exe_dir: Option<&Path>) -> Option<FluentBundle<FluentResource>> {
    let external = exe_dir
        .map(|d| d.join("locales").join(lang).join("main.ftl"))
        .and_then(|p| std::fs::read_to_string(p).ok());
    let source = external.or_else(|| {
        EMBEDDED
            .iter()
            .find(|(l, _)| *l == lang)
            .map(|(_, s)| (*s).to_owned())
    })?;
    let resource = match FluentResource::try_new(source) {
        Ok(r) => r,
        // Keep the valid entries of a partially broken community file.
        Err((r, errs)) => {
            tracing::warn!(lang, errors = errs.len(), "locale file has syntax errors");
            r
        }
    };
    let mut bundle = empty_bundle(lang);
    if let Err(errs) = bundle.add_resource(resource) {
        tracing::warn!(lang, errors = errs.len(), "duplicate locale keys");
    }
    Some(bundle)
}

fn empty_bundle(lang: &str) -> FluentBundle<FluentResource> {
    let id: LanguageIdentifier = lang.parse().unwrap_or_default();
    let mut b = FluentBundle::new(vec![id]);
    // Unicode isolation marks render as boxes in some fonts.
    b.set_use_isolating(false);
    b
}

fn system_language() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    // SAFETY: buffer is LOCALE_NAME_MAX_LENGTH wide.
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if n > 1 {
        String::from_utf16_lossy(&buf[..(n as usize - 1)])
    } else {
        FALLBACK.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(src: &str) -> Vec<String> {
        src.lines()
            .filter_map(|l| l.split_once(" = ").map(|(k, _)| k.trim().to_owned()))
            .filter(|k| !k.starts_with('#') && !k.is_empty())
            .collect()
    }

    #[test]
    fn italian_and_english_have_same_keys() {
        let en = keys(EMBEDDED[0].1);
        let it = keys(EMBEDDED[1].1);
        assert_eq!(en, it);
    }

    #[test]
    fn translates_with_args() {
        let l = Localizer::new("it-IT", None);
        assert_eq!(l.language(), "it");
        assert_eq!(l.tr("tray-quit"), "Esci");
        assert_eq!(
            l.tr_args("tray-tooltip", &[("replay", "attivo")]),
            "Hyper Clips — replay attivo"
        );
    }

    #[test]
    fn unknown_language_falls_back_to_english() {
        let l = Localizer::new("xx", None);
        assert_eq!(l.language(), "en");
        assert_eq!(l.tr("tray-quit"), "Quit");
    }

    #[test]
    fn missing_key_returns_id() {
        let l = Localizer::new("en", None);
        assert_eq!(l.tr("no-such-key"), "no-such-key");
    }
}
