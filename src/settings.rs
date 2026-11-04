//! Settings provider slice (#91): searchable setting names with effective
//! values and source. The palette opens `config.toml` through the native
//! Preferences editor; applying/canceling individual values is a later slice.

use crate::config::Config;

/// One searchable setting.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    /// User-visible setting name.
    pub name: String,
    /// Effective value as displayed.
    pub value: String,
    /// "global", "profile <name>", "keybinding".
    pub source: String,
    /// Changing it needs a new window/session.
    pub restart: bool,
    /// Conflict or validation note.
    pub conflict: Option<String>,
}

impl Setting {
    /// Human label.
    #[must_use]
    pub fn label(&self) -> String {
        let base = format!("{} = {} ({})", self.name, self.value, self.source);
        if self.restart {
            format!("{} [restart required]", base)
        } else if self.conflict.is_some() {
            format!("{} [{}]", base, self.conflict.clone().unwrap_or_default())
        } else {
            base
        }
    }
}

fn add(out: &mut Vec<Setting>, name: &str, value: String, source: &str, restart: bool) {
    out.push(Setting {
        name: name.to_string(),
        value: value.clone(),
        source: source.to_string(),
        restart,
        conflict: None,
    });
}

/// Global settings from a Config snapshot.
#[must_use]
pub fn settings(cfg: &Config) -> Vec<Setting> {
    let mut out = Vec::new();
    add(
        &mut out,
        "theme",
        cfg.theme.name().to_string(),
        "global",
        false,
    );
    add(&mut out, "font", cfg.font.to_string(), "global", true);
    add(
        &mut out,
        "font-size",
        format!("{}", cfg.font_size),
        "global",
        true,
    );
    add(
        &mut out,
        "line-height",
        format!("{}", cfg.line_height),
        "global",
        true,
    );
    add(
        &mut out,
        "font-features",
        cfg.font_features.to_string(),
        "global",
        true,
    );
    add(
        &mut out,
        "ligatures",
        format!("{}", cfg.ligatures),
        "global",
        true,
    );
    add(
        &mut out,
        "scrollback",
        format!("{}", cfg.scrollback),
        "global",
        false,
    );
    let tab_bar_visible = match cfg.tab_bar.visible {
        crate::config::TabBar::Always => "always".to_string(),
        crate::config::TabBar::Auto => "auto".to_string(),
        crate::config::TabBar::Hidden => "hidden".to_string(),
    };
    add(
        &mut out,
        "tab-bar.visible",
        tab_bar_visible,
        "global",
        false,
    );
    add(
        &mut out,
        "tab-bar.show-profiles",
        format!("{}", cfg.tab_bar.show_profiles),
        "global",
        false,
    );
    add(
        &mut out,
        "tab-bar.show-cwd",
        format!("{}", cfg.tab_bar.show_cwd),
        "global",
        false,
    );
    add(
        &mut out,
        "tab-bar.close-button",
        format!("{}", cfg.tab_bar.close_button),
        "global",
        false,
    );
    add(
        &mut out,
        "opacity",
        format!("{}", cfg.opacity),
        "global",
        false,
    );
    add(
        &mut out,
        "copy-on-select",
        format!("{}", cfg.copy_on_select),
        "global",
        false,
    );
    add(
        &mut out,
        "session-restore",
        format!("{}", cfg.session_restore),
        "global",
        false,
    );
    add(
        &mut out,
        "shell-integration",
        format!("{}", cfg.shell_integration),
        "global",
        false,
    );
    add(
        &mut out,
        "command-notify-ms",
        format!("{}", cfg.command_notify_ms),
        "global",
        false,
    );
    add(
        &mut out,
        "detach-prefix",
        cfg.detach_prefix.to_string(),
        "global",
        false,
    );
    add(&mut out, "editor", cfg.editor.to_string(), "global", false);
    add(
        &mut out,
        "starting-profile",
        cfg.starting_profile.to_string(),
        "global",
        false,
    );
    add(
        &mut out,
        "window.cols",
        format!("{}", cfg.window.cols),
        "global",
        true,
    );
    add(
        &mut out,
        "window.rows",
        format!("{}", cfg.window.rows),
        "global",
        true,
    );
    add(
        &mut out,
        "window.padding.x",
        format!("{}", cfg.window.padding.x),
        "global",
        true,
    );
    add(
        &mut out,
        "window.padding.y",
        format!("{}", cfg.window.padding.y),
        "global",
        true,
    );
    add(
        &mut out,
        "split.visible",
        format!("{}", cfg.split.visible),
        "global",
        false,
    );
    add(
        &mut out,
        "split.padding",
        format!("{}", cfg.split.padding),
        "global",
        false,
    );
    add(
        &mut out,
        "search.visible",
        format!("{}", cfg.search.visible),
        "global",
        false,
    );
    add(
        &mut out,
        "search.case-sensitive-default",
        format!("{}", cfg.search.case_sensitive_default),
        "global",
        false,
    );
    add(
        &mut out,
        "search.whole-word-default",
        format!("{}", cfg.search.whole_word_default),
        "global",
        false,
    );
    add(
        &mut out,
        "search.regex-default",
        format!("{}", cfg.search.regex_default),
        "global",
        false,
    );
    add(
        &mut out,
        "search.context-lines",
        format!("{}", cfg.search.context_lines),
        "global",
        false,
    );
    for profile in cfg.profiles.iter() {
        let source = format!("profile {}", profile.name).to_string();
        add(
            &mut out,
            &format!("profile.{}.theme", profile.name.clone()),
            profile.theme.clone(),
            &source,
            false,
        );
        add(
            &mut out,
            &format!("profile.{}.starting-directory", profile.name.clone()),
            profile.starting_directory.clone(),
            &source,
            false,
        );
        add(
            &mut out,
            &format!("profile.{}.tab-color", profile.name.clone()),
            profile.tab_color.clone().unwrap_or_default().to_string(),
            &source,
            false,
        );
        add(
            &mut out,
            &format!("profile.{}.scrollback", profile.name.clone()),
            format!("{}", profile.scrollback.unwrap_or(0)),
            &source,
            false,
        );
        add(
            &mut out,
            &format!("profile.{}.font", profile.name.clone()),
            profile.font.clone().unwrap_or_default().to_string(),
            &source,
            true,
        );
        add(
            &mut out,
            &format!("profile.{}.font-size", profile.name.clone()),
            format!("{}", profile.font_size.unwrap_or(0.0)),
            &source,
            true,
        );
        add(
            &mut out,
            &format!("profile.{}.line-height", profile.name.clone()),
            format!("{}", profile.line_height.unwrap_or(0.0)),
            &source,
            true,
        );
        add(
            &mut out,
            &format!("profile.{}.confirm-close", profile.name.clone()),
            format!("{}", profile.confirm_close.unwrap_or(false)),
            &source,
            false,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProfileConfig;

    #[test]
    fn settings_list_has_global_values() {
        let cfg = Config::default();
        let out = settings(&cfg);
        assert!(out.iter().any(|s| s.name == "theme"));
        assert!(out.iter().any(|s| s.name == "font"));
        assert!(out.iter().any(|s| s.name == "search.context-lines"));
    }

    #[test]
    fn label_carries_source_and_restart() {
        let cfg = Config::default();
        let out = settings(&cfg);
        let font = out.iter().find(|s| s.name == "font").unwrap();
        assert!(font.label().contains("global"));
        assert!(font.label().contains("restart required"));
    }

    #[test]
    fn profile_settings_use_profile_source() {
        let cfg = Config {
            profiles: vec![ProfileConfig {
                name: "Git Bash".into(),
                command: "bash".into(),
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                starting_directory: "~".to_string(),
                theme: "".to_string(),
                tab_color: None,
                font: None,
                font_size: None,
                line_height: None,
                confirm_close: None,
                scrollback: None,
            }],
            ..Default::default()
        };
        let out = settings(&cfg);
        assert!(out.iter().any(|s| s.source.starts_with("profile ")));
    }
}
