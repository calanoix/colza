//! User preferences set on the settings page, saved across runs.
//!
//! Stored as plain `key = value` lines in
//! `$XDG_CONFIG_HOME/colza/settings` (next to the screencast restore
//! token, see `token_store.rs`), rather than through serde: two enums
//! don't justify a dependency, and the format stays hand-editable.
//!
//! Same failure policy as `token_store.rs`: nothing here returns an
//! error. A missing or unreadable file, or an unknown value, means the
//! default for that key; a failed write is reported to stderr and the
//! choice still applies for the rest of the run.

use std::path::PathBuf;

/// Light, dark, or following the desktop.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    /// The value written to the settings file.
    fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Prefs {
    pub theme: Theme,
}

impl Prefs {
    pub fn load() -> Self {
        settings_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                eprintln!("colza: could not create {}: {err}", parent.display());
                return;
            }
        }
        if let Err(err) = std::fs::write(&path, self.serialize()) {
            eprintln!("colza: could not save settings to {}: {err}", path.display());
        }
    }

    /// Unknown keys and values are skipped, so a file written by a newer
    /// colza (or edited by hand) still loads what it can.
    fn parse(text: &str) -> Self {
        let mut prefs = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "theme" => {
                    if let Some(theme) = Theme::ALL.into_iter().find(|t| t.key() == value) {
                        prefs.theme = theme;
                    }
                }
                // `capture` (the old Live/Still choice) is no longer read:
                // a pick now takes the first capture method not denied.
                // Like any unknown key, it's skipped, and dropped on the
                // next save.
                _ => {}
            }
        }
        prefs
    }

    fn serialize(&self) -> String {
        format!("theme = {}\n", self.theme.key())
    }
}

/// `$XDG_CONFIG_HOME/colza/settings`, falling back to `$HOME/.config`.
fn settings_path() -> Option<PathBuf> {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(dir.join("colza").join("settings"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_roundtrips() {
        let prefs = Prefs { theme: Theme::Dark };
        assert_eq!(Prefs::parse(&prefs.serialize()), prefs);
    }

    #[test]
    fn parse_is_tolerant() {
        let prefs = Prefs::parse("theme=light\n# comment\ncapture = sideways\nfuture = 1\n");
        assert_eq!(
            prefs,
            Prefs { theme: Theme::Light }
        );
        assert_eq!(Prefs::parse(""), Prefs::default());
    }
}