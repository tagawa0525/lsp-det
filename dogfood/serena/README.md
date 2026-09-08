# Route Serena through lsp-det

[日本語](README.ja.md)

Serena (solidlsp) lets a project's settings replace the launch command for each language server. To route Python and TypeScript through lsp-det, add the following to the target project's `.serena/project.yml`. No code change on Serena's side is needed.

```yaml
language_servers: ["python", "typescript"]
ls_specific_settings:
  python:
    ls_base_cmd: ["lsp-det", "--", "pyright-langserver", "--stdio"]
  typescript:
    ls_base_cmd: ["lsp-det", "--", "typescript-language-server", "--stdio"]
```

- The keys under `ls_specific_settings` are Serena's `LanguageServerId` values (`python` / `typescript`)
- `lsp-det` and the upstream command are resolved on Serena's process PATH. In this repository, direnv puts `target/release` on the PATH (run `cargo build --release` first). pyright and typescript-language-server come from flake.nix
- Go is out of scope because Serena cannot replace its launch command (design chapter 9)

The observed facts are in [docs/research/serena-integration-measurement.md](../../docs/research/serena-integration-measurement.md) (Japanese). Serena's own readiness wait (the regex on "Found N source files", the `$/progress` token) still holds as-is, since lsp-det forwards logs and progress verbatim, and the downstream side's hold layers on top of it. When tsserver crashes, Serena alone returns references as a successful empty array, but through lsp-det it becomes an error with a reason.
