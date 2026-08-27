# reference/ — 先行事例の参照リポジトリ

lsp-det の設計・実装で参照する外部リポジトリの置き場。すべて `git clone --depth 1` の浅い clone で、このディレクトリ配下は git 管理しない（`.gitignore` 参照）。再取得は下表の URL から行う。「取得時点」列は 2026-08-28 に clone した際の HEAD。

## プロキシ・ミドルウェアの先行実装

| ディレクトリ | URL | 取得時点 | 参照目的 |
| --- | --- | --- | --- |
| ra-multiplex | https://github.com/pr2502/ra-multiplex | d01f84d | 多重化プロキシ (Rust)。ID 書き換え・shutdown 横取り・handshake 中の通知問題 (#89)・cancel 未対応の帰結。移転先: https://codeberg.org/p2502/lspmux |
| lsp-devtools | https://github.com/swyddfa/lsp-devtools | 589b552 | 検査プロキシ。プロセス残留 (#132)・exit 伝播 (#191)・stderr フラッシュ (#248) の実例 |
| lsp-ws-proxy | https://github.com/qualified/lsp-ws-proxy | 452a29d | WebSocket⇔stdio 変換。URI remap という中身書き換えの例 |
| lsp-proxy | https://github.com/techee/lsp-proxy | 9b5a2a5 | 1 言語複数サーバの集約。capability マージ・initialize 同期保留の例 |
| lspx | https://github.com/thefrontside/lspx | 1b9649f | capability ベースのルーティングと応答マージ |
| ls_proxy | https://github.com/axelson/ls_proxy | 2435d95 | 観測プロキシ。Content-Length 分割でパースが壊れた実例 (README) |
| emacs-lsp-booster | https://github.com/blahgeek/emacs-lsp-booster | 004bb50 | stdio ラッパー型プロキシ (Rust)。エンコーディング書き換え型 |

## 言語サーバー（readiness 挙動の一次ソース）

| ディレクトリ | URL | 取得時点 | 参照目的 |
| --- | --- | --- | --- |
| rust-analyzer | https://github.com/rust-lang/rust-analyzer | 70d74f4 | `experimental/serverStatus` / `quiescent` の実装 (reload.rs, main_loop.rs, lsp/capabilities.rs)。統合テストの quiescent 待ち (tests/slow-tests/support.rs) |
| golang-tools | https://github.com/golang/tools | e2ef89b | gopls の "Setting up workspace" progress (gopls/internal/server/general.go, internal/progress/progress.go) |
| pyright | https://github.com/microsoft/pyright | 6a7d491 | 機械可読な準備完了通知を持たない例。ログ正規表現でしか判定できない現状の確認 |
| typescript-language-server | https://github.com/typescript-language-server/typescript-language-server | 19fce01 | `$/progress` が完了とクラッシュを区別できない例 |
| eclipse.jdt.ls | https://github.com/eclipse-jdtls/eclipse.jdt.ls | f118000 | 独自 `language/status` (ServiceReady / ProjectStatus)。rust-analyzer 以外で唯一の機械可読 readiness |

clangd (llvm/llvm-project) は monorepo が巨大なため clone しない。必要時は Web で参照する。

## クライアント・ブリッジ（統合先と事実上の標準）

| ディレクトリ | URL | 取得時点 | 参照目的 |
| --- | --- | --- | --- |
| serena | https://github.com/oraios/serena | 7fcbca7 | solidlsp の言語別補正の実測記録。`ls_specific_settings` / `ls_base_cmd` (dependency_provider.py)、gopls ハードコード (language_servers/gopls.py)、待ち時間フック |
| nvim-lspconfig | https://github.com/neovim/nvim-lspconfig | 3928e63 | 起動宣言の事実上の標準スキーマ (cmd / root_markers / settings) |
| mason-registry | https://github.com/mason-org/mason-registry | c1cf013 | 入手方法の事実上の標準 (package.yaml, purl 形式) |
| helix | https://github.com/helix-editor/helix | 079a789 | languages.toml の起動宣言。自動インストールなしの割り切り |
| zed | https://github.com/zed-industries/zed | 5218009 | LspAdapter / LspInstaller の分離設計 (Rust)。rust-analyzer の serverStatus の扱い |
| claude-code-lsps-piebald | https://github.com/Piebald-AI/claude-code-lsps | 92afe66 | CC の `.lsp.json` プラグインの実例集 |
| claude-code-lsps-boostvolt | https://github.com/boostvolt/claude-code-lsps | 0395648 | 同上（別実装） |
| cclsp | https://github.com/ktnyt/cclsp | 93414a1 | LSP→MCP 薄ブリッジ。範囲・準備完了の扱いの有無を確認 |
| mcpls | https://github.com/bug-ops/mcpls | 5a6b7c4 | 同上 |
| codex-lsp | https://github.com/code-yeongyu/codex-lsp | 5583664 | Codex 向けブリッジ。編集後フックで診断を返す設計 |

## 仕様・プロトコル

| ディレクトリ | URL | 取得時点 | 参照目的 |
| --- | --- | --- | --- |
| language-server-protocol | https://github.com/microsoft/language-server-protocol | 8be2e19 | LSP 本体仕様。issue #511 (readiness)・#54・DocumentSymbol.range の仕様文。Base Protocol |
| LSAP | https://github.com/lsp-client/LSAP | 0c7bcc2 | 上位レイヤー（合成クエリ）。内部の範囲・準備完了補正の有無を確認 |
| lsai-protocol | https://github.com/LadislavSopko/lsai-protocol | e5ae849 | フォールバック戦略の分類 (spec/LSAI-v1.4.md)。CC BY-NC のため設計参考のみ |
