//! Portable mode (SPEC CONFIG-020, PKG-004, FR-103): when the app runs
//! from an extracted folder, the config lives beside the executable instead
//! of in `%APPDATA%`. The portable ZIP built by `packaging/build-portable.ps1`
//! ships a `rustty.portable` marker file that selects this mode; a
//! folder holding a local `config.toml` beside the exe counts as portable
//! even without the marker, which is what PKG-004 ("use local config by
//! default") asks for. No FFI here: plain path checks.

use std::path::{Path, PathBuf};

/// Marker file in the portable ZIP: its presence beside the exe selects the
/// local `config.toml`.
pub const MARKER: &str = "rustty.portable";

/// Portable config path for the folder holding the executable, or `None`
/// when the folder is not a portable install (no marker, no local config).
#[must_use]
pub fn portable_config_path(exe_dir: &Path) -> Option<PathBuf> {
    let local = exe_dir.join("config.toml");
    if exe_dir.join(MARKER).exists() || local.exists() {
        Some(local)
    } else {
        None
    }
}

/// Portable config path for the running executable, `None` when it is not
/// inside a portable folder.
#[must_use]
pub fn running_config_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    portable_config_path(exe.parent()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Freshly created temp folder for one test; unique per test name.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rustty-portable-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn plain_folder_is_not_portable() {
        let dir = temp_dir("plain");
        assert_eq!(portable_config_path(&dir), None);
        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }

    #[test]
    fn marker_selects_local_config() {
        let dir = temp_dir("marker");
        std::fs::write(dir.join(MARKER), "").expect("write marker");
        assert_eq!(portable_config_path(&dir), Some(dir.join("config.toml")));
        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }

    #[test]
    fn local_config_alone_is_portable() {
        let dir = temp_dir("local");
        std::fs::write(dir.join("config.toml"), "theme = \"predawn\"\n").expect("write config");
        assert_eq!(portable_config_path(&dir), Some(dir.join("config.toml")));
        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }
}
