# Verifying upstream changes locally

[日本語](README.ja.md)

lsp-det's next phase is engaging upstream (ADR 0009 decision A-3, ADR 0010 decision A-4, ADR 0011 decision C). Before submitting, apply the change to a clone of the upstream, build it, and run lsp-det's conformance tests and acceptance-condition tests against it.

| Upstream                                  | Change                                                                                                                                                                             | Acceptance condition (`tests/upstream_dev.rs`)                                                           |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| pyright                                   | Return `InitializeResult.serverInfo` (ADR 0011 decision C)                                                                                                                         | `pyright_names_itself_in_server_info`                                                                    |
| typescript-language-server                | Same                                                                                                                                                                               | `typescript_language_server_names_itself_in_server_info`                                                 |
| typescript-language-server                | Exit when tsserver is killed by a signal, as it already does for a non-zero exit code (upstream #302 / #305)                                                                       | `typescript_language_server_exits_when_tsserver_is_killed`                                               |
| rust-analyzer                             | Speak the server state protocol itself (spec chapters 3-7, the mapping table in chapter 10)                                                                                        | `rust_analyzer_speaks_the_server_state_protocol`                                                         |
| rust-analyzer                             | (the alternative) Add a `readiness` field to `experimental/serverStatus`. The mapping reads the field when present                                                                 | `rust_analyzer_reports_readiness_in_server_status`                                                       |
| gopls                                     | Same                                                                                                                                                                               | `gopls_speaks_the_server_state_protocol`                                                                 |
| Serena                                    | Raise on a tsserver crash for every cross-file query, not only the first ((a) of the M7 observation). Listing lsp-det in the registry (oraios/serena#1988) is proposed in an issue | `scripts/serena/probe.py` with `CRASH=1 VIA_LSP_DET=0` exits 0 (below)                                   |
| LSP itself (`vscode-languageserver-node`) | `proposed.serverState.ts` and the `.md` of the same name (`workspace/serverState`, `workspace/serverStateChanged`, `serverStateProvider`; listed in the meta model as proposed)    | none. `npm run compile:protocol` and `lint` / `test:node` / `generate:metaModel` in `protocol` must pass |

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

| Upstream                   | Branch                                                                                                                                                                       | Content                                                                                                                                                                                                                                                                                                                                                                                                                                                    | Acceptance condition                                                                                                                                                                                               |
| -------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| pyright                    | `server-info` on `tagawa0525/pyright`                                                                                                                                        | `serverInfo: {name: productName, version}` in `languageServerBase.ts`'s `initialize()`                                                                                                                                                                                                                                                                                                                                                                     | passes                                                                                                                                                                                                             |
| typescript-language-server | `server-info` on `tagawa0525/typescript-language-server`                                                                                                                     | Centralizes version reading in `src/version.ts`, adds `serverInfo: {name, version}` to the `initialize` result                                                                                                                                                                                                                                                                                                                                             | passes (lint and build also pass)                                                                                                                                                                                  |
| typescript-language-server | `tsserver-exit-by-signal` on `tagawa0525/typescript-language-server` (based on upstream master; independent of `server-info`)                                                | Removes the `if (exitCode)` guard from the `onExit` handler in `lsp-server.ts`, so the server stops for a tsserver killed by a signal (`exitCode: null`) too. Since #585 the client disposes its exit handlers before killing tsserver, so its own shutdown never reaches `onExit`. Two cases in `ts-client.test.ts`. Replaces the rejected `no-server-request-failed` (`RequestFailed` path; `docs/upstream-submissions.md`)                              | Passes (`typescript_language_server_exits_when_tsserver_is_killed`; the stock 6.0.0 survives and answers `[]`). vitest 141 cases, lint and typecheck pass; the fork's CI (3 OS × Node 22 / 24) on the fork's PR #1 |
| rust-analyzer              | `server-state` on `tagawa0525/rust-analyzer`                                                                                                                                 | The full `experimental/serverState` set (`lsp/ext.rs`, `current_server_state` in `reload.rs`, the notification in `main_loop.rs`, the capability, `lsp-extensions.md` and its hash). An undiscovered workspace is `error`                                                                                                                                                                                                                                  | passes (`cargo xtask tidy`, 99 lib tests also pass). 7.1 / 7.2 / 7.3 also pass under the identity mapping                                                                                                          |
| rust-analyzer              | `server-status-readiness` on `tagawa0525/rust-analyzer` (same base as `server-state`; the issue lists both, and only the one the maintainers pick is sent as a PR)           | `readiness` (`initializing` / `indexing` / `ready`, `current_readiness` in `reload.rs`) in `ServerStatusParams`, the initial `last_reported_status` at `initializing` (the number of notifications does not change), the field and the note "this field, unlike the others, is meant for clients that decide whether to trust an answer" in `lsp-extensions.md` with its hash, the type in `editors/code`                                                  | passes (`cargo xtask tidy`, 99 lib tests also pass). The mapping reads the field; 7.1 / 7.2 / 7.3 also pass. Measured with `scripts/rust-analyzer/status-probe.py`                                                 |
| LSP itself                 | `server-state` on `tagawa0525/vscode-languageserver-node`                                                                                                                    | `protocol/src/common/proposed.serverState.ts` (types, request, notification, `$ServerStateClientCapabilities` / `$ServerStateServerCapabilities`), `proposed.serverState.md` (the specification text), `Proposed` in `api.ts`, the regenerated `metaModel.json`. Dependencies: `npm ci --ignore-scripts && node ./build/bin/all.js install && npm run symlink` at the root (a plain `npm ci` at the root installs playwright in its postinstall; avoid it) | passes (`compile:protocol`, `lint`, 5 `test:node` tests; `generate:metaModel` is an additive diff)                                                                                                                 |
| Serena                     | `tsserver-crash-on-request-path` on `tagawa0525/serena` (based on upstream main; it touches the same method as oraios/serena#1978, so it is rebased once that PR is decided) | Call `_raise_if_crashed()` at the top of `_wait_for_cross_file_references_if_needed`, before the latch. One test for the latched-after-crash case and a CHANGELOG entry                                                                                                                                                                                                                                                                                    | Passes (the probe exits 0; the stock HEAD exits 1; `test_typescript_timeout_policy.py` 30 tests, `test/solidlsp/typescript` 20 tests, ruff and ty pass; the same on top of #1978)                                  |
| gopls                      | `server-state` on `tagawa0525/tools`                                                                                                                                         | `server_state.go` (`indexing` -> `ready` on each folder's initial load, `error` on load failure and "Error loading workspace"), a `ServerStateProvider` hook in `protocol.go` (delivers `experimental/serverState` outside the generated code) and the notification                                                                                                                                                                                        | passes (`go test ./internal/server ./internal/protocol` also pass). 7.2 / 7.3 also pass under the identity mapping                                                                                                 |

With an upstream that speaks this protocol itself (the patched rust-analyzer / gopls), lsp-det becomes the identity mapping and passes the upstream's notifications straight through. The following conformance test assertions are premised on the mapping acting as an observer, so failing them is correct on the patched build:

- `gopls_spec_7_1_through_lsp_det_with_real_gopls`'s "not `ready` right after `initialize`": on a small fixture the upstream honestly answers `ready` (spec 7.1 item 1 is relaxed by ADR 0009 decision C-5)
- `gopls_does_not_reemit_workspace_setup_on_go_mod_change`: the upstream's initial-load notifications (`indexing` -> `ready`) are still pending in the receive queue after `wait_until_ready` and get picked up there. The upstream is not reissuing them on the go.mod change (gopls reloads synchronously within the request after a go.mod change, so staying `ready` is correct)

With the `tsserver-exit-by-signal` build of typescript-language-server, two conformance assertions fail, and correctly so: `typescript_language_server_tsserver_crash_becomes_health_error_with_real_server` (the language server now exits with tsserver, so lsp-det exits too and the test's write fails with a broken pipe: the upstream is gone, spec chapter 8, rather than lying) and `typescript_language_server_spec_7_1_through_lsp_det_with_real_server` (the source build bundles a TypeScript that is not in `TESTED_VERSIONS`, so no guarantee is declared).

On lsp-det's side, name matching is case-insensitive (pyright calls itself "Pyright"), and whether to replace the basis for a guarantee with `serverInfo`'s version is up to the mapping (typescript-language-server's `serverInfo` version is the wrapper's version). Applying the patch to rust-analyzer also surfaced and fixed a bug where the identity mapping's initial state query happened before `initialized` (PR #26)

The clones that receive changes for an upstream (the six in the "Forks and remotes" table below) have the fork as `origin` and the upstream as `upstream`, and the global git hooks skip this repository's convention checks (the Conventional Commits subject, markdownlint auto-fix, ruff, rustfmt) in any clone with an `upstream` remote. The upstream's conventions are checked by its own pinned tools and CI. The other clones under `reference/` (the shallow clones in `reference/README.md`) have only `origin` and are never committed to

## Serena

The change on Serena's side is applied to `reference/serena` (install the dev dependencies with `uv sync --frozen --all-groups --all-extras`) and confirmed with `scripts/serena/probe.py`, which fetches references. The acceptance condition for the fork's `tsserver-crash-on-request-path` is that `CRASH=1 VIA_LSP_DET=0` exits 0 (the references request right after killing tsserver raises `TypeScriptServerCrashedError`, and the probe prints the "references after crash raised TypeScriptServerCrashedError" line). The stock upstream returns 0 locations as a success; the probe prints "NO ERROR SURFACED" and exits 1. The same tool compares the path through lsp-det:

```bash
# acceptance condition (exits 0; line 0 column 16 of a.ts is the exported function's name)
cd reference/serena && CRASH=1 VIA_LSP_DET=0 uv run --frozen python ../../scripts/serena/probe.py typescript /path/to/repo a.ts 0 16
# comparison through lsp-det
cd reference/serena && uv run --frozen python ../../scripts/serena/probe.py python /path/to/repo a.py 0 4
```

`VIA_LSP_DET=0` compares against no lsp-det, and `CRASH=1` compares the view right after killing tsserver. The observation record is in `docs/research/serena-integration-measurement.md` (Japanese).

## Forks and remotes

The 6 repositories submitted upstream each have a public fork under tagawa0525, and the clones under `reference/` have `origin` pointing at the fork and `upstream` at the original repository (as of 2026-09-03; `vscode-languageserver-node` since 2026-09-09). Changes are pushed to a branch on the fork and a PR is opened against upstream from there.

| clone                                  | origin (fork)                                      | upstream                                                           |
| -------------------------------------- | -------------------------------------------------- | ------------------------------------------------------------------ |
| `reference/pyright`                    | `github.com/tagawa0525/pyright`                    | `github.com/microsoft/pyright`                                     |
| `reference/typescript-language-server` | `github.com/tagawa0525/typescript-language-server` | `github.com/typescript-language-server/typescript-language-server` |
| `reference/rust-analyzer`              | `github.com/tagawa0525/rust-analyzer`              | `github.com/rust-lang/rust-analyzer`                               |
| `reference/golang-tools`               | `github.com/tagawa0525/tools`                      | `github.com/golang/tools`                                          |
| `reference/serena`                     | `github.com/tagawa0525/serena`                     | `github.com/oraios/serena`                                         |
| `reference/vscode-languageserver-node` | `github.com/tagawa0525/vscode-languageserver-node` | `github.com/microsoft/vscode-languageserver-node`                  |

The clones are shallow (`--depth 1`). That is enough to cut a branch and push it, but run `git fetch --unshallow upstream` when history is needed.

## Notes

- `reference/` holds shallow clones not tracked by git. Changes there are a workspace until they go upstream; they do not enter lsp-det's own repository
- Upstream's own versions are not added to `TESTED_VERSIONS`. Only the distributed versions pinned by flake.nix, once they pass 7.2 / 7.3, are added
