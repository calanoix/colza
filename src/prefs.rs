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

    fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// Which portal supplies the image under the loupe.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Capture {
    /// `ScreenCast` + PipeWire: live, cursor hidden.
    #[default]
    Live,
    /// `Screenshot`: one still frame per pick, cursor baked in.
    Still,
}

impl Capture {
    fn key(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Still => "still",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Prefs {
    pub theme: Theme,
    pub capture: Capture,
}

impl Prefs {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = path() else {
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
                    if let Some(t) = Theme::ALL.into_iter().find(|t| t.key() == value) {
                        prefs.theme = t;
                    }
                }
                "capture" => {
                    if let Some(c) = [Capture::Live, Capture::Still]
                        .into_iter()
                        .find(|c| c.key() == value)
                    {
                        prefs.capture = c;
                    }
                }
                _ => {}
            }
        }
        prefs
    }

    fn serialize(&self) -> String {
        format!(
            "theme = {}\ncapture = {}\n",
            self.theme.key(),
            self.capture.key()
        )
    }
}

/// `$XDG_CONFIG_HOME/colza/settings`, falling back to `$HOME/.config`.
fn path() -> Option<PathBuf> {
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
    fn roundtrip() {
        let prefs = Prefs { theme: Theme::Dark, capture: Capture::Still };
        assert_eq!(Prefs::parse(&prefs.serialize()), prefs);
    }

    #[test]
    fn tolerant_parse() {
        let p = Prefs::parse("theme=light\n# comment\ncapture = sideways\nfuture = 1\n");
        assert_eq!(p, Prefs { theme: Theme::Light, capture: Capture::Live });
        assert_eq!(Prefs::parse(""), Prefs::default());
    }
}