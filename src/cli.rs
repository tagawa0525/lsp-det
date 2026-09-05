//! The launch interface, complete within argv (v0.1-design.md chapter 3).
//!
//! ```text
//! lsp-det -- <upstream command> [args...]
//! ```
//!
//! There are no flags. Which mapping is used is decided by the name the upstream calls itself
//! in `InitializeResult.serverInfo` (design 4.2); there is no time-based escape hatch and no
//! gate switch (ADR 0009 decisions D-10 and D-11). The launch specification lives permanently
//! in the caller's configuration file (`.lsp.json` / `ls_args`).

#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    pub command: String,
    pub command_args: Vec<String>,
}

pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut rest = argv.iter();
    match rest.next().map(String::as_str) {
        Some("--") => {}
        Some(other) => return Err(format!("unknown flag: {other:?}")),
        None => return Err("missing upstream command after `--`".to_string()),
    }
    let command = rest
        .next()
        .ok_or("missing upstream command after `--`")?
        .clone();
    let command_args = rest.cloned().collect();
    Ok(Args {
        command,
        command_args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_minimal_form() {
        let args = parse_args(&s(&["--", "rust-analyzer"])).unwrap();
        assert_eq!(
            args,
            Args {
                command: "rust-analyzer".to_string(),
                command_args: vec![],
            }
        );
    }

    #[test]
    fn parses_command_with_trailing_args() {
        let args = parse_args(&s(&["--", "gopls", "serve", "-v"])).unwrap();
        assert_eq!(args.command, "gopls");
        assert_eq!(
            args.command_args,
            vec!["serve".to_string(), "-v".to_string()]
        );
    }

    #[test]
    fn passes_flag_like_arguments_after_the_separator_to_the_upstream() {
        // Everything after `--` belongs to the upstream. It does not collide with lsp-det flags.
        let args = parse_args(&s(&["--", "gopls", "--adapter", "x"])).unwrap();
        assert_eq!(
            args.command_args,
            vec!["--adapter".to_string(), "x".to_string()]
        );
    }

    #[test]
    fn rejects_the_removed_flags() {
        // ADR 0009 decision D-11: the CLI is only `lsp-det -- <upstream command>`.
        // The mapping is chosen by the upstream's serverInfo.name (D-2); there is no
        // time-based escape hatch and no gate switch (D-10, D-1).
        for argv in [
            &["--adapter", "rust-analyzer", "--", "rust-analyzer"][..],
            &["--gate-timeout", "300", "--", "gopls"][..],
            &["--gate-mode", "error", "--", "gopls"][..],
            &["--no-gate", "--", "gopls"][..],
        ] {
            assert!(
                parse_args(&s(argv)).is_err(),
                "accepted a removed flag: {argv:?}"
            );
        }
    }

    #[test]
    fn errors_when_separator_is_missing() {
        assert!(parse_args(&s(&["rust-analyzer"])).is_err());
    }

    #[test]
    fn errors_when_command_is_missing_after_separator() {
        assert!(parse_args(&s(&["--"])).is_err());
        assert!(parse_args(&s(&[])).is_err());
    }

    #[test]
    fn errors_on_unknown_flag() {
        assert!(parse_args(&s(&["--bogus", "--", "gopls"])).is_err());
    }
}
