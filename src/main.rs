//! `rusttyterm` CLI entry point.
//!
//! Stub: loads the configuration and reports it. The window layer
//! (ConPTY + Direct2D) lands in the next milestones (spec §32).

use std::process::ExitCode;

use rusttyterm::config::Config;

fn main() -> ExitCode {
    match Config::load() {
        Ok(cfg) => {
            println!(
                "rusttyterm v{} (stub): config ok, {}x{} grid, theme {}",
                env!("CARGO_PKG_VERSION"),
                cfg.window.cols,
                cfg.window.rows,
                cfg.theme.name,
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("rusttyterm: {e}");
            ExitCode::FAILURE
        }
    }
}
