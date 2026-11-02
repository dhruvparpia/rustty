//! `rusttyterm` CLI entry point: load config, probe a shell, open the
//! terminal window (spec §32, window milestone).

#[cfg(windows)]
use std::path::Path;
use std::process::ExitCode;

use rusttyterm::config::Config;
#[cfg(windows)]
use rusttyterm::shell::SpawnOptions;
use rusttyterm::theme;

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
    let cli = rusttyterm::cli::parse(std::env::args_os().skip(1));
    for unknown in &cli.unknown {
        eprintln!("rusttyterm: ignoring unknown argument {unknown:?}");
    }
    if cli.help {
        println!("{}", rusttyterm::cli::usage());
        return ExitCode::SUCCESS;
    }
    if cli.version {
        println!("rusttyterm {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let mut cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("rusttyterm: {e}");
            return ExitCode::FAILURE;
        }
    };
    // CONFIG-013: `--profile` beats the config's `starting-profile`
    // (the Jump List's per-profile tasks launch with this flag).
    if let Some(profile) = cli.profile {
        cfg.starting_profile = profile;
    }
    // THEME-009/013/014: preset name, then themes/<name>.toml next to the
    // config, then the [theme] colour overrides. Problems are reported and
    // the nearest working theme is used. `--theme` beats the config.
    let theme_dir =
        rusttyterm::config::config_path().and_then(|p| p.parent().map(|dir| dir.join("themes")));
    // THEME-010: an empty theme name follows the system dark/light setting.
    let theme_name: &str = match &cli.theme {
        Some(name) => name,
        None if cfg.theme.name().is_empty() => theme::Theme::system_preset(),
        None => cfg.theme.name(),
    };
    let (theme, problems) =
        theme::resolve_configured(theme_name, &cfg.theme.overrides(), theme_dir.as_deref());
    for problem in &problems {
        eprintln!("rusttyterm: {problem}");
    }
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
    let cli = rusttyterm::cli::parse(std::env::args_os().skip(1));
    for unknown in &cli.unknown {
        eprintln!("rusttyterm: ignoring unknown argument {unknown:?}");
    }
    if cli.help {
        println!("{}", rusttyterm::cli::usage());
        return ExitCode::SUCCESS;
    }
    if cli.version {
        println!("rusttyterm {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    match Config::load() {
        Ok(cfg) => {
            println!(
                "rusttyterm v{} (no window on this platform): config ok, {}x{} grid, theme {}",
                env!("CARGO_PKG_VERSION"),
                cfg.window.cols,
                cfg.window.rows,
                cli.theme.unwrap_or_else(|| {
                    if cfg.theme.name().is_empty() {
                        theme::Theme::system_preset().to_owned()
                    } else {
                        cfg.theme.name().to_owned()
                    }
                }),
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("rusttyterm: {e}");
            ExitCode::FAILURE
        }
    }
}
