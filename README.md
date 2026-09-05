# lsp-det

[日本語](README.ja.md)

A reference implementation of the server state protocol, which removes the "silent lies" of language servers.

LSP gives a client no machine-readable way to learn whether the server can fully answer a request. As a result, the client accepts an empty array from an unfinished index, a successful-looking response from a broken server, or a result that ignores recent edits, all as legitimate answers. In editors, human eyes and a sense of timing compensated for this. Coding agents do not compensate. They send `textDocument/references` right after `initialize`, read the empty array as "no references", and go ahead with the rename or the deletion.

lsp-det consists of two things.

- **The server state protocol** ([docs/spec/server-state.md](docs/spec/server-state.md), Japanese; an English translation is planned): a vocabulary that describes a server's state on two axes, `health` and `readiness`, plus capabilities that guarantee coverage (answers computed over the whole workspace index) and freshness of responses in that state. The end goal is a proposal to LSP itself
- **The transparent proxy `lsp-det`** (Rust, single binary): sits between a client and a language server and provides the protocol to both sides. It maps the language server's own vocabulary onto the protocol, and on behalf of clients that do not speak the protocol, it holds cross-workspace requests until `ready`

## What changes

Measured with Claude Code driving rust-analyzer through lsp-det ([docs/research/claude-code-dogfooding.md](docs/research/claude-code-dogfooding.md)):

| Situation                                              | Without lsp-det                                                | With lsp-det                                                                     |
| ------------------------------------------------------ | -------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| `references` sent 6 ms after the `initialize` response | Receives the empty array of a mid-index server as success      | Held until `ready`, then the complete result (6 locations in 2 files)            |
| An 80-second index of a 1935-file Rust workspace       | Same                                                           | Held for 82 seconds, then the complete result. The client's timeout did not fire |
| tsserver crashes and only the language server survives | Every later `references` is an empty array reported as success | An error with the reason, as `health: error`                                     |

There is no upper bound on holding. Synthesizing "treat it as `ready` after some time" would create the very lie the protocol removes (spec chapter 6, item 6).

## The protocol in brief

```typescript
interface ServerState {
  health: "ok" | "warning" | "error";               // is it functioning
  readiness: "initializing" | "indexing" | "ready"; // is the index complete
  message?: string;                                 // for humans; never for machine decisions
}
```

| Name                                | Kind               | Content                                                                                             |
| ----------------------------------- | ------------------ | --------------------------------------------------------------------------------------------------- |
| `experimental/serverState`          | Request            | Answers the `ServerState` at the moment of the query, without waiting                               |
| `experimental/serverStateChanged`   | Notification       | Sent whenever `health` or `readiness` changes (only if the client declared the subscription)        |
| `serverStateProvider` (server cap.) | `InitializeResult` | `{coverage?: {scope, incomplete}, freshness?: {fileChanges}}`; `{}` promises the notifications only |
| `serverState` (client cap.)         | `InitializeParams` | Subscribes to notifications and declares "I read the state and decide whether to wait myself"       |

Guarantees apply when `readiness` is `ready` and `health` is not `error`, and they are declared by naming what is missing from the ideal rather than as booleans. `coverage` says which scope the cross-workspace answers (`references`, `definition`, `implementation`, `workspace/symbol`, `rename`, call hierarchy and so on) are computed over (`"workspace"` or `"openDocuments"`), so they will not grow later as indexing proceeds, and lists the methods whose results the server caps together with the cap (`incomplete`: rust-analyzer caps `workspace/symbol` at 128, gopls at 100, without saying so). `freshness` lists which kinds of `workspace/didChangeWatchedFiles` changes are incorporated when `ready` (`fileChanges`, in addition to `textDocument/didChange`): pyright and typescript-language-server incorporate `Changed` but report `ready` before a `Created` or `Deleted` file is folded in.

There is no `dead` in the protocol. A vanished process shows up as the end of the connection, and a surviving relay that returns successful-looking responses is `health: "error"`. An observer outside the server, such as a relay, reports `unknown` on an axis it cannot observe and never claims `ok` or `ready` without observation (chapter 8).

Chapter 10 of the spec maps existing vocabularies onto the protocol: rust-analyzer's `experimental/serverStatus`, gopls's `$/progress`, pyright's startup log, typescript-language-server's progress and crash log, jdtls's `language/status`, and clangd's absence of any signal.

## How the proxy works

```text
client ──[plain LSP]── downstream side ──[LSP + server state protocol]── upstream side ──[plain LSP]── language server
                        (stands in for       the boundary inside lsp-det       (maps the
                         the client)                                            server's vocabulary)
```

**The upstream side** stands in for the language server. It selects a mapping by the name the server gives in `InitializeResult.serverInfo` (or, failing that, in its startup log), maps the server's vocabulary onto `ServerState`, and adds `serverStateProvider` to the `InitializeResult`. Guarantees are declared only for versions that passed conformance tests 7.2 and 7.3. If the server speaks the protocol itself, nothing is added and everything passes through.

| Language server            | Readiness signal                                                   | Health signal                          | Versions with guarantees             |
| -------------------------- | ------------------------------------------------------------------ | -------------------------------------- | ------------------------------------ |
| rust-analyzer              | `quiescent` in `experimental/serverStatus`                         | `health` in the same notification      | 1.98.0, 2026-08-03                   |
| gopls                      | End of the `$/progress` "Setting up workspace"                     | The "Error loading workspace" progress | 0.23.0                               |
| pyright / basedpyright     | File enumeration finished in `window/logMessage` (once per folder) | None (`unknown`)                       | pyright 1.1.412, basedpyright 1.39.8 |
| typescript-language-server | `$/progress` "Initializing JS/TS language features…"               | The "[tsserver] Exited" log → `error`  | TypeScript 5.9.3                     |
| Anything else              | None (`unknown` on both axes)                                      |                                        | Not declared                         |

**The downstream side** stands in for the client. If the client declares `experimental.serverState`, the state is forwarded and nothing is held. Otherwise the recommended client behavior of spec chapter 9 is performed on its behalf.

| `health` \ `readiness`       | `initializing` / `indexing`   | `ready`          | `unknown`        |
| ---------------------------- | ----------------------------- | ---------------- | ---------------- |
| `ok` / `warning` / `unknown` | Hold cross-workspace requests | Forward          | Forward          |
| `error`                      | Fail immediately              | Fail immediately | Fail immediately |

Notifications, single-file queries (hover, completion, documentSymbol and so on), lifecycle messages, and everything from server to client pass straight through. A `$/cancelRequest` or `shutdown` received while holding answers the held requests with an error before being forwarded. No request is left without a response.

Message bodies are forwarded as the original bytes. Only the notifications a mapping needs and the `initialize` exchange are parsed.

## Usage

```text
lsp-det -- <language server command> [args...]
```

There are no flags. The mapping is chosen by what the server calls itself, and there is neither a time-based escape hatch nor a switch for holding. The launch line lives in the client's configuration.

A Claude Code plugin (`.lsp.json`):

```json
{
  "rust-analyzer-via-lsp-det": {
    "command": "lsp-det",
    "args": ["--", "rust-analyzer"],
    "extensionToLanguage": { ".rs": "rust" }
  },
  "pyright-via-lsp-det": {
    "command": "lsp-det",
    "args": ["--", "pyright-langserver", "--stdio"],
    "extensionToLanguage": { ".py": "python", ".pyi": "python" }
  }
}
```

The real file for all four servers is [dogfood/claude-plugin/.lsp.json](dogfood/claude-plugin/.lsp.json) and the procedure is [dogfood/README.md](dogfood/README.md). For Serena, put the same command in `ls_specific_settings.<language>.ls_base_cmd` of `.serena/project.yml` ([dogfood/serena/README.md](dogfood/serena/README.md)).

lsp-det logs the selected mapping and every state transition to stderr.

```text
lsp-det: upstream is "rust-analyzer" version "2026-08-03"; using its mapping, declaring {"coverage":{"scope":"workspace","incomplete":{"workspace/symbol":128}},"freshness":{"fileChanges":["Created","Changed","Deleted"]}}
lsp-det: [0.041s] server state -> {"health":"unknown","readiness":"initializing"} (previous held 0.041s)
lsp-det: [0.213s] server state -> {"health":"ok","readiness":"indexing"} (previous held 0.172s)
lsp-det: [6.712s] server state -> {"health":"ok","readiness":"ready"} (previous held 6.499s)
```

Two more things the downstream side does on behalf of clients that do not do them (ADR 0015). If the client neither declares `workspace.didChangeWatchedFiles` nor sends that notification, lsp-det lists the workspace with `git ls-files` (tracked and untracked files that are not ignored) before each cross-workspace request and reports files that were created, changed, or deleted since the last request to the server, so a language server that does not watch the disk itself (gopls, pyright) still sees edits made with shell tools. This needs `git` on the PATH and a workspace inside a git work tree; elsewhere nothing is sent. And a `didOpen` for a document that is already open is rewritten into a full-text `didChange`, which is what LSP requires and what typescript-language-server insists on. Both disappear once the client does these itself.

Linux, macOS, and Windows. If the client dies without closing the pipe, lsp-det exits and takes the language server with it, using the mechanism each OS provides: `PR_SET_PDEATHSIG` on Linux, `kqueue` process events on macOS, and a Job Object on Windows. Release binaries for each OS are attached to [GitHub Releases](https://github.com/tagawa0525/lsp-det/releases) from `v*` tags.

## Build and test

The only dependencies are `serde`, `serde_json`, `thiserror`, and `libc`. There is no async runtime.

```bash
nix develop            # pins rustc, rust-analyzer, gopls, pyright, and typescript-language-server
cargo build --release  # target/release/lsp-det
cargo test             # deterministic tests with a fake language server and a fake client
cargo test --test conformance -- --ignored   # 19 real-server tests (local only, not in CI)
```

The tests are the spec made executable. Swapping the subject applies them to real servers and real clients.

| Test                          | Spec chapter         | Subject                                                                                                                        |
| ----------------------------- | -------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `tests/conformance.rs`        | 7, 8.4               | The upstream side of lsp-det, against a fake upstream (`examples/fake_lsp_server.rs`) and four real servers                    |
| `tests/client_conformance.rs` | 9.1                  | The downstream side of lsp-det, against a conformant fake upstream and a fake upstream calling itself rust-analyzer            |
| `tests/process_lifetime.rs`   | Design 4.5           | lsp-det and its upstream exit when the client or lsp-det dies abruptly, on all three OSes in CI                                |
| `tests/upstream_dev.rs`       | Changes to upstreams | Acceptance criteria for the patches on the upstream forks ([scripts/upstream/README.md](scripts/upstream/README.md), Japanese) |

## Working with upstreams

The proxy is a temporary home. Once a language server speaks the protocol itself, the upstream mapping becomes the identity. Once a client reads the state itself, the downstream stand-in stops. The more conformant implementations exist, the thinner lsp-det becomes.

The changes for that are prepared on forks of the upstreams and pass their acceptance criteria locally. None has been submitted upstream yet.

| Upstream                   | Change                                                                                 |
| -------------------------- | -------------------------------------------------------------------------------------- |
| pyright                    | Return `InitializeResult.serverInfo`                                                   |
| typescript-language-server | Same                                                                                   |
| rust-analyzer              | Speak the protocol as the successor of `experimental/serverStatus`                     |
| gopls                      | Speak the protocol (per-folder initial load and load failures)                         |
| Serena                     | Read `experimental.serverState` and drop its own readiness detection (about 285 lines) |

## Documents

Documents other than this README are written in Japanese.

| Document                                               | Content                                                                                                                                                          |
| ------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [docs/spec/server-state.md](docs/spec/server-state.md) | The normative text of the server state protocol. Where other documents disagree, this one is right                                                               |
| [docs/v0.1-design.md](docs/v0.1-design.md)             | The implementation scope of the proxy (upstream side, downstream side, mappings, execution model)                                                                |
| [docs/adr/README.md](docs/adr/README.md)               | Index of design decisions, listing the ones still in force and the rejected alternatives                                                                         |
| [docs/vision.md](docs/vision.md)                       | Long-term vision (declaration ranges and launch manifests are frozen)                                                                                            |
| [docs/research/](docs/research/)                       | 22 investigations and measurements: how each language server signals readiness, prior proxies, and the LSP integrations of Serena, Claude Code, Zed, and VS Code |

## Status

v0.1 (rust-analyzer and gopls) and v0.2 (pyright, typescript-language-server, Serena integration) are complete. Next are the upstream submissions and an English translation of the spec.
