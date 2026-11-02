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
    /// `--workspace <name>`: open this workspace template (WORKSPACE-004).
    pub workspace: Option<String>,
    /// `--workspaces`: print the template names and exit.
    pub workspaces: bool,
    /// First positional argument: directory to open the terminal in
    /// (issue #40, "Open terminal here" from Explorer).
    pub directory: Option<String>,
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
            "--workspace" => {
                cli.workspace = args.next().map(|v| v.to_string_lossy().into_owned());
            }
            "--workspaces" => cli.workspaces = true,
            "-h" | "--help" => cli.help = true,
            "--version" => cli.version = true,
            // First positional is the working directory (Open terminal
            // here); any later positional stays unknown.
            _ if cli.directory.is_none() && !arg.starts_with('-') => {
                cli.directory = Some(arg);
            }
            _ => cli.unknown.push(arg),
        }
    }
    cli
}

/// One-line usage for `--help`.
#[must_use]
pub const fn usage() -> &'static str {
    "usage: rusttyterm [directory] [--profile <name>] [--theme <name>] [--workspace <name> | --workspaces] [--version] [-h|--help]"
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
        // The first positional is the directory; the unknown flag stands alone.
        assert_eq!(cli.unknown, vec!["--nope"]);
        assert_eq!(cli.directory.as_deref(), Some("x"));
    }

    #[test]
    fn first_positional_is_the_open_directory() {
        let cli = parse(os(&[r"C:\My Docs", "--profile", "pwsh"]));
        assert_eq!(cli.directory.as_deref(), Some(r"C:\My Docs"));
        assert_eq!(cli.profile.as_deref(), Some("pwsh"));
        assert!(cli.unknown.is_empty());
        // A second positional is not a directory.
        let cli = parse(os(&["C:\\a", "C:\\b"]));
        assert_eq!(cli.directory.as_deref(), Some("C:\\a"));
        assert_eq!(cli.unknown, vec!["C:\\b"]);
    }

    #[test]
    fn flag_without_value_reads_as_absent() {
        let cli = parse(os(&["--profile"]));
        assert_eq!(cli.profile, None);
        assert!(cli.unknown.is_empty());
    }

    #[test]
    fn workspace_flags() {
        let cli = parse(os(&["--workspace", "Web App"]));
        assert_eq!(cli.workspace.as_deref(), Some("Web App"));
        assert!(!cli.workspaces);
        assert!(parse(os(&["--workspaces"])).workspaces);
        assert_eq!(parse(os(&["--workspace"])).workspace, None);
    }

    #[test]
    fn help_and_version() {
        assert!(parse(os(&["-h"])).help);
        assert!(parse(os(&["--help"])).help);
        assert!(parse(os(&["--version"])).version);
    }
}
