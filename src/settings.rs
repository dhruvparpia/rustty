//! Settings provider slice (#91): searchable setting names with effective
//! values and source. The palette opens `config.toml` through the native
//! Preferences editor; atomic in-place edits are the next slice.

use std::path::Path;

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
        "session-rail",
        format!("{}", cfg.session_rail),
        "global",
        false,
    );
    add(
        &mut out,
        "automation-writes",
        format!("{}", cfg.automation_writes),
        "global",
        true,
    );
    add(
        &mut out,
        "welcome",
        format!("{}", cfg.welcome),
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
            &format!("profile.{}.confirm-close", profile.name.clone()),
            format!("{}", profile.confirm_close.unwrap_or(false)),
            &source,
            false,
        );
    }
    out
}

/// Atomic text exchange for the future settings editor (issue #91).
/// The editor supplies the text; comments are preserved if the text keeps them.
///
/// # Errors
///
/// Any I/O failure; the original file remains on disk.
#[allow(dead_code, reason = "settings editor writes exchanged text")]
pub fn write_text_atomic(path: &Path, text: String) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_file_name("config.toml.tmp");
    let backup = path.with_file_name("config.toml.old");
    std::fs::remove_file(&backup).ok();
    std::fs::write(&tmp, text)?;
    if path.exists() {
        std::fs::rename(path, &backup)?;
    }
    std::fs::rename(&tmp, path)?;
    std::fs::remove_file(&backup).ok();
    Ok(())
}

/// Atomic plain-key editor for the settings overlay (issue #91).
/// Replaces one top-level scalar assignment, keeps the trailing comment, and
/// writes through `write_text_atomic`. Table/profile keys are later slices.
///
/// # Errors
///
/// Any I/O failure; the original file remains on disk.
#[allow(dead_code, reason = "settings overlay edits plain keys")]
pub fn replace_plain(path: &Path, key: String, value: String) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let lines = text.lines().map(|s| s.to_string()).collect::<Vec<String>>();
    let key = key.to_string();
    let mut found = false;
    let first_section = lines.iter().position(|line| line.trim().starts_with('['));
    let mut out = Vec::new();
    for line in lines.iter() {
        let Some(eq) = line.find('=') else {
            out.push(line.clone());
            continue;
        };
        let name = line[..eq].to_string().trim().to_string();
        if !found && name == key {
            found = true;
            let rest = line[eq + 1..].to_string();
            let comment = match rest.find('#') {
                Some(c) => rest[c..].to_string(),
                None => String::new(),
            };
            let head = line[..eq + 1].to_string().trim_end().to_string();
            out.push(
                format!("{} {} {}", head, value, comment)
                    .trim_end()
                    .to_string(),
            );
        } else {
            out.push(line.clone());
        }
    }
    if !found {
        let insert = first_section.unwrap_or(out.len());
        out.insert(insert, format!("{} = {}", key, value).to_string());
    }
    let new_text = format!("{}\n", out.join("\n")).to_string();
    write_text_atomic(path, new_text)
}

/// Atomic `[section] key` edit for table settings (#91).
#[allow(
    dead_code,
    clippy::cmp_owned,
    clippy::needless_range_loop,
    reason = "line editor compares borrowed strings and indexes lines"
)]
pub fn replace_section(
    path: &Path,
    header: String,
    key: String,
    value: String,
) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let lines = text.lines().map(|s| s.to_string()).collect::<Vec<String>>();
    let Some(start) = lines
        .iter()
        .position(|line| line.trim().to_string() == header)
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("section {} not found", header).to_string(),
        ));
    };
    let mut end = start + 1;
    while end < lines.len() && !lines[end].trim().starts_with('[') {
        end += 1;
    }
    let mut found = false;
    let mut out = Vec::new();
    for i in 0..lines.len() {
        let line = lines[i].clone();
        if i > start && i < end {
            let Some(eq) = line.find('=') else {
                out.push(line.clone());
                continue;
            };
            let name = line[..eq].to_string().trim().to_string();
            if !found && name == key {
                found = true;
                let rest = line[eq + 1..].to_string();
                let comment = match rest.find('#') {
                    Some(c) => rest[c..].to_string(),
                    None => String::new(),
                };
                let head = line[..eq + 1].to_string().trim_end().to_string();
                out.push(
                    format!("{} {} {}", head, value, comment)
                        .trim_end()
                        .to_string(),
                );
            } else {
                out.push(line.clone());
            }
        } else {
            out.push(line.clone());
        }
    }
    if !found {
        out.insert(start + 1, format!("{} = {}", key, value).to_string());
    }
    let new_text = format!("{}\n", out.join("\n")).to_string();
    write_text_atomic(path, new_text)
}

/// Atomic `[[profile]]` key edit for profile settings (#91).
#[allow(
    dead_code,
    clippy::cmp_owned,
    clippy::needless_range_loop,
    reason = "line editor compares borrowed strings and indexes lines"
)]
pub fn replace_profile(
    path: &Path,
    profile: String,
    key: String,
    value: String,
) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let lines = text.lines().map(|s| s.to_string()).collect::<Vec<String>>();
    let profile_line = format!("name = {:?}", profile).to_string();
    let mut block_start = None;
    let mut block_end = None;
    let mut i = 0usize;
    while i < lines.len() {
        if lines[i].trim().to_string() == "[[profile]]" {
            let mut end = i + 1;
            while end < lines.len() && !lines[end].trim().starts_with('[') {
                end += 1;
            }
            let mut has_name = false;
            for j in i + 1..end {
                if lines[j].trim().to_string() == profile_line {
                    has_name = true;
                    break;
                }
            }
            if has_name {
                block_start = Some(i);
                block_end = Some(end);
                break;
            }
        }
        i += 1;
    }
    if block_start.is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("profile {:?} not found", profile).to_string(),
        ));
    }
    let start = block_start.unwrap_or(0);
    let end = block_end.unwrap_or(lines.len());
    let mut found = false;
    let mut out = Vec::new();
    for i in 0..lines.len() {
        let line = lines[i].clone();
        if i > start && i < end {
            let Some(eq) = line.find('=') else {
                out.push(line.clone());
                continue;
            };
            let name = line[..eq].to_string().trim().to_string();
            if !found && name == key {
                found = true;
                let rest = line[eq + 1..].to_string();
                let comment = match rest.find('#') {
                    Some(c) => rest[c..].to_string(),
                    None => String::new(),
                };
                let head = line[..eq + 1].to_string().trim_end().to_string();
                out.push(
                    format!("{} {} {}", head, value, comment)
                        .trim_end()
                        .to_string(),
                );
            } else {
                out.push(line.clone());
            }
        } else {
            out.push(line.clone());
        }
    }
    if !found {
        out.insert(start + 1, format!("{} = {}", key, value).to_string());
    }
    let new_text = format!("{}\n", out.join("\n")).to_string();
    write_text_atomic(path, new_text)
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

    #[test]
    fn atomic_text_exchange_round_trips() {
        let dir = std::env::temp_dir().join(format!("rustty-config-atomic-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        let text = "# comment\nfont = \"Cascadia Mono\"\n".to_string();
        write_text_atomic(&path, text.clone()).expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_failure_keeps_the_original() {
        let dir = std::env::temp_dir().join(format!("rustty-config-fail-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        let original = "theme = \"one-dark\"\n".to_string();
        std::fs::write(&path, &original).expect("seed");
        let tmp = path.with_file_name("config.toml.tmp");
        std::fs::remove_file(&tmp).ok();
        std::fs::remove_dir(&tmp).ok();
        std::fs::create_dir_all(&tmp).expect("tmp directory");
        assert!(write_text_atomic(&path, "font = \"Cascadia Mono\"\n".to_string()).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), original);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn replace_plain_preserves_the_comment() {
        let dir = std::env::temp_dir().join(format!("rustty-settings-edit-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "# keep\nfont = \"Old\" # comment\n").expect("seed");
        replace_plain(&path, "font".to_string(), "\"New\"".to_string()).expect("edit");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "# keep\nfont = \"New\" # comment\n",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_plain_inserts_a_missing_plain_key_before_sections() {
        let dir =
            std::env::temp_dir().join(format!("rustty-settings-insert-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "# head\n[window]\ncols = 80\n").expect("seed");
        replace_plain(&path, "font".to_string(), "\"Cascadia Mono\"".to_string()).expect("insert");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "# head\nfont = \"Cascadia Mono\"\n[window]\ncols = 80\n",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn replace_section_preserves_the_comment() {
        let dir =
            std::env::temp_dir().join(format!("rustty-settings-section-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[window]\ncols = 80 # keep\nrows = 24\n").expect("seed");
        replace_section(
            &path,
            "[window]".to_string(),
            "cols".to_string(),
            "120".to_string(),
        )
        .expect("edit");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "[window]\ncols = 120 # keep\nrows = 24\n",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_profile_preserves_the_comment() {
        let dir =
            std::env::temp_dir().join(format!("rustty-settings-profile-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[[profile]]\nname = \"Git Bash\"\nconfirm-close = false # keep\n\n[[profile]]\nname = \"PowerShell\"\n").expect("seed");
        replace_profile(
            &path,
            "Git Bash".to_string(),
            "confirm-close".to_string(),
            "true".to_string(),
        )
        .expect("edit");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "[[profile]]\nname = \"Git Bash\"\nconfirm-close = true # keep\n\n[[profile]]\nname = \"PowerShell\"\n",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
