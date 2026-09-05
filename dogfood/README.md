# Dogfooding (using lsp-det from Claude Code)

A local plugin that makes Claude Code launch rust-analyzer, gopls, pyright, and typescript-language-server through lsp-det (with `pyright-langserver --stdio` and `typescript-language-server --stdio` as upstreams, the same commands the official `pyright-lsp` and `typescript-lsp` plugins use). Running this way is not a success criterion. It is a means of observation that catches real-server behavior the conformance tests miss (v0.1-design.md, chapter 1; Japanese).

## Steps

1. Put the `lsp-det` of the working tree on the PATH (the `command` in `.lsp.json` expects a binary on the PATH). Do not install it globally. The `.envrc` at the repository root (direnv) loads the development environment (`flake.nix`) and adds `target/release` to the PATH

   ```bash
   direnv allow
   cargo build --release
   which lsp-det   # → target/release/lsp-det
   ```

   Claude Code inherits the PATH when started in this directory, so the latest build is used. Rebuild with `cargo build --release` after changing the source

2. Start Claude Code with this plugin loaded

   ```bash
   claude --plugin-dir dogfood/claude-plugin
   ```

   When several plugins declare the same file extension, the definition registered first wins. Plugins from `--plugin-dir` are registered before those from the official marketplace, so lsp-det is used even while the official `rust-analyzer-lsp`, `pyright-lsp`, and `typescript-lsp` remain enabled. To be certain, disable the official ones in `/plugin`

3. Check that it works

   - The Errors tab of `/plugin` shows no launch failure (`Executable not found in $PATH` and the like)
   - Starting with `claude --debug` shows the language servers' stderr. lsp-det writes `lsp-det: upstream is "rust-analyzer" version ...; using its mapping, declaring {...}` (pyright and typescript-language-server return no `serverInfo`, so the line is `upstream introduced itself in its startup log as "pyright" version ...` or `... "typescript-language-server" version <TypeScript version>`) and every state transition `lsp-det: [0.000s] server state -> {...}` to stderr

## What to observe (v0.1-design.md, chapter 8)

- When Claude Code starts the server, and when it sends the first cross-workspace request (references, definition, and so on)
- Claude Code's request timeout, and how it shows `RequestFailed` and `RequestCancelled`
- How Claude Code handles notifications it does not know (`$/progress`, `experimental/serverStatus`)

Record the observed facts in `docs/research/claude-code-dogfooding.md` (Japanese). The mapping selection log (lsp-det's stderr) is kept under `~/.claude/debug/` only when started with `claude --debug`.
