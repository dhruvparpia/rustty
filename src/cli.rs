//! Minimal command-line parsing (no dependency): `--profile <name>` picks
//! the starting profile, `--theme <name>` overrides the configured theme,
//! and the Jump List tasks (CONFIG-013, THEME-009) launch with these flags.

use std::ffi::OsString;

/// Parsed command line. Unknown arguments are reported and ignored; the
/// app is a GUI, a stray flag must not stop a terminal from opening.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cli {
    /// `--profile <name>`: shell profile for the first tab.
    pub profile: Option<String>,
    /// `--theme <name>`: theme preset or `themes/<name>.toml`.
    pub theme: Option<String>,
    /// `-h`/`--help` was given.
    pub help: bool,
    /// `--version` was given.
    pub version: bool,
    /// Arguments this version does not know.
    pub unknown: Vec<String>,
}

/// Parse `args` (argv without the program name). A flag without its value
/// (last argument) reads as absent.
#[must_use]
pub fn parse<I: IntoIterator<Item = OsString>>(args: I) -> Cli {
    let mut cli = Cli::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy().into_owned();
        match arg.as_str() {
            "--profile" => cli.profile = args.next().map(|v| v.to_string_lossy().into_owned()),
            "--theme" => cli.theme = args.next().map(|v| v.to_string_lossy().into_owned()),
            "-h" | "--help" => cli.help = true,
            "--version" => cli.version = true,
            _ => cli.unknown.push(arg),
        }
    }
    cli
}

/// One-line usage for `--help`.
#[must_use]
pub const fn usage() -> &'static str {
    "usage: rusttyterm [--profile <name>] [--theme <name>] [--version] [-h|--help]"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_profile_and_theme() {
        let cli = parse(os(&["--profile", "pwsh", "--theme", "gruvbox-dark"]));
        assert_eq!(cli.profile.as_deref(), Some("pwsh"));
        assert_eq!(cli.theme.as_deref(), Some("gruvbox-dark"));
        assert!(cli.unknown.is_empty());
    }

    #[test]
    fn unknown_arguments_are_collected_not_fatal() {
        let cli = parse(os(&["--nope", "x"]));
        assert_eq!(cli.unknown, vec!["--nope", "x"]);
    }

    #[test]
    fn flag_without_value_reads_as_absent() {
        let cli = parse(os(&["--profile"]));
        assert_eq!(cli.profile, None);
        assert!(cli.unknown.is_empty());
    }

    #[test]
    fn help_and_version() {
        assert!(parse(os(&["-h"])).help);
        assert!(parse(os(&["--help"])).help);
        assert!(parse(os(&["--version"])).version);
    }
}
