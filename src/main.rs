//! `rustty` CLI entry point: load config, probe a shell, open the
//! terminal window (spec §32, window milestone).

// Console subsystem (issue #47): `rustty --attach <n>` must run in
// the caller's console in the foreground (PowerShell only waits for
// console-subsystem processes), which a GUI-subsystem exe cannot do.
// A GUI launch closes the allocated console window right away.

use std::process::ExitCode;

use rustty::config::Config;
use rustty::theme;

#[cfg(windows)]
fn main() -> ExitCode {
    let cli = rustty::cli::parse(std::env::args_os().skip(1));
    // SECRET-008: `process::exit` because `secrets run` returns the command's
    // full 32-bit exit code, which `ExitCode` cannot carry.
    if let Some(parsed) = &cli.secrets {
        rustty::cli::attach_parent_console();
        std::process::exit(rustty::secrets::run_cli(parsed));
    }
    // AUTO-012/013: these reach a running window and never open one. `mcp` speaks
    // over its standard streams, so it must not touch the console attachment.
    if let Some(args) = &cli.ctl {
        rustty::cli::attach_parent_console();
        return ExitCode::from(rustty::automation::ctl::main(args));
    }
    if cli.mcp {
        return ExitCode::from(rustty::automation::mcp::main());
    }
    let is_session_verb = cli.list_sessions
        || cli.new_session
        || cli.session_info.is_some()
        || cli.terminate_session.is_some()
        || cli.attach.is_some();
    if cli.help || cli.version || cli.workspaces || is_session_verb {
        // Launched by a taskbar/Jump List task with no parent console,
        // the console subsystem allocated a window; attach to the
        // calling console so output prints where it was typed, keeping
        // any redirection intact. From a real console this is a no-op.
        rustty::cli::attach_parent_console();
    } else {
        // GUI launch: close the console window the subsystem allocated.
        // shortcut: it flashes briefly on double-click; a GUI-subsystem
        // launcher stub hides it if that ever matters.
        rustty::cli::free_console();
    }
    for unknown in &cli.unknown {
        eprintln!("rustty: ignoring unknown argument {unknown:?}");
    }
    if cli.help {
        println!("{}", rustty::cli::usage());
        return ExitCode::SUCCESS;
    }
    if cli.version {
        println!("rustty {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if cli.workspaces {
        // WORKSPACE-004: print the template names, one per line.
        for name in rustty::workspace::names() {
            println!("{name}");
        }
        return ExitCode::SUCCESS;
    }
    let mut cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("rustty: {e}");
            return ExitCode::FAILURE;
        }
    };
    // CONFIG-013: `--profile` beats the config's `starting-profile`
    // (the Jump List's per-profile tasks launch with this flag).
    if let Some(profile) = &cli.profile {
        cfg.starting_profile = profile.clone();
    }
    // Issue #46: session host verbs. They print and exit; a failing
    // verb must never fall through to opening a GUI window.
    if is_session_verb {
        return session_verb(&cli);
    }
    // THEME-009/013/014: preset name, then themes/<name>.toml next to the
    // config, then the [theme] colour overrides. Problems are reported and
    // the nearest working theme is used. `--theme` beats the config.
    let theme_dir =
        rustty::config::config_path().and_then(|p| p.parent().map(|dir| dir.join("themes")));
    // THEME-010: an empty theme name follows the system dark/light setting.
    let theme_name: &str = match &cli.theme {
        Some(name) => name,
        None if cfg.theme.name().is_empty() => theme::Theme::system_preset(),
        None => cfg.theme.name(),
    };
    let (theme, problems) =
        theme::resolve_configured(theme_name, &cfg.theme.overrides(), theme_dir.as_deref());
    for problem in &problems {
        eprintln!("rustty: {problem}");
    }
    let mut spawn = rustty::shell::probe_shell(&cfg);
    // Issue #40: a positional directory ("Open terminal here" from
    // Explorer, or a plain `rustty C:\dir`) sets the first tab's
    // working directory. Not a directory: report and open as usual.
    if let Some(dir) = cli.directory {
        if std::path::Path::new(&dir).is_dir() {
            spawn.cwd = Some(dir);
        } else {
            eprintln!("rustty: {dir:?} is not a directory; opening in the default location");
        }
    }
    match rustty::window::run(&cfg, theme, spawn, cli.workspace.as_deref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rustty: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Session host verbs (issue #46): talk to the per-user `rustty-host`
/// and print its answer. Stable exit codes per the attach contract:
/// 2 usage, 3 not found, 5 host unavailable, 6 protocol incompatible;
/// a failing verb never opens a window.
#[cfg(windows)]
fn session_verb(cli: &rustty::cli::Cli) -> ExitCode {
    use std::process::ExitCode;

    use rustty::host::{EXIT_NOT_FOUND, EXIT_PROTOCOL, EXIT_USAGE, client};
    use serde_json::json;

    if cli.new_session && cli.session_name.as_deref().is_some_and(|s| s.is_empty()) {
        eprintln!("rustty: --name needs a value");
        return ExitCode::from(EXIT_USAGE);
    }
    // Console attach (issue #47). Parsed in cli.rs so a malformed verb
    // fails visibly and never falls through to opening a GUI window
    // (docs/design-session-attach.md). Attaching with --profile is an
    // error: attach never replaces the session's original profile.
    if cli.attach.is_some() && cli.profile.is_some() {
        eprintln!("rustty: --attach does not accept --profile");
        return ExitCode::from(EXIT_USAGE);
    }
    let want_alias = cli.attach.or(cli.session_info).or(cli.terminate_session);
    if want_alias.is_some() && want_alias == Some(0) {
        // A malformed alias (not a positive number) is a usage error.
        eprintln!("rustty: session alias must be a positive number");
        return ExitCode::from(EXIT_USAGE);
    }
    if let Some(alias) = cli.attach {
        if cli.read_only && cli.take_control {
            eprintln!("rustty: --read-only and --take-control are mutually exclusive");
            return ExitCode::from(EXIT_USAGE);
        }
        // The detach prefix comes from the config; a broken config
        // falls back to defaults like every other entry point.
        let cfg = match Config::load() {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("rustty: {e}; using defaults");
                Config::default()
            }
        };
        let prefix = match rustty::attach::parse_prefix(&cfg.detach_prefix) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("rustty: {e}");
                return ExitCode::from(EXIT_USAGE);
            }
        };
        return match rustty::attach::run(alias, cli.read_only, cli.take_control, prefix) {
            Ok(code) => ExitCode::from(code),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
                ) =>
            {
                // Not a TTY, or a self-attach: usage-class failures.
                eprintln!("rustty: {e}");
                ExitCode::from(EXIT_USAGE)
            }
            Err(e) => {
                eprintln!("rustty: {e}");
                ExitCode::from(rustty::host::EXIT_HOST_UNAVAILABLE)
            }
        };
    }

    let mut c = match client::Client::connect() {
        Ok(c) => c,
        Err(e) => {
            if cli.list_sessions && e.raw_os_error() == Some(2) {
                // No host and none spawnable: nothing is running, which
                // is the honest empty answer (exit 0, per the contract).
                // Any other connect error is a real failure, not "empty".
                if cli.json {
                    println!("{{\"schema\":\"host-session/v1\",\"sessions\":[]}}");
                } else {
                    println!("no sessions");
                }
                return ExitCode::SUCCESS;
            }
            let code = if e.kind() == std::io::ErrorKind::ConnectionRefused {
                EXIT_PROTOCOL
            } else {
                rustty::host::EXIT_HOST_UNAVAILABLE
            };
            eprintln!("rustty: {e}");
            return ExitCode::from(code);
        }
    };

    let (method, params) = if cli.new_session {
        (
            "session.create",
            json!({
                "profile": cli.profile,
                "name": cli.session_name,
                "cwd": cli.directory,
                "cols": cli.session_cols,
                "rows": cli.session_rows,
                "persistent": !cli.ephemeral,
            }),
        )
    } else if let Some(alias) = cli.session_info {
        ("session.info", json!({"alias": alias}))
    } else if let Some(alias) = cli.terminate_session {
        (
            "session.terminate",
            json!({"alias": alias, "force": cli.force}),
        )
    } else {
        ("session.list", json!({}))
    };

    let resp = match c.call(method, params) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("rustty: {e}");
            return ExitCode::from(rustty::host::EXIT_HOST_UNAVAILABLE);
        }
    };
    if let Some(err) = resp.get("error") {
        let code = err
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("internal");
        let message = err
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        eprintln!("rustty: {code}: {message}");
        return ExitCode::from(if code == "target-stale" {
            EXIT_NOT_FOUND
        } else {
            1
        });
    }
    let result = resp
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if cli.json {
        println!("{result}");
        return ExitCode::SUCCESS;
    }
    match method {
        "session.list" => {
            let sessions = result.get("sessions").and_then(serde_json::Value::as_array);
            let Some(sessions) = sessions else {
                println!("no sessions");
                return ExitCode::SUCCESS;
            };
            if sessions.is_empty() {
                println!("no sessions");
                return ExitCode::SUCCESS;
            }
            println!("#  NAME          PROFILE  STATE   PID       CWD");
            for s in sessions {
                let g = |k: &str| {
                    s.get(k)
                        .map(serde_json::Value::to_string)
                        .unwrap_or_default()
                        .trim_matches('"')
                        .to_owned()
                };
                println!(
                    "{:<3} {:<13} {:<8} {:<5} {:<9} {}",
                    g("alias"),
                    g("name"),
                    g("profile"),
                    g("state"),
                    g("pid"),
                    g("cwd"),
                );
            }
        }
        "session.info" => {
            let Some(s) = result.get("session") else {
                println!("no session");
                return ExitCode::SUCCESS;
            };
            for k in [
                "alias",
                "id",
                "name",
                "profile",
                "program",
                "cwd",
                "cols",
                "rows",
                "pid",
                "persistent",
                "state",
                "exitCode",
            ] {
                if let Some(v) = s.get(k) {
                    println!(
                        "{k}: {}",
                        v.as_str().map(str::to_owned).unwrap_or(v.to_string())
                    );
                }
            }
        }
        "session.create" => {
            let alias = result
                .get("session")
                .and_then(|s| s.get("alias"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            println!("created session {alias}");
        }
        _ => {
            let alias = result
                .get("session")
                .and_then(|s| s.get("alias"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            println!("session {alias} terminated");
        }
    }
    ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    let cli = rustty::cli::parse(std::env::args_os().skip(1));
    if let Some(parsed) = &cli.secrets {
        std::process::exit(rustty::secrets::run_cli(parsed));
    }
    if let Some(args) = &cli.ctl {
        return ExitCode::from(rustty::automation::ctl::main(args));
    }
    if cli.mcp {
        return ExitCode::from(rustty::automation::mcp::main());
    }
    for unknown in &cli.unknown {
        eprintln!("rustty: ignoring unknown argument {unknown:?}");
    }
    if cli.help {
        println!("{}", rustty::cli::usage());
        return ExitCode::SUCCESS;
    }
    if cli.version {
        println!("rustty {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if cli.workspaces {
        // WORKSPACE-004: print the template names, one per line.
        for name in rustty::workspace::names() {
            println!("{name}");
        }
        return ExitCode::SUCCESS;
    }
    match Config::load() {
        Ok(cfg) => {
            println!(
                "rustty v{} (no window on this platform): config ok, {}x{} grid, theme {}",
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
            eprintln!("rustty: {e}");
            ExitCode::FAILURE
        }
    }
}
