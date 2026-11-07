//! Finding the file a program name means (SHELL-030, SECRET-011), without the
//! search `CreateProcessW` does when it isn't given one. That search looks in the
//! current folder first, so a repository could plant `claude.exe` or `git.exe` and
//! have it run for anything started from there.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// File types `CreateProcessW` can start, in the order a shell tries them.
const RUNNABLE_EXTENSIONS: [&str; 4] = [".com", ".exe", ".bat", ".cmd"];

/// `base` as a runnable file: itself if it already has a runnable extension,
/// then with each runnable extension appended.
fn runnable_file(base: &Path, is_file: &impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let has_runnable_extension = base.extension().is_some_and(|ext| {
        RUNNABLE_EXTENSIONS
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(&known[1..]))
    });
    if has_runnable_extension && is_file(base) {
        return Some(base.to_path_buf());
    }
    RUNNABLE_EXTENSIONS.iter().find_map(|extension| {
        let mut name = base.as_os_str().to_os_string();
        name.push(extension);
        let candidate = PathBuf::from(name);
        is_file(&candidate).then_some(candidate)
    })
}

/// Whether a name says where it is: anything with a separator or a drive.
#[must_use]
pub fn has_path_part(command: &OsStr) -> bool {
    command
        .as_encoded_bytes()
        .iter()
        .any(|byte| matches!(byte, b'\\' | b'/' | b':'))
}

/// The folders a bare name may come from, in order: the Windows folders first, as
/// `CreateProcessW` has them, then `path_var`'s absolute entries. A relative entry,
/// which means wherever the process happens to be, is left out.
#[must_use]
pub fn search_dirs(windows_dirs: &[PathBuf], path_var: Option<&OsStr>) -> Vec<PathBuf> {
    let on_path = path_var
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|dir| dir.is_absolute());
    windows_dirs.iter().cloned().chain(on_path).collect()
}

/// The first file `command` names in `dirs`, which [`search_dirs`] made safe.
#[must_use]
pub fn find_in_dirs(
    command: &OsStr,
    dirs: &[PathBuf],
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    dirs.iter()
        .find_map(|dir| runnable_file(&dir.join(command), &is_file))
}

/// Find the file a command names. An explicit path is used as given. A bare
/// name is searched on the absolute entries of `path_var` only, so the current
/// directory never supplies a command by accident: a repository cannot plant
/// `git.exe` for `secrets run -- git` to hand secrets to.
pub fn resolve_command(
    command: &OsStr,
    path_var: Option<&OsStr>,
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if has_path_part(command) {
        return runnable_file(Path::new(command), &is_file);
    }
    find_in_dirs(command, &search_dirs(&[], path_var), is_file)
}

/// Whether a path is a file a process can be started from. An App Execution Alias
/// in `WindowsApps` is a reparse point that opening refuses, so following it says
/// nothing is there; asking about the link itself finds it.
#[must_use]
pub fn is_runnable_file(path: &Path) -> bool {
    path.is_file()
        || path
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_file())
}

/// True for a file only `cmd.exe` can run.
#[must_use]
pub fn is_batch(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

#[cfg(all(test, windows))]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn a_relative_path_entry_is_never_searched() {
        // Break caught: `.` or an empty entry on PATH bringing the current folder back.
        let path = OsString::from(r".;tools;;C:\real;..\up;\rooted;C:drive");
        let dirs = search_dirs(&[PathBuf::from(r"C:\Windows")], Some(&path));
        assert_eq!(
            dirs,
            [PathBuf::from(r"C:\Windows"), PathBuf::from(r"C:\real")]
        );
        let found = find_in_dirs(OsStr::new("x"), &dirs, |p| p == Path::new(r"tools\x.exe"));
        assert_eq!(found, None);
    }

    #[test]
    fn the_windows_folders_win_over_path() {
        // As CreateProcessW has them, so `cmd.exe` still means the system's.
        let path = OsString::from(r"C:\planted");
        let dirs = search_dirs(&[PathBuf::from(r"C:\Windows\System32")], Some(&path));
        let found = find_in_dirs(OsStr::new("cmd.exe"), &dirs, |_| true);
        assert_eq!(found, Some(PathBuf::from(r"C:\Windows\System32\cmd.exe")));
    }

    #[test]
    fn a_name_with_any_path_part_is_explicit() {
        for explicit in [r".\x", "./x", r"C:\x", "C:x", r"..\x", r"dir\x"] {
            assert!(has_path_part(OsStr::new(explicit)), "{explicit}");
        }
        for bare in ["x", "x.exe", "claude", "my tool.exe"] {
            assert!(!has_path_part(OsStr::new(bare)), "{bare}");
        }
    }

    #[test]
    fn an_app_execution_alias_counts_as_a_file() {
        // Break caught: Path::is_file saying no to them, so `pwsh.exe` and `wsl.exe`
        // installed from the Store stop resolving.
        let Some(dir) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let dir = Path::new(&dir).join(r"Microsoft\WindowsApps");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let alias = path.extension().is_some_and(|ext| ext == "exe")
                && entry
                    .metadata()
                    .is_ok_and(|meta| meta.len() == 0 && !meta.is_dir());
            if alias {
                assert!(is_runnable_file(&path), "{}", path.display());
            }
        }
    }

    #[test]
    fn a_folder_or_a_missing_file_is_not_runnable() {
        assert!(!is_runnable_file(&std::env::temp_dir()));
        assert!(!is_runnable_file(Path::new(r"C:\no\such\file.exe")));
    }
}
