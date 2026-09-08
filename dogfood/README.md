# Dogfooding (using lsp-det from Claude Code)

A local plugin that makes Claude Code launch rust-analyzer, gopls, pyright, typescript-language-server, and nixd through lsp-det (with `pyright-langserver --stdio` and `typescript-language-server --stdio` as upstreams, the same commands the official `pyright-lsp` and `typescript-lsp` plugins use; `nixd` for `.nix`). Running this way is not a success criterion. It is a means of observation that catches real-server behavior the conformance tests miss ([docs/v0.1-design.md](../docs/v0.1-design.md), chapter 1; Japanese).

## Steps

1. Put the `lsp-det` of the working tree on the PATH (the `command` in `.lsp.json` expects a binary on the PATH). Do not install it globally. The `.envrc` at the repository root (direnv) loads the `servers` shell of `flake.nix` (the build tools plus every language server) and adds `target/release` to the PATH

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
   - Starting with `claude --debug` shows the language servers' stderr. This is also where a hold is diagnosed: lsp-det writes one line when it holds a cross-workspace request (`holding textDocument/references (id 3) while {...}; 1 held`) and one when the request leaves the queue (`released ... after 6.712s: ready`, or `rejected` / `cancelled` / `answered with an error` with the reason). A request that stays held shows a mapping that missed the server's signal; without `--debug` it is visible only as the request not returning. lsp-det writes `lsp-det: upstream is "rust-analyzer" version ...; using its mapping, declaring {...}` (pyright and typescript-language-server return no `serverInfo`, so the line is `upstream introduced itself in its startup log as "pyright" version ...` or `... "typescript-language-server" version <TypeScript version>`) and every state transition `lsp-det: [0.000s] server state -> {...}` to stderr

## Daily use (loading the plugin on every launch)

Claude Code loads a plugin placed under `~/.claude/skills/` on every launch (as `<directory name>@skills-dir`), so a symlink from there to `dogfood/claude-plugin` makes lsp-det the LSP path for Rust, Python, TypeScript, Go, and Nix without `--plugin-dir` (ADR 0021 decision F). Three things must hold in the shell that starts Claude Code:

- `lsp-det` is on the PATH (for example `target/release` of this checkout on the login PATH). It is still the working tree's build, not an installed binary
- The upstream commands are on the PATH: `rust-analyzer` usually comes from the project's own dev shell (direnv), `nixd` and `pyright-langserver` and `typescript-language-server` must be installed by the user (the official plugins expect them on the PATH too)
- The official `rust-analyzer-lsp`, `pyright-lsp`, and `typescript-lsp` plugins are disabled. When two plugins claim an extension the one registered first wins, and the order between the skills directory and the marketplace is not documented

On NixOS the author does this from home-manager (an out-of-store symlink into `~/.claude/skills`, `home.sessionPath`, and `enabledPlugins`; the configuration lives outside this repository).

## What to observe ([docs/v0.1-design.md](../docs/v0.1-design.md), chapter 8)

- When Claude Code starts the server, and when it sends the first cross-workspace request (references, definition, and so on)
- Claude Code's request timeout, and how it shows `RequestFailed` and `RequestCancelled`
- How Claude Code handles notifications it does not know (`$/progress`, `experimental/serverStatus`)

Record the observed facts in [docs/research/claude-code-dogfooding.md](../docs/research/claude-code-dogfooding.md) (Japanese). The mapping selection log (lsp-det's stderr) is kept under `~/.claude/debug/` only when started with `claude --debug`.
