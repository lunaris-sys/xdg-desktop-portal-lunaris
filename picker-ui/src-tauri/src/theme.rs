//! Minimal Lunaris theme loader for the picker UI.
//!
//! Reads `~/.config/lunaris/appearance.toml`, picks dark or light
//! defaults, and applies the `[overrides]` table on top. Token
//! references like `accent = "$foreground"` resolve against the
//! base theme.
//!
//! This is a tiny subset of what `desktop-shell` does. Picker UI
//! only needs ~7 colors; pulling in the full theme loader would
//! mean adding desktop-shell as a dep, which it isn't.

use serde::{Deserialize, Serialize};

/// Color tokens the picker UI cares about. Maps 1:1 to CSS custom
/// properties on `:root` after fetch.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub bg_app: String,
    pub bg_card: String,
    pub fg_app: String,
    pub fg_muted: String,
    pub border: String,
    pub accent: String,
    pub danger: String,
}

impl Theme {
    fn dark_defaults() -> Self {
        Self {
            bg_app: "#0f0f0f".into(),
            bg_card: "#171717".into(),
            fg_app: "#fafafa".into(),
            fg_muted: "#a1a1aa".into(),
            border: "#27272a".into(),
            accent: "#6366f1".into(),
            danger: "#ef4444".into(),
        }
    }

    fn light_defaults() -> Self {
        Self {
            bg_app: "#ffffff".into(),
            bg_card: "#fafafa".into(),
            fg_app: "#0f0f0f".into(),
            fg_muted: "#71717a".into(),
            border: "#e4e4e7".into(),
            accent: "#6366f1".into(),
            danger: "#ef4444".into(),
        }
    }
}

#[derive(Deserialize, Default)]
struct AppearanceFile {
    theme: Option<ThemeSection>,
    overrides: Option<toml::Table>,
}

#[derive(Deserialize, Default)]
struct ThemeSection {
    mode: Option<String>,
}

/// Load the user's theme. Falls back to dark defaults on any
/// error so the picker always has a working theme.
pub fn load_theme() -> Theme {
    let Some(config_root) = dirs::config_dir() else {
        return Theme::dark_defaults();
    };
    let path = config_root.join("lunaris").join("appearance.toml");
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Theme::dark_defaults(),
    };
    let parsed: AppearanceFile = toml::from_str(&contents).unwrap_or_default();

    let mode = parsed
        .theme
        .as_ref()
        .and_then(|t| t.mode.as_deref())
        .unwrap_or("dark");
    let mut theme = if mode == "light" {
        Theme::light_defaults()
    } else {
        Theme::dark_defaults()
    };

    if let Some(overrides) = parsed.overrides {
        apply_overrides(&mut theme, &overrides);
    }
    theme
}

/// Apply user overrides to a base theme. Two-pass so a token-ref
/// like `accent = "$foreground"` resolves against the *base*
/// theme, not against another override that hasn't been applied
/// yet (would create order-dependent results).
fn apply_overrides(theme: &mut Theme, overrides: &toml::Table) {
    let base = theme.clone();
    for (key, val) in overrides.iter() {
        let toml::Value::String(raw) = val else {
            continue;
        };
        let resolved = resolve_token(raw, &base);
        apply_override(theme, key, &resolved);
    }
}

/// Resolve a `$token` reference against the base theme. Plain
/// hex colors pass through unchanged.
fn resolve_token(val: &str, base: &Theme) -> String {
    let Some(token) = val.strip_prefix('$') else {
        return val.to_string();
    };
    match token {
        "foreground" | "fg" | "fg_app" | "primary" => base.fg_app.clone(),
        "muted" | "fg_muted" | "secondary" => base.fg_muted.clone(),
        "background" | "bg" | "bg_app" => base.bg_app.clone(),
        "card" | "bg_card" => base.bg_card.clone(),
        "border" => base.border.clone(),
        "accent" => base.accent.clone(),
        _ => val.to_string(),
    }
}

fn apply_override(theme: &mut Theme, key: &str, val: &str) {
    match key {
        "accent" => theme.accent = val.to_string(),
        "background" | "bg_app" => theme.bg_app = val.to_string(),
        "card" | "bg_card" => theme.bg_card = val.to_string(),
        "foreground" | "fg_app" => theme.fg_app = val.to_string(),
        "muted" | "fg_muted" => theme.fg_muted = val.to_string(),
        "border" => theme.border = val.to_string(),
        "danger" | "error" => theme.danger = val.to_string(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_resolves_to_base_color() {
        let base = Theme::dark_defaults();
        assert_eq!(resolve_token("$foreground", &base), base.fg_app);
        assert_eq!(resolve_token("$accent", &base), base.accent);
    }

    #[test]
    fn plain_color_passes_through() {
        let base = Theme::dark_defaults();
        assert_eq!(resolve_token("#ff0000", &base), "#ff0000");
    }

    #[test]
    fn unknown_token_returns_literal() {
        let base = Theme::dark_defaults();
        assert_eq!(resolve_token("$nonexistent", &base), "$nonexistent");
    }

    /// Mirrors Tim's reported appearance.toml: `accent = "$foreground"`
    /// resolves to the fg_app color of the active theme.
    #[test]
    fn override_with_token_reference() {
        let mut theme = Theme::dark_defaults();
        let mut overrides = toml::Table::new();
        overrides.insert(
            "accent".to_string(),
            toml::Value::String("$foreground".to_string()),
        );
        apply_overrides(&mut theme, &overrides);
        assert_eq!(theme.accent, theme.fg_app);
    }
}
