//! 擬似クライアント。プロセス寿命のテスト（`tests/process_lifetime.rs`）専用。
//!
//! 引数のコマンドを、自分の stdin / stdout / stderr を継承させて起動し、
//! 子の pid を stderr に出してから、殺されるまで何もしない。
//!
//! stdin を継承させるのが要点である。テストが持つパイプを子（lsp-det）が
//! 直接持つので、この擬似クライアントを殺しても子の stdin は閉じない。
//! EOF では終了しない状況を作り、OS の機構（ADR 0012 決定 B）だけが子を
//! 終了させることを確かめる。

use std::process::{Command, Stdio};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((program, rest)) = args.split_first() else {
        eprintln!("usage: pseudo_client <command> [args...]");
        std::process::exit(2);
    };
    let child = Command::new(program)
        .args(rest)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|err| {
            eprintln!("pseudo-client: cannot start {program:?}: {err}");
            std::process::exit(1);
        });
    eprintln!("pseudo-client: child pid {}", child.id());
    // 殺されるまで待つ。子を wait しないので、子が先に終わっても戻らない。
    loop {
        std::thread::park();
    }
}
