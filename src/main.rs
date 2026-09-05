use std::env;
use std::io;
use std::process::ExitCode;

use lsp_det::{cli, process, proxy};

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().skip(1).collect();
    let args = match cli::parse_args(&argv) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("lsp-det: {err}");
            eprintln!("usage: lsp-det -- <command> [args...]");
            return ExitCode::from(2);
        }
    };

    // Exit this process too when the parent (the client) dies unexpectedly.
    // Set this up at the start of main, before anything else (v0.1-design.md 4.5, ADR 0012).
    process::exit_with_parent();

    match proxy::run(io::stdin(), io::stdout(), &args.command, &args.command_args) {
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
