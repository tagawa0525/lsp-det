# LSP 本体（microsoft/language-server-protocol） への提出

[../upstream-submissions.md](../upstream-submissions.md) の戦略に沿って LSP 本体（microsoft/language-server-protocol） に出した文面と、提出後の反応・再測定の記録。出した文面は一字も変えずに残す。現在地（何を出し、何が返ってきたか）は同文書の一覧を見る。

## microsoft/language-server-protocol#511 へのコメントと proposal issue

第 3 段（第 2 段のどれかに反応があってから）。issue 先: `microsoft/language-server-protocol`。根拠: [research/readiness-vocabulary-corpus.ja.md](../research/readiness-vocabulary-corpus.ja.md)、仕様 10 章、fork `tagawa0525/vscode-languageserver-node` の `server-state`（`proposed.serverState.ts` と `.md`）。proposal issue を先に立て、#511 にはそれへのリンクを添えてコメントする。

microsoft/language-server-protocol#511 へのコメント:

```markdown
Coming back to this thread from a different angle, coding agents as LSP clients, with measurements.

What @matklad wrote in 2021 still holds: `serverStatus` is for humans, and `ContentModified` makes the client poll. What has changed is the client. Agents (Claude Code, Serena, Zed's agent, …) send `textDocument/references` right after starting the server and take an empty or partial answer as a fact. Measured with Claude Code: rust-analyzer answers `references` with `[]` 6 ms after `initialize`, typescript-language-server answers with the declaration only, and the agent then deletes a function that is still in use. A person sees the spinner and waits; an agent does not.

Meanwhile more than twenty servers have each invented the same facts in their own vocabulary: rust-analyzer's `experimental/serverStatus`, jdtls's `language/status`, Sorbet's `sorbet/showOperation`, the Dart analysis server's `ANALYZING` token, clangd's `backgroundIndexProgress`, … (corpus: https://github.com/tagawa0525/lsp-det/blob/main/docs/research/readiness-vocabulary-corpus.md). The facts are: is the index complete, is the server broken, does the answer include my edits.

I have written that up as a proposal, `workspace/serverState` / `workspace/serverStateChanged` with `{health, readiness}` and a `serverStateProvider` capability that says what `ready` guarantees, as `proposed.serverState.ts` plus specification text in the vscode-languageserver-node format (https://github.com/tagawa0525/vscode-languageserver-node/tree/server-state/protocol/src/common), together with a reference implementation: a transparent proxy that speaks it today on behalf of 18 servers by mapping their vocabularies (https://github.com/tagawa0525/lsp-det). Details and the discussion are in the proposal issue: #NNNN.
```

proposal issue の題名: `Proposal: workspace/serverState, a server says whether its index is complete and whether it is functional`

proposal issue の本文:

````markdown
## Summary

A language server indexes its workspace after `initialize` and, while the index is incomplete, answers workspace-wide requests (`textDocument/references`, `workspace/symbol`, `textDocument/rename`, …) with a complete-looking result that is missing entries. LSP has no vocabulary for "this result is incomplete": `$/progress` is a display for humans (a client cannot tell which requests a token affects), and `ServerCancelled` (#1367) makes the client poll. A client that is a person sees the spinner and waits. A client that is a coding agent sends the request 6 ms after `initialize` and acts on the answer.

This proposes one request, one notification and one capability:

- `ServerState { health: "ok" | "warning" | "error", readiness: "initializing" | "indexing" | "ready", message?: string }`. Two independent axes. A failure of indexing is expressed through `health`, never through `readiness`.
- `workspace/serverState` (client → server): the state at the moment the request is received, answered at once.
- `workspace/serverStateChanged` (server → client): sent on every change of `health` or `readiness`, if the client declared `workspace.serverState: true`.
- `serverStateProvider: { coverage?, freshness? }`: what `ready` guarantees, written by naming what is missing from the ideal. `coverage: { scope: "workspace" | "openDocuments" | "document", incomplete: { [method]: cap } }` says over which index results are computed and which methods are capped. `freshness: { fileChanges: ("Created" | "Changed" | "Deleted")[] }` says which `didChangeWatchedFiles` kinds are incorporated before `ready` is reported again (`didChange` always is). `{}` promises the notification only, and even that has value.

Implementation in the vscode-languageserver-node format, with the specification text: https://github.com/tagawa0525/vscode-languageserver-node/tree/server-state/protocol/src/common (`proposed.serverState.ts`, `proposed.serverState.md`; `metaModel.json` regenerated with the entries marked proposed).

## Why this is not new vocabulary

More than twenty servers already report these facts, each in its own words (corpus, with sources: https://github.com/tagawa0525/lsp-det/blob/main/docs/research/readiness-vocabulary-corpus.md; the mapping of each onto the proposal: https://github.com/tagawa0525/lsp-det/blob/main/docs/spec/server-state.md#10-mapping-from-existing-implementations):

| server                                                    | readiness                                                          | health                                                            |
| --------------------------------------------------------- | ------------------------------------------------------------------ | ----------------------------------------------------------------- |
| rust-analyzer                                             | `experimental/serverStatus` `quiescent`                            | `health` in the same notification                                 |
| gopls                                                     | `$/progress` "Setting up workspace" / "Loading packages"           | `$/progress` "Error loading workspace"                            |
| jdtls                                                     | `language/status` `ServiceReady`                                   | `language/status` `ProjectStatus`, diagnostics on the project URI |
| Sorbet                                                    | `sorbet/showOperation` (nested)                                    | none                                                              |
| Dart                                                      | `$/progress` token `ANALYZING`                                     | none                                                              |
| clangd                                                    | `$/progress` token `backgroundIndexProgress`                       | none                                                              |
| pyright, typescript-language-server, nixd, nil, Metals, … | `$/progress` with server-specific titles, `window/logMessage` text | `window/showMessage`, crash of a child process                    |

Clients read them one by one: Zed reads `experimental/serverStatus` for rust-analyzer; Serena has a per-server waiting rule for each of 30 servers, several of them wrong (they wait for a signal the server never sends). #511 asked for this in 2018 and stalled in 2021 on "servers can use `$/progress`" and "who is the consumer". Both answers have changed: `$/progress` does not say which requests are affected or whether the server is broken, and the consumers are now programs.

## What exists today

- A reference implementation: https://github.com/tagawa0525/lsp-det, a transparent proxy (Rust, no dependencies beyond serde) that speaks the protocol under the `experimental/` prefix on behalf of 18 servers by mapping their vocabularies, holds workspace-wide requests for clients that do not speak it, and declares only the guarantees it has verified per server version. Conformance tests for servers (chapter 7 of the specification) and for clients (chapter 9) run against fake servers in CI and against the real servers locally.
- Measurements of what each server answers before `ready`, of what happens when the server is broken, and of whether edits are incorporated, per server: https://github.com/tagawa0525/lsp-det/tree/main/docs/research.
- Two servers with the protocol applied on a fork (rust-analyzer, gopls), and for rust-analyzer an alternative that adds a `readiness` field to `experimental/serverStatus` (rust-lang/rust-analyzer#NNNN).

## Design notes

- Why a notification and not a server-side hold: a server that holds a request until its index is complete removes the empty answer but cannot say whether what is being waited for is an index or a broken server, cannot say which edits the answer includes, and does not let a client that would rather proceed with a partial answer do so. To the client a held request is indistinguishable from an unresponsive server. The proposal does not forbid holding; a server that holds reports `indexing` while it holds.
- Why `health` and `readiness` are separate: a failed index is not a state of readiness, and waiting for `ready` on a broken server is the wrong behavior. `health: "error"` tells the client to stop waiting.
- Why the guarantees name what is missing: a boolean "complete" is unverifiable. "computed over the whole workspace, `workspace/symbol` capped at 128" is, and a client can act on it (narrow the query, compare the count with the cap).
- No time: no value changes on the grounds of elapsed time. "no signal for a while" is not `ready`.
- Forward compatibility: a client ignores a field it does not know, and reads nothing from an axis whose value it does not know.

## Ask

Feedback on the shape, and whether a PR against the 3.19 specification text (`proposed.serverState.md` is already in that format) is the right next step.
````
