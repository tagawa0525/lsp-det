# Verifying upstream changes locally

[日本語](README.ja.md)

lsp-det's next phase is engaging upstream (ADR 0009 decision A-3, ADR 0010 decision A-4, ADR 0011 decision C). Before submitting, apply the change to a clone of the upstream, build it, and run lsp-det's conformance tests and acceptance-condition tests against it.

| Upstream                   | Change                                                                                      | Acceptance condition (`tests/upstream_dev.rs`)           |
| -------------------------- | ------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| pyright                    | Return `InitializeResult.serverInfo` (ADR 0011 decision C)                                  | `pyright_names_itself_in_server_info`                    |
| typescript-language-server | Same                                                                                        | `typescript_language_server_names_itself_in_server_info` |
| rust-analyzer              | Speak the server state protocol itself (spec chapters 3-7, the mapping table in chapter 10) | `rust_analyzer_speaks_the_server_state_protocol`         |
| gopls                      | Same                                                                                        | `gopls_speaks_the_server_state_protocol`                 |
| Serena                     | Declare `experimental.serverState` and drop its own readiness judgment (observed in M7)     | `scripts/serena/probe.py` (Python; below)                |

## Procedure

1. Apply the change under `reference/<repo>` (the clone is shallow; prepare a separate fork before submitting upstream)
2. Build it. Inside `nix develop .#servers` (or direnv), where pnpm and node live:

   ```bash
   scripts/upstream/build-pyright.sh
   scripts/upstream/build-typescript-language-server.sh
   scripts/upstream/build-rust-analyzer.sh   # uses rustup's stable (upstream's rust-version is newer than the flake's rustc)
   scripts/upstream/build-gopls.sh
   ```

   The launchers land in `target/upstream/bin/` (not tracked by `git`).
3. Put it at the front of the PATH and run the acceptance conditions:

   ```bash
   PATH="$PWD/target/upstream/bin:$PATH" cargo test --test upstream_dev -- --ignored
   ```

   **Failing is correct before the change is applied.** Once it passes, submit upstream.
4. Run the existing conformance tests with the same PATH and check for regressions:

   ```bash
   PATH="$PWD/target/upstream/bin:$PATH" cargo test --test conformance -- --ignored
   ```

   A source build names a version different from the distributed one (rust-analyzer as `0.0.0 (<sha> <date>)`, gopls as `v0.0.0-<date>-<sha>`, pyright as the clone's own version). Since it is not in `TESTED_VERSIONS`, no guarantee is declared, and the test that "declares a guarantee for a measured version" fails. This is expected; what matters is that the rest (7.1 / 7.2 / 7.3, rejection, reissue) passes. Also, on a build with the change that adds `serverInfo` applied, the assertion that records the current upstream's state ("the premise no longer holds. ... now returns serverInfo") fails. This too is a sign the change is working; once it lands upstream, rewrite the conformance test's premise accordingly

## Changes already prepared (branches on the fork)

| Upstream                   | Branch                                                                                                                         | Content                                                                                                                                                                                                                                                             | Acceptance condition                                                                                                                               |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| pyright                    | `server-info` on `tagawa0525/pyright`                                                                                          | `serverInfo: {name: productName, version}` in `languageServerBase.ts`'s `initialize()`                                                                                                                                                                              | passes                                                                                                                                             |
| typescript-language-server | `server-info` on `tagawa0525/typescript-language-server`                                                                       | Centralizes version reading in `src/version.ts`, adds `serverInfo: {name, version}` to the `initialize` result                                                                                                                                                      | passes (lint and build also pass)                                                                                                                  |
| typescript-language-server | `no-server-request-failed` on `tagawa0525/typescript-language-server` (based on upstream master; independent of `server-info`) | `TsClient.execute` / `executeCustom` reject with `RequestFailed` (-32803, message carries the exit reason) instead of the `NoServer` empty response when tsserver is not running. 2 cases added to `ts-client.test.ts`                                              | passes (`typescript_language_server_fails_requests_after_tsserver_exit`; plain 5.3.0 fails with `result: []`). 141 vitest cases and lint also pass |
| rust-analyzer              | `server-state` on `tagawa0525/rust-analyzer`                                                                                   | The full `experimental/serverState` set (`lsp/ext.rs`, `current_server_state` in `reload.rs`, the notification in `main_loop.rs`, the capability, `lsp-extensions.md` and its hash). An undiscovered workspace is `error`                                           | passes (`cargo xtask tidy`, 99 lib tests also pass). 7.1 / 7.2 / 7.3 also pass under the identity mapping                                          |
| gopls                      | `server-state` on `tagawa0525/tools`                                                                                           | `server_state.go` (`indexing` -> `ready` on each folder's initial load, `error` on load failure and "Error loading workspace"), a `ServerStateProvider` hook in `protocol.go` (delivers `experimental/serverState` outside the generated code) and the notification | passes (`go test ./internal/server ./internal/protocol` also pass). 7.2 / 7.3 also pass under the identity mapping                                 |

With an upstream that speaks this protocol itself (the patched rust-analyzer / gopls), lsp-det becomes the identity mapping and passes the upstream's notifications straight through. The following conformance test assertions are premised on the mapping acting as an observer, so failing them is correct on the patched build:

- `gopls_spec_7_1_through_lsp_det_with_real_gopls`'s "not `ready` right after `initialize`": on a small fixture the upstream honestly answers `ready` (spec 7.1 item 1 is relaxed by ADR 0009 decision C-5)
- `gopls_does_not_reemit_workspace_setup_on_go_mod_change`: the upstream's initial-load notifications (`indexing` -> `ready`) are still pending in the receive queue after `wait_until_ready` and get picked up there. The upstream is not reissuing them on the go.mod change (gopls reloads synchronously within the request after a go.mod change, so staying `ready` is correct)

On lsp-det's side, name matching is case-insensitive (pyright calls itself "Pyright"), and whether to replace the basis for a guarantee with `serverInfo`'s version is up to the mapping (typescript-language-server's `serverInfo` version is the wrapper's version). Applying the patch to rust-analyzer also surfaced and fixed a bug where the identity mapping's initial state query happened before `initialized` (PR #26)

Commit under `reference/` with `--no-verify`. This repository's commit hook (the Conventional Commits subject, markdownlint auto-fix) would otherwise rewrite the upstream's own documents

## Serena

The change on Serena's side (declare `experimental.serverState` and read `experimental/serverState` and `serverStateChanged`) is applied to `reference/serena`, and confirmed with `scripts/serena/probe.py`, which fetches references through lsp-det:

```bash
cd reference/serena && uv run --frozen python ../../scripts/serena/probe.py python /path/to/repo a.py 0 4
```

`VIA_LSP_DET=0` compares against no lsp-det, and `CRASH=1` compares the view right after killing tsserver. The observation record is in `docs/research/serena-integration-measurement.md` (Japanese).

## Forks and remotes

The 5 repositories submitted upstream each have a public fork under tagawa0525, and the clones under `reference/` have `origin` pointing at the fork and `upstream` at the original repository (as of 2026-09-03). Changes are pushed to a branch on the fork and a PR is opened against upstream from there.

| clone                                  | origin (fork)                                      | upstream                                                           |
| -------------------------------------- | -------------------------------------------------- | ------------------------------------------------------------------ |
| `reference/pyright`                    | `github.com/tagawa0525/pyright`                    | `github.com/microsoft/pyright`                                     |
| `reference/typescript-language-server` | `github.com/tagawa0525/typescript-language-server` | `github.com/typescript-language-server/typescript-language-server` |
| `reference/rust-analyzer`              | `github.com/tagawa0525/rust-analyzer`              | `github.com/rust-lang/rust-analyzer`                               |
| `reference/golang-tools`               | `github.com/tagawa0525/tools`                      | `github.com/golang/tools`                                          |
| `reference/serena`                     | `github.com/tagawa0525/serena`                     | `github.com/oraios/serena`                                         |

The clones are shallow (`--depth 1`). That is enough to cut a branch and push it, but run `git fetch --unshallow upstream` when history is needed.

## Notes

- `reference/` holds shallow clones not tracked by git. Changes there are a workspace until they go upstream; they do not enter lsp-det's own repository
- Upstream's own versions are not added to `TESTED_VERSIONS`. Only the distributed versions pinned by flake.nix, once they pass 7.2 / 7.3, are added
