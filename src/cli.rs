//! argv 完結の起動インターフェース (v0.1-design.md 3章)。
//!
//! ```text
//! lsp-det --adapter <name> [--gate-timeout <sec>] [--no-gate] [--gate-mode <hold|error>] -- <上流コマンド> [args...]
//! ```
//!
//! M1 では `--adapter` / `--gate-timeout` / `--no-gate` / `--gate-mode` は
//! 受理するが未使用 (ゲート未実装のため)。M2 で意味を持たせる。

#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    pub adapter: Option<String>,
    pub gate_timeout: Option<u64>,
    pub no_gate: bool,
    pub gate_mode: Option<String>,
    pub command: String,
    pub command_args: Vec<String>,
}

pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut adapter = None;
    let mut gate_timeout = None;
    let mut no_gate = false;
    let mut gate_mode = None;

    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--" => {
                i += 1;
                break;
            }
            "--adapter" => {
                i += 1;
                let value = argv.get(i).ok_or("--adapter requires a value")?;
                adapter = Some(value.clone());
                i += 1;
            }
            "--gate-timeout" => {
                i += 1;
                let value = argv.get(i).ok_or("--gate-timeout requires a value")?;
                gate_timeout = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid --gate-timeout value: {value:?}"))?,
                );
                i += 1;
            }
            "--gate-mode" => {
                i += 1;
                let value = argv.get(i).ok_or("--gate-mode requires a value")?;
                gate_mode = Some(value.clone());
                i += 1;
            }
            "--no-gate" => {
                no_gate = true;
                i += 1;
            }
            other => {
                return Err(format!("unknown flag: {other:?}"));
            }
        }
    }

    let mut rest = argv[i..].iter();
    let command = rest
        .next()
        .ok_or("missing upstream command after `--`")?
        .clone();
    let command_args = rest.cloned().collect();

    Ok(Args {
        adapter,
        gate_timeout,
        no_gate,
        gate_mode,
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
                adapter: None,
                gate_timeout: None,
                no_gate: false,
                gate_mode: None,
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
    fn parses_adapter_flag() {
        let args = parse_args(&s(&["--adapter", "rust-analyzer", "--", "rust-analyzer"])).unwrap();
        assert_eq!(args.adapter, Some("rust-analyzer".to_string()));
    }

    #[test]
    fn parses_gate_timeout_flag() {
        let args = parse_args(&s(&["--gate-timeout", "300", "--", "gopls"])).unwrap();
        assert_eq!(args.gate_timeout, Some(300));
    }

    #[test]
    fn parses_no_gate_flag() {
        let args = parse_args(&s(&["--no-gate", "--", "gopls"])).unwrap();
        assert!(args.no_gate);
    }

    #[test]
    fn parses_gate_mode_flag() {
        let args = parse_args(&s(&["--gate-mode", "error", "--", "gopls"])).unwrap();
        assert_eq!(args.gate_mode, Some("error".to_string()));
    }

    #[test]
    fn parses_all_flags_together_in_any_order() {
        let args = parse_args(&s(&[
            "--gate-mode",
            "error",
            "--no-gate",
            "--adapter",
            "gopls",
            "--gate-timeout",
            "60",
            "--",
            "gopls",
            "serve",
        ]))
        .unwrap();
        assert_eq!(args.adapter, Some("gopls".to_string()));
        assert_eq!(args.gate_timeout, Some(60));
        assert!(args.no_gate);
        assert_eq!(args.gate_mode, Some("error".to_string()));
        assert_eq!(args.command, "gopls");
        assert_eq!(args.command_args, vec!["serve".to_string()]);
    }

    #[test]
    fn errors_when_separator_is_missing() {
        assert!(parse_args(&s(&["rust-analyzer"])).is_err());
    }

    #[test]
    fn errors_when_command_is_missing_after_separator() {
        assert!(parse_args(&s(&["--"])).is_err());
    }

    #[test]
    fn errors_on_unknown_flag() {
        assert!(parse_args(&s(&["--bogus", "--", "gopls"])).is_err());
    }

    #[test]
    fn errors_on_invalid_gate_timeout_value() {
        assert!(parse_args(&s(&["--gate-timeout", "not-a-number", "--", "gopls"])).is_err());
    }
}
