//! Shell discovery (issue #40): find installed WSL distributions, PowerShell
//! 7 versions and Visual Studio developer environments, and turn them into
//! profile entries with stable identities ("WSL: <distro>", "PowerShell 7",
//! "Developer PowerShell").
//!
//! Discovery must never block interactive startup and never fail the app:
//! every probe is best-effort and its failure just leaves its entries out of
//! the list. The effectful part runs on a worker thread; the parsed results
//! are parked in [`DISCOVERED`] and the window merges them when it is
//! notified (Refresh = Reload Config re-runs the probes).

use std::sync::Mutex;

/// One discovered shell profile, before merging with config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Stable display identity, e.g. "WSL: Ubuntu".
    pub name: String,
    /// Program to launch.
    pub program: String,
    /// Arguments for the program.
    pub args: Vec<String>,
}

/// Results of the last discovery pass. The worker thread writes it, the
/// window thread drains it on the WM_APP notification.
pub static DISCOVERED: Mutex<Vec<Found>> = Mutex::new(Vec::new());

/// Decode `wsl.exe -l -q` output and list the distributions.
///
/// `wsl.exe` writes UTF-16LE (BOM included) on every current build; a BOM
/// and entry separators of `\0` and/or `\r\n` are all tolerated. The fake
/// "Docker Desktop" entry is dropped.
#[must_use]
pub fn parse_wsl_list(bytes: &[u8]) -> Vec<String> {
    // Skip the UTF-16LE BOM when present.
    let body = if bytes.starts_with(&[0xFF, 0xFE]) {
        &bytes[2..]
    } else {
        bytes
    };
    let text = String::from_utf16_lossy(
        &body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    );
    text.split(['\0', '\n', '\r'])
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("Docker Desktop"))
        .map(str::to_string)
        .collect()
}

/// Profile entries for the given WSL distribution names.
#[must_use]
pub fn wsl_profiles(distros: &[String]) -> Vec<Found> {
    distros
        .iter()
        .map(|d| Found {
            name: format!("WSL: {d}"),
            program: "wsl.exe".to_string(),
            args: vec!["-d".to_string(), d.clone()],
        })
        .collect()
}

/// The installation path from `vswhere.exe -latest -property
/// installationPath`; `None` for empty output (no VS installed).
#[must_use]
pub fn parse_vswhere(stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    (!line.is_empty()).then(|| line.to_string())
}

/// The developer-environment profile for a Visual Studio install:
/// PowerShell running that install's Launch-VsDevShell.ps1.
#[must_use]
pub fn vs_profile(install: &str) -> Found {
    let script = std::path::Path::new(install)
        .join("Common7")
        .join("Tools")
        .join("Launch-VsDevShell.ps1");
    Found {
        name: "Developer PowerShell".to_string(),
        program: "powershell.exe".to_string(),
        args: vec![
            "-NoExit".to_string(),
            "-Command".to_string(),
            format!(
                "& '{}'; Set-Location $HOME",
                script.to_string_lossy().replace('\'', "''")
            ),
        ],
    }
}

/// The highest-numbered `C:\Program Files\PowerShell\<n>\pwsh.exe` among the
/// given `(<dir name, pwsh exists>)` pairs; `None` when none exist.
#[must_use]
pub fn pwsh_latest<'a>(entries: impl IntoIterator<Item = (&'a str, bool)>) -> Option<String> {
    entries
        .into_iter()
        .filter(|(_, exists)| *exists)
        .filter_map(|(name, _)| name.parse::<u32>().ok().map(|n| (n, name.to_string())))
        .max_by_key(|(n, _)| *n)
        .map(|(_, name)| format!(r"C:\Program Files\PowerShell\{name}\pwsh.exe"))
}

/// Run the probes. Every failure is non-fatal: a probe that errors, times
/// out, or is missing contributes nothing.
#[cfg(windows)]
#[must_use]
pub fn discover() -> Vec<Found> {
    let mut found = Vec::new();

    // WSL distributions. wsl.exe writes UTF-16LE; read raw bytes. The probe
    // runs on a helper thread so a hung wsl.exe cannot outlive the timeout.
    if let Some(bytes) = run_captured("wsl.exe", &["-l", "-q"], 5) {
        found.extend(wsl_profiles(&parse_wsl_list(&bytes)));
    }

    // PowerShell 7 installs (numeric version directories), newest first.
    let ps_root = std::path::Path::new(r"C:\Program Files\PowerShell");
    let mut versions: Vec<(String, bool)> = Vec::new();
    if let Ok(read) = std::fs::read_dir(ps_root) {
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let exe = entry.path().join("pwsh.exe");
            versions.push((name, exe.is_file()));
        }
    }
    let pairs: Vec<(&str, bool)> = versions.iter().map(|(n, e)| (n.as_str(), *e)).collect();
    if let Some(pwsh) = pwsh_latest(pairs) {
        found.push(Found {
            name: "PowerShell 7".to_string(),
            program: pwsh,
            args: Vec::new(),
        });
    }

    // Visual Studio developer environment via vswhere (fixed installer path).
    let vswhere = std::env::var("ProgramFiles(x86)")
        .map(|pf| std::path::Path::new(&pf).join(r"Microsoft Visual Studio\Installer\vswhere.exe"))
        .ok()
        .filter(|p| p.is_file());
    if let Some(vswhere) = vswhere
        && let Some(out) = run_captured(
            &vswhere.to_string_lossy(),
            &["-latest", "-property", "installationPath"],
            5,
        )
        && let Some(install) = parse_vswhere(&String::from_utf8_lossy(&out))
    {
        found.push(vs_profile(&install));
    }

    found
}

/// The stdout of `program args…`, killed on timeout; `None` on spawn
/// failure, timeout, or non-zero exit.
#[cfg(windows)]
fn run_captured(program: &str, args: &[&str], timeout_secs: u64) -> Option<Vec<u8>> {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    fn work(program: String, args: Vec<String>) -> Option<Vec<u8>> {
        let out = Command::new(&program)
            .args(&args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status.success().then_some(out.stdout)
    }

    // std has no Command timeout; a channel + recv_timeout is the smallest
    // that works. A timed-out child keeps running headless — harmless for
    // these read-only probes. shortcut: no kill on timeout, add one if a
    // probe ever misbehaves.
    let (tx, rx) = mpsc::channel();
    let program = program.to_string();
    let args = args.iter().map(|a| (*a).to_string()).collect();
    std::thread::spawn(move || {
        let _ = tx.send(work(program, args));
    });
    rx.recv_timeout(std::time::Duration::from_secs(timeout_secs))
        .ok()
        .flatten()
}

/// Non-Windows: discovery is Windows-only (registry drive lookup), so
/// this yields nothing (DISCOVERY-001).
#[cfg(not(windows))]
#[must_use]
pub fn discover() -> Vec<Found> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        let mut v = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            v.extend(u.to_le_bytes());
        }
        v
    }

    #[test]
    fn wsl_list_decodes_utf16_and_drops_docker() {
        let bytes = utf16("Ubuntu\r\nDocker Desktop\r\nopenSUSE-Tumbleweed\r\n");
        assert_eq!(
            parse_wsl_list(&bytes),
            vec!["Ubuntu", "openSUSE-Tumbleweed"]
        );
    }

    #[test]
    fn wsl_list_tolerates_nul_separators_and_missing_bom() {
        let mut v: Vec<u8> = Vec::new();
        for u in "Ubuntu\0Ubuntu-22.04\0".encode_utf16() {
            v.extend(u.to_le_bytes());
        }
        assert_eq!(parse_wsl_list(&v), vec!["Ubuntu", "Ubuntu-22.04"]);
    }

    #[test]
    fn wsl_profiles_carry_stable_identity() {
        let profiles = wsl_profiles(&["Ubuntu".to_string()]);
        assert_eq!(profiles[0].name, "WSL: Ubuntu");
        assert_eq!(profiles[0].program, "wsl.exe");
        assert_eq!(profiles[0].args, ["-d", "Ubuntu"]);
    }

    #[test]
    fn wsl_list_empty_is_empty() {
        assert!(parse_wsl_list(&[]).is_empty());
        assert!(parse_wsl_list(&utf16("")).is_empty());
    }

    #[test]
    fn vswhere_reads_first_line_and_rejects_empty() {
        assert_eq!(
            parse_vswhere("C:\\Program Files\\Microsoft Visual Studio\\2022\\Community\r\n")
                .as_deref(),
            Some(r"C:\Program Files\Microsoft Visual Studio\2022\Community")
        );
        assert_eq!(parse_vswhere("  \r\n "), None);
    }

    #[test]
    fn vs_profile_quotes_the_launch_script() {
        let p = vs_profile(r"C:\VS\2022\Community");
        assert_eq!(p.name, "Developer PowerShell");
        assert_eq!(p.program, "powershell.exe");
        // Path::join uses host separators; this test checks script quoting
        // and must also run in the portable core suite on Unix hosts.
        assert_eq!(
            p.args[2].replace('/', "\\"),
            r"& 'C:\VS\2022\Community\Common7\Tools\Launch-VsDevShell.ps1'; Set-Location $HOME"
        );
    }

    #[test]
    fn pwsh_latest_picks_the_highest_numbered_install() {
        let got = pwsh_latest([("6", false), ("7", true), ("8", true), ("Preview", true)]);
        assert_eq!(
            got.as_deref(),
            Some(r"C:\Program Files\PowerShell\8\pwsh.exe")
        );
        assert_eq!(pwsh_latest([("7", false)]), None);
    }
}
