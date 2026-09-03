//! 上流に出す変更の受け入れ条件（ローカル専用。すべて `#[ignore]`）。
//!
//! `scripts/upstream/build-*.sh` で `reference/` の clone をビルドし、
//! `target/upstream/bin` を PATH の先頭に置いて実行する:
//!
//! ```text
//! PATH="$PWD/target/upstream/bin:$PATH" cargo test --test upstream_dev -- --ignored
//! ```
//!
//! 各テストは「上流がこう振る舞うようになれば通る」という形で書く。
//! 配布版や無変更のソースビルドでは**失敗するのが正しい**（まだ変更が
//! 入っていない）。clone に変更を当ててビルドし直し、通ったら上流に出す。
//! CI では回さない（v0.1-design.md 6 章）。
//!
//! 対象:
//! - pyright / typescript-language-server: `InitializeResult.serverInfo` を
//!   返す（LSP 3.15 の標準項目。ADR 0011 決定 C）。返せば lsp-det は起動ログ
//!   ではなく `serverInfo` で写像を選ぶ
//! - rust-analyzer / gopls: サーバー状態プロトコルを自ら話す（仕様 3〜7 章。
//!   vision.md 5 章の経路 2）。話せば lsp-det の上流側は恒等写像になり
//!   （仕様 8.2 の 6、8.4 の 2）、宣言と状態は上流のものがそのまま流れる

mod support;

use serde_json::{Value, json};
use support::{ConformanceClient, ServerUnderTest};

/// 上流を lsp-det を挟まずに直接起動する被験者。
fn direct(command: &str, args: &[&str], root: std::path::PathBuf) -> ServerUnderTest {
    ServerUnderTest {
        program: which(command),
        args: args.iter().map(|a| a.to_string()).collect(),
        root,
    }
}

/// lsp-det 経由で起動する被験者。
fn via_lsp_det(command: &str, args: &[&str], root: std::path::PathBuf) -> ServerUnderTest {
    let mut all = vec!["--".to_string(), command.to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: all,
        root,
    }
}

/// PATH からコマンドを探す。`ServerUnderTest.program` は絶対パスでも名前でも
/// よいが、どのビルドが使われたかを失敗時に見せるために解決しておく。
fn which(command: &str) -> std::path::PathBuf {
    let path = std::env::var_os("PATH").expect("PATH がない");
    let found = std::env::split_paths(&path)
        .flat_map(|dir| candidates(&dir, command))
        .find(|candidate| is_executable(candidate))
        .unwrap_or_else(|| panic!("{command} が PATH にない"));
    eprintln!("upstream_dev: {command} -> {}", found.display());
    found
}

/// `dir` の中で `command` として実行されうるファイル名。Windows は拡張子を
/// 補って探す (npm の起動子は `.cmd`)。
fn candidates(dir: &std::path::Path, command: &str) -> Vec<std::path::PathBuf> {
    let mut found = vec![dir.join(command)];
    if cfg!(windows) {
        found.extend(
            ["exe", "cmd", "bat"]
                .iter()
                .map(|ext| dir.join(format!("{command}.{ext}"))),
        );
    }
    found
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

fn server_info_of(server: &ServerUnderTest) -> Value {
    let mut client = ConformanceClient::start(server);
    let result = client.initialize_with_root(false, &server.root);
    client.shutdown();
    result["result"]["serverInfo"].clone()
}

// ---------------------------------------------------------------------------
// serverInfo を返さないサーバーへの変更（ADR 0011 決定 C）
// ---------------------------------------------------------------------------

/// pyright: `InitializeResult.serverInfo` を `{name: "pyright", version}` で返す。
/// basedpyright は既に返している（`{name: "basedpyright", version: "1.39.8"}`）。
#[test]
#[ignore = "上流の変更の受け入れ条件。ローカル専用。PATH に target/upstream/bin を置いて cargo test --test upstream_dev -- --ignored"]
fn pyright_names_itself_in_server_info() {
    let project = support::TempPyProject::with_cross_file_reference("upstream-dev");
    let info = server_info_of(&direct(
        "pyright-langserver",
        &["--stdio"],
        project.root.clone(),
    ));
    // 上流は productName ("Pyright") を名乗る。lsp-det は大文字小文字を区別しない。
    assert!(
        info["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case("pyright")),
        "pyright が serverInfo で名乗っていない: {info}"
    );
    assert!(
        info["version"].as_str().is_some_and(|v| !v.is_empty()),
        "版を名乗っていない: {info}"
    );

    // 名乗れば lsp-det は起動ログではなく serverInfo で写像を選ぶ。
    // 版が pyright::TESTED_VERSIONS になければ保証は宣言しない（true）。
    let mut client = ConformanceClient::start(&via_lsp_det(
        "pyright-langserver",
        &["--stdio"],
        project.root.clone(),
    ));
    let result = client.initialize_with_root(true, &project.root);
    assert!(
        !result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null(),
        "lsp-det が写像を選べていない: {result}"
    );
    client.shutdown();
}

/// typescript-language-server: `InitializeResult.serverInfo` を
/// `{name: "typescript-language-server", version}` で返す。
#[test]
#[ignore = "上流の変更の受け入れ条件。ローカル専用。PATH に target/upstream/bin を置いて cargo test --test upstream_dev -- --ignored"]
fn typescript_language_server_names_itself_in_server_info() {
    let project = support::TempTsProject::with_cross_file_reference("upstream-dev");
    let info = server_info_of(&direct(
        "typescript-language-server",
        &["--stdio"],
        project.root.clone(),
    ));
    assert_eq!(
        info["name"],
        json!("typescript-language-server"),
        "typescript-language-server が serverInfo で名乗っていない: {info}"
    );
    assert!(
        info["version"].as_str().is_some_and(|v| !v.is_empty()),
        "版を名乗っていない: {info}"
    );
}

// ---------------------------------------------------------------------------
// サーバー状態プロトコルを自ら話す変更（vision.md 5 章の経路 2）
// ---------------------------------------------------------------------------

/// 上流が自ら `serverStateProvider` を宣言し、`experimental/serverState` に答える。
/// lsp-det を挟むと上流側は恒等写像になり、宣言と状態は上流のものがそのまま
/// 流れる（仕様 8.4 の 2）。
fn assert_upstream_speaks_the_protocol(command: &str, args: &[&str], root: std::path::PathBuf) {
    // 直接: 宣言と応答がある。
    let mut upstream = ConformanceClient::start(&direct(command, args, root.clone()));
    let direct_result = upstream.initialize_with_root(true, &root);
    let declared =
        direct_result["result"]["capabilities"]["experimental"]["serverStateProvider"].clone();
    assert!(
        !declared.is_null() && declared != json!(false),
        "{command} が serverStateProvider を宣言していない: {direct_result}"
    );
    let direct_state = upstream.request("experimental/serverState", json!({}));
    assert!(
        direct_state["error"].is_null() && !direct_state["result"]["readiness"].is_null(),
        "{command} が experimental/serverState に答えない: {direct_state}"
    );
    upstream.shutdown();

    // lsp-det 経由: 宣言は上流のものと一致し、状態も上流の答えがそのまま返る。
    let mut client = ConformanceClient::start(&via_lsp_det(command, args, root.clone()));
    let result = client.initialize_with_root(true, &root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"], declared,
        "lsp-det が上流の宣言を書き換えた（仕様 8.4 の 2）: {result}"
    );
    let state = client.request("experimental/serverState", json!({}));
    assert!(
        state["error"].is_null() && !state["result"]["readiness"].is_null(),
        "lsp-det 経由で experimental/serverState が答えない: {state}"
    );
    client.shutdown();
}

/// rust-analyzer: `experimental/serverStatus` の後継として本プロトコルを話す
/// （仕様 10 章。quiescent → readiness、health はそのまま、保証を宣言）。
#[test]
#[ignore = "上流の変更の受け入れ条件。ローカル専用。PATH に target/upstream/bin を置いて cargo test --test upstream_dev -- --ignored"]
fn rust_analyzer_speaks_the_server_state_protocol() {
    let project = support::TempCargoProject::with_cross_file_reference("upstream-dev");
    assert_upstream_speaks_the_protocol("rust-analyzer", &[], project.root.clone());
}

/// gopls: "Setting up workspace" の progress に加えて本プロトコルを話す
/// （go.mod 変更後の再ロードも `indexing` として伝えられるようになる）。
#[test]
#[ignore = "上流の変更の受け入れ条件。ローカル専用。PATH に target/upstream/bin を置いて cargo test --test upstream_dev -- --ignored"]
fn gopls_speaks_the_server_state_protocol() {
    let project = support::TempGoProject::with_cross_file_reference("upstream-dev");
    assert_upstream_speaks_the_protocol("gopls", &[], project.root.clone());
}
