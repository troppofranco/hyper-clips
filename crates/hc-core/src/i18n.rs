//! Translations: embedded en/it tables, overridable by external files in `lang/`.
//!
//! Lookup order: selected language -> English -> the key itself.

use std::collections::HashMap;
use std::path::Path;

const EN: &str = include_str!("../lang/en.toml");
const IT: &str = include_str!("../lang/it.toml");

pub struct Translator {
    selected: HashMap<String, String>,
    fallback: HashMap<String, String>,
}

fn parse_table(text: &str) -> HashMap<String, String> {
    let Ok(table) = text.parse::<toml::Table>() else {
        return HashMap::new();
    };
    table
        .into_iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_owned())))
        .collect()
}

/// Maps a locale such as `it-IT` or `en_US` to a supported code, defaulting to `en`.
pub fn normalize_language(tag: &str) -> &'static str {
    let lower = tag.to_ascii_lowercase();
    if lower.starts_with("it") {
        "it"
    } else {
        "en"
    }
}

impl Translator {
    /// `language` is a config value: `"auto"` or a code. `system_locale` is only used for `"auto"`.
    /// A file `<lang_dir>/<code>.toml` replaces the embedded table of the same code.
    pub fn new(language: &str, system_locale: &str, lang_dir: &Path) -> Self {
        let code = if language == "auto" {
            normalize_language(system_locale).to_owned()
        } else {
            language.to_owned()
        };

        let fallback = parse_table(EN);
        let embedded = match code.as_str() {
            "it" => Some(IT),
            "en" => Some(EN),
            _ => None,
        };
        // The code was validated by Config (letters, digits, '-', '_'), so it
        // cannot escape `lang_dir`.
        let external = std::fs::read_to_string(lang_dir.join(format!("{code}.toml"))).ok();

        let selected = match (external.as_deref(), embedded) {
            (Some(text), _) => parse_table(text),
            (None, Some(text)) => parse_table(text),
            (None, None) => HashMap::new(),
        };
        Self { selected, fallback }
    }

    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.selected
            .get(key)
            .or_else(|| self.fallback.get(key))
            .map_or(key, String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn italian_and_english_have_the_same_keys() {
        let en = parse_table(EN);
        let it = parse_table(IT);
        assert!(!en.is_empty());
        let mut en_keys: Vec<_> = en.keys().collect();
        let mut it_keys: Vec<_> = it.keys().collect();
        en_keys.sort();
        it_keys.sort();
        assert_eq!(en_keys, it_keys);
    }

    #[test]
    fn selection_and_fallback() {
        let none = Path::new("/nonexistent-lang-dir");
        let it = Translator::new("auto", "it-IT", none);
        assert_eq!(it.t("tray_quit"), "Esci");
        let en = Translator::new("auto", "de-DE", none);
        assert_eq!(en.t("tray_quit"), "Quit");
        // Unknown language falls back to English; unknown key returns the key.
        let xx = Translator::new("xx", "en", none);
        assert_eq!(xx.t("tray_quit"), "Quit");
        assert_eq!(xx.t("no_such_key"), "no_such_key");
    }

    #[test]
    fn external_file_overrides_embedded() {
        let dir = std::env::temp_dir().join(format!("hc-i18n-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(dir.join("it.toml"), "tray_quit = \"Chiudi\"\n").ok();
        let tr = Translator::new("it", "en", &dir);
        assert_eq!(tr.t("tray_quit"), "Chiudi");
        // Keys missing from the external file fall back to English.
        assert_eq!(tr.t("tray_open"), "Open Hyper Clips");
        std::fs::remove_dir_all(&dir).ok();
    }
}
