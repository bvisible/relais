//! Translated messages. English and French tables are embedded in the binary.

use std::collections::HashMap;
use std::process::Command;
use std::sync::OnceLock;

const EN: &str = include_str!("../locales/en.json");
const FR: &str = include_str!("../locales/fr.json");

fn table() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let source = if language() == "fr" { FR } else { EN };
        serde_json::from_str(source).unwrap_or_default()
    })
}

fn english() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| serde_json::from_str(EN).unwrap_or_default())
}

/// `fr` or `en`: `RELAIS_LANG`, then the POSIX locale variables, then the
/// macOS preferred languages, since terminals often run with `C.UTF-8`.
pub fn language() -> &'static str {
    static LANGUAGE: OnceLock<&'static str> = OnceLock::new();
    LANGUAGE.get_or_init(|| {
        for var in ["RELAIS_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(var) {
                let value = value.to_lowercase();
                if value.is_empty() || value.starts_with("c.") || value == "c" || value == "posix" {
                    continue;
                }
                return if value.starts_with("fr") { "fr" } else { "en" };
            }
        }
        macos_preferred_language().unwrap_or("en")
    })
}

fn macos_preferred_language() -> Option<&'static str> {
    let output = Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text
        .split(['(', ',', ')', '\n'])
        .map(|s| s.trim().trim_matches('"'))
        .find(|s| !s.is_empty())?;
    Some(if first.starts_with("fr") { "fr" } else { "en" })
}

/// The message for `key`, with `{name}` placeholders replaced.
pub fn t(key: &str, args: &[(&str, &str)]) -> String {
    let template = table()
        .get(key)
        .or_else(|| english().get(key))
        .map(String::as_str)
        .unwrap_or(key);
    args.iter()
        .fold(template.to_string(), |text, (name, value)| {
            text.replace(&format!("{{{name}}}"), value)
        })
}
