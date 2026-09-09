# Measurement of `workspace/symbol` capping (2026-09-04)

[日本語](workspace-symbol-truncation-measurement.ja.md)

Basis for ADR 0013. The protocol's `completeness` field (the name at the time) promised that the response is complete for the 11 methods listed in 7.0, including `workspace/symbol`. Serena's investigation ([serena-processing-around-lsp.md](serena-processing-around-lsp.md) (Japanese), chapter 4) found that it passes `limit: 128` to rust-analyzer, so we measured whether capping occurs across 4 servers.

## Conclusion

| Server                     | Version                  | Response to 300 matches | Cap                                                                                      |
| -------------------------- | ------------------------ | ----------------------- | ---------------------------------------------------------------------------------------- |
| rust-analyzer              | 2026-08-03 (nixpkgs)     | 128                     | **Yes.** Default of `workspace.symbol.search.limit` is 128. Setting it to 1000 gives 300 |
| gopls                      | 0.23.0                   | 100                     | **Yes.** `maxSymbols = 100` at `workspace_symbol.go:29`. Not configurable                |
| pyright                    | 1.1.412                  | 300                     | None                                                                                     |
| typescript-language-server | 5.3.0 (TypeScript 5.9.3) | 300                     | None                                                                                     |

Servers that cap the result never report it. The response is an ordinary `result` array, and nothing corresponding to `isIncomplete` exists for `workspace/symbol` (LSP has that only for completion). No log is emitted either.

## Reason for the limit (source)

- rust-analyzer `crates/rust-analyzer/src/config.rs:1068-1071`: a client like VS Code reissues the search every time the results are narrowed, so the first search does not need every result; a client that wants every result up front may need to raise the limit.
- gopls `gopls/internal/golang/workspace_symbol.go:27-29`: the maximum number of results to send to the client. It keeps the top 100 by fuzzy score in a fixed-length array. A comment in the same file notes that LSP's `workspace/symbol` has no server-side filter (microsoft/language-server-protocol#941).

Both are built as a score-ordered fuzzy search for an editor picker. There was never a contract to enumerate everything.

## Measurement method

For each language, we built a fixture with 300 top-level symbols (3 files x 100) sharing the prefix `wsymprobe`, launched the server directly without lsp-det in front of it, sent `initialize` → `initialized`, and waited for the readiness signal (rust-analyzer: `quiescent: true`; gopls: the end of "Setting up workspace"; pyright: "Found 3 source files"; tsls: the end of "Initializing JS/TS language features" after `didOpen`) before sending `workspace/symbol` with `{"query": "wsymprobe"}` and counting the results whose name contains `wsymprobe`. rust-analyzer was also measured with `initializationOptions.workspace.symbol.search.limit = 1000`.

We also recorded the response to an empty query (`""`): rust-analyzer returned the 6 crate roots, gopls returned `null`, pyright returned `[]`, and tsls returned 300.

## What not to generalize

- Only the versions pinned by flake.nix were measured. The limit can change across versions (and for rust-analyzer, also with configuration)
- Servers other than these 4 (jdtls, clangd, etc.) were not measured
- We measured only whether capping occurs. Whether the score-ordered ranking matches what an agent wants is a separate question
