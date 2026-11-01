//! `rusttyterm` CLI entry point: load config, probe a shell, open the
//! terminal window (spec §32, window milestone).

use std::path::Path;
use std::process::ExitCode;

use rusttyterm::config::Config;
use rusttyterm::shell::SpawnOptions;
use rusttyterm::theme::Theme;

#[cfg(windows)]
fn probe_shell(cfg: &Config) -> SpawnOptions {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let (program, args) = if Path::new(r"C:\Program Files\Git\bin\bash.exe").exists() {
        (
            String::from(r"C:\Program Files\Git\bin\bash.exe"),
            vec![String::from("-l")],
        )
    } else if Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe").exists() {
        (
            String::from(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            Vec::new(),
        )
    } else {
        let powershell = Path::new(&sys)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        (powershell.to_string_lossy().into_owned(), Vec::new())
    };
    if program.contains("\\Git\\") {
        // MSYS shells (Git Bash, MSYS2) only attach to a ConPTY when told
        // to use the pseudo-console; without these vars their output never
        // reaches the ConPTY pipes. Set once here in the parent; every
        // ConPTY child inherits the environment (main is single-threaded
        // at this point, so set_var is sound).
        unsafe {
            std::env::set_var("MSYS", "enable_pcon");
            std::env::set_var("MSYS2", "enable_pcon");
        }
    }
    SpawnOptions {
        program,
        args,
        cols: cfg.window.cols,
        rows: cfg.window.rows,
        cwd: None,
        env: std::collections::BTreeMap::new(),
    }
}

#[cfg(windows)]
fn main() -> ExitCode {
    let cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("rusttyterm: {e}");
            return ExitCode::FAILURE;
        }
    };
    let theme = match Theme::preset(&cfg.theme) {
        Some(theme) => theme,
        None => {
            // THEME-014: an unknown preset falls back to predawn.
            eprintln!(
                "rusttyterm: unknown theme {:?} (known: {}), using predawn",
                cfg.theme,
                Theme::preset_names().join(", ")
            );
            Theme::predawn()
        }
    };
    let spawn = probe_shell(&cfg);
    match rusttyterm::window::run(&cfg, theme, spawn) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rusttyterm: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    match Config::load() {
        Ok(cfg) => {
            println!(
                "rusttyterm v{} (no window on this platform): config ok, {}x{} grid, theme {}",
                env!("CARGO_PKG_VERSION"),
                cfg.window.cols,
                cfg.window.rows,
                cfg.theme,
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("rusttyterm: {e}");
            ExitCode::FAILURE
        }
    }
}
