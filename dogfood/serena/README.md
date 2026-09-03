# Serena を lsp-det 経由にする

Serena（solidlsp）は言語サーバーの起動コマンドをプロジェクト設定で差し替えられる。Python と TypeScript を lsp-det 経由にするには、対象プロジェクトの `.serena/project.yml` に次を足す。Serena 側のコード変更は要らない。

```yaml
language_servers: ["python", "typescript"]
ls_specific_settings:
  python:
    ls_base_cmd: ["lsp-det", "--", "pyright-langserver", "--stdio"]
  typescript:
    ls_base_cmd: ["lsp-det", "--", "typescript-language-server", "--stdio"]
```

- `ls_specific_settings` のキーは Serena の `LanguageServerId` の値（`python` / `typescript`）
- `lsp-det` と上流のコマンドは Serena のプロセスの PATH で解決される。本リポジトリでは direnv が `target/release` を PATH に置く（先に `cargo build --release`）。pyright と typescript-language-server は flake.nix のもの
- Go は Serena が起動コマンドを差し替えられないので対象外（設計 9 章）

観測した事実は [docs/research/serena-integration-measurement.md](../../docs/research/serena-integration-measurement.md)。Serena の readiness 待ち（"Found N source files" の正規表現、`$/progress` のトークン）は lsp-det がログと progress を原文のまま流すのでそのまま成立し、lsp-det の下流側の保留はその上に重なる。tsserver が落ちたときは、Serena 単体では references が空配列を成功として返すが、lsp-det 経由では理由付きのエラーになる。
