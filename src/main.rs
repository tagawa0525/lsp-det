use std::env;
use std::io;
use std::process::ExitCode;

use lsp_det::{adapter, cli, proxy};

#[cfg(unix)]
use lsp_det::process;

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().skip(1).collect();
    let args = match cli::parse_args(&argv) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("lsp-det: {err}");
            eprintln!(
                "usage: lsp-det [--adapter <name>] [--gate-timeout <sec>] [--no-gate] [--gate-mode <hold|error>] -- <command> [args...]"
            );
            return ExitCode::from(2);
        }
    };

    let adapter = match args.adapter.as_deref() {
        None => None,
        Some("rust-analyzer") => Some(adapter::RustAnalyzerAdapter::new()),
        Some(other) => {
            eprintln!("lsp-det: unknown adapter {other:?} (available: rust-analyzer)");
            return ExitCode::from(2);
        }
    };

    // 親 (クライアント) が死んだらこのプロセスも終了する。
    // main の最初、他の処理より先に設定する (v0.1-design.md 4.7)。
    #[cfg(unix)]
    process::set_self_pdeathsig();

    match proxy::run(
        io::stdin(),
        io::stdout(),
        &args.command,
        &args.command_args,
        adapter,
    ) {
        Ok(code) => exit_code_from(code),
        Err(err) => {
            eprintln!("lsp-det: failed to run upstream {:?}: {err}", args.command);
            ExitCode::from(1)
        }
    }
}

fn exit_code_from(code: i32) -> ExitCode {
    ExitCode::from(code.clamp(0, 255) as u8)
}
