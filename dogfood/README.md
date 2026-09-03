# ドッグフーディング（Claude Code で lsp-det を使う）

Claude Code に、rust-analyzer・gopls・pyright・typescript-language-server を lsp-det 経由で起動させるためのローカルプラグイン（公式の `pyright-lsp` / `typescript-lsp` と同じく `pyright-langserver --stdio` / `typescript-language-server --stdio` を上流にする）。稼働そのものは成功基準ではなく、準拠テストが見落とす実サーバーの挙動を拾う観測手段である（v0.1-design.md 1 章）。

## 手順

1. 作業ツリーの `lsp-det` を PATH に置く（`.lsp.json` の `command` は PATH 上のバイナリを前提とする）。グローバルにはインストールしない。リポジトリ直下の `.envrc`（direnv）が開発環境（`flake.nix`）を読み込み、`target/release` を PATH に足す

   ```bash
   direnv allow
   cargo build --release
   which lsp-det   # → target/release/lsp-det
   ```

   Claude Code はこのディレクトリで起動すると PATH を継承するので、最新のビルドが使われる。ソースを変えたら `cargo build --release` し直す

2. このプラグインを読み込んで Claude Code を起動する

   ```bash
   claude --plugin-dir dogfood/claude-plugin
   ```

   同じ拡張子を複数のプラグインが宣言したときは先に登録された定義が使われる。`--plugin-dir` のプラグインは公式マーケットプレイスのものより先に登録されるので、公式の `rust-analyzer-lsp` / `pyright-lsp` / `typescript-lsp` が有効なままでも lsp-det 側が使われる。確実にしたいときは `/plugin` で公式のものを無効にする

3. 動いていることを確かめる

   - `/plugin` の Errors タブに起動失敗が出ていないこと（`Executable not found in $PATH` 等）
   - `claude --debug` で起動すると LSP サーバーの stderr が見える。lsp-det は `lsp-det: upstream is "rust-analyzer" version ...; using its mapping, declaring {...}`（pyright と typescript-language-server は `serverInfo` を返さないので `upstream introduced itself in its startup log as "pyright" version ...` / `... "typescript-language-server" version <TypeScript の版>`） と、状態遷移 `lsp-det: [0.000s] server state -> {...}` を stderr に出す

## 観測項目（v0.1-design.md 8 章）

- Claude Code がサーバーをいつ起動し、いつ最初の横断リクエスト（references / definition 等）を投げるか
- Claude Code のリクエストタイムアウトと、`RequestFailed` / `RequestCancelled` の見せ方
- Claude Code が未知の通知（`$/progress`、`experimental/serverStatus`）をどう扱うか

観測した事実は `docs/research/claude-code-dogfooding.md` に追記する。写像の選択ログ（lsp-det の stderr）は `claude --debug` で起動したときだけ `~/.claude/debug/` に残る。
