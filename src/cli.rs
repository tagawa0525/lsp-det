//! argv 完結の起動インターフェース (v0.1-design.md 3 章)。
//!
//! ```text
//! lsp-det -- <上流コマンド> [args...]
//! ```
//!
//! フラグは持たない。どの写像を使うかは上流が `InitializeResult.serverInfo`
//! で名乗る名前で決まり (設計 4.2)、時間の非常口もゲートの切り替えもない
//! (ADR 0009 決定 D-10・D-11)。起動指定は呼び出し側の設定ファイル
//! (`.lsp.json` / `ls_args`) に常在させる。

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
        // `--` 以降は上流のもの。lsp-det のフラグと衝突しない。
        let args = parse_args(&s(&["--", "gopls", "--adapter", "x"])).unwrap();
        assert_eq!(
            args.command_args,
            vec!["--adapter".to_string(), "x".to_string()]
        );
    }

    #[test]
    fn rejects_the_removed_flags() {
        // ADR 0009 決定 D-11: CLI は `lsp-det -- <上流コマンド>` のみ。
        // 写像は上流の serverInfo.name で選び (D-2)、時間の非常口も
        // ゲートの切り替えも持たない (D-10、D-1)。
        for argv in [
            &["--adapter", "rust-analyzer", "--", "rust-analyzer"][..],
            &["--gate-timeout", "300", "--", "gopls"][..],
            &["--gate-mode", "error", "--", "gopls"][..],
            &["--no-gate", "--", "gopls"][..],
        ] {
            assert!(
                parse_args(&s(argv)).is_err(),
                "削除済みのフラグを受理した: {argv:?}"
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
