# Claude Code への提出

[../upstream-submissions.md](../upstream-submissions.md) の戦略に沿って Claude Code に出した文面と、提出後の反応・再測定の記録。出した文面は一字も変えずに残す。現在地（何を出し、何が返ってきたか）は同文書の一覧を見る。

## (1) anthropics/claude-code#76870 へのコメント

````markdown
Measured on 2.1.266 today, direct to rust-analyzer with a recorder on the server's stdin: `initialized`, `didOpen` and `textDocument/references` go out within 1 ms of the `initialize` response, rust-analyzer answers `[]` 2 ms later, and the tool result says "No references found" for a function with two call sites in the same crate.

On what to wait for, since a fix needs a signal:

- `$/progress` alone is not it. typescript-language-server sends no progress until a file is opened, its first `references` answers with the declaration only, and when tsserver crashes it sends a progress "end" that looks like completion (#82416).
- rust-analyzer has `experimental/serverStatus` (`quiescent: true` once the initial load is done; Zed waits for it). jdtls has `language/status`, Dart has `$/analyzerStatus`, gopls has nothing readable today. Each server says it differently and most say nothing.

I have been working on exactly this gap: a small protocol, `experimental/serverState` (`readiness: initializing | indexing | ready`, plus `health`, so a crashed server is an error rather than an empty answer), and a transparent proxy, lsp-det (https://github.com/tagawa0525/lsp-det), that speaks it for 20+ servers by reading each server's own signal and holds cross-file requests until `ready`. It runs from a plugin's `.lsp.json` with no client change (`"command": "lsp-det", "args": ["--", "rust-analyzer"]`). Measured through it with Claude Code on a larger crate: the `references` sent right after startup is held 6.7 s and comes back complete, and the client's request did not time out even at an 82 s hold on a big workspace.

For the client, the minimal step would be: if the server declares `serverStateProvider`, wait for `readiness: "ready"` before the first cross-file request, or put "index not complete" in the tool result instead of an empty list. The measured cost of not doing so, with a fixed prompt: the agent deleted a function that was still in use, in TypeScript and in Go, because the first `references` came back empty (https://github.com/tagawa0525/lsp-det/blob/main/docs/research/claude-code-dogfooding.md, round 6).
````

## (2) anthropics/claude-code#82416 へのコメント

````markdown
On request 1 (never return an empty result for a server failure): one concrete way typescript-language-server itself produces that empty result has a fix upstream, typescript-language-server/typescript-language-server#1125. When its tsserver child is killed by a signal (SIGKILL, or Node's own OOM abort, `{code: null, signal: 'SIGABRT'}`), `exitCode` is `null`, the `if (exitCode)` in `onExit` skips the shutdown, and the language server stays alive answering `[]` to every `references`. Reproduced on 5.3.0 and 6.0.0: `references` returns 2 locations; `kill -KILL <tsserver pid>`; the next `references` returns `[]` with no error, the only trace being a type 1 `window/logMessage` `[tsserver] Exited. Code: null. Signal: SIGKILL`. The PR makes the language server exit, as it already does for a non-zero exit code.

That fix relies on the client doing request 2 (respawn when the tracked process is gone). Once it lands, every tsserver crash ends in exactly symptom 2 above: the language server process is gone and the client keeps answering `server is running`. So the two requests go together.

For the wedged-but-alive case (symptom 1) there is no standard signal a client could check today. I am proposing one, `experimental/serverState` (`readiness` plus `health`, so a crashed or never-loaded server is an error rather than an empty answer; details in my comment on #76870), and lsp-det (https://github.com/tagawa0525/lsp-det) implements it as a proxy in front of typescript-language-server. After the crash above, the same `references` through it comes back as an error (`health: error`, with the tsserver exit message) instead of `[]`.
````

## (3) anthropics/claude-code#85225 へのコメント

````markdown
One more data point on the part that matters for the fix, that sending the notification is sufficient. I have a proxy, lsp-det (https://github.com/tagawa0525/lsp-det), that sends `workspace/didChangeWatchedFiles` on the client's behalf when the client declares neither the capability nor the notification. With Claude Code 2.1.261 and gopls, which does not watch the disk itself: direct, a file created with Bash stays invisible for the whole session and `references` keeps the old count; through the proxy, one `Created` notification goes out before the next `references` and the new file is in the answer. Same with pyright. The agent-level consequence, measured with a fixed prompt: direct, it deleted a Go function whose only caller was in the Bash-created file (`go build` then fails); through the proxy it did not (https://github.com/tagawa0525/lsp-det/blob/main/docs/research/claude-code-dogfooding.md, rounds 5 and 6).

Re-checked on 2.1.266 today: `initialize` still sends `workspace: {configuration: false, workspaceFolders: false}` with no `didChangeWatchedFiles`, and the session sends only `initialized` and `didOpen`.
````

## (4) anthropics/claude-code#16360 へのコメント

````markdown
Since csharp-ls fixed its side (0.24.0), the client gap that remains from this issue is `workspace.configuration: false`. Still the case on 2.1.266: `initialize` sends `workspace: {configuration: false, workspaceFolders: false}`. Two servers where it shows: nixd logs `workspace/configuration: client does not support workspace configuration` and falls back to its default expression, and nil likewise runs with its defaults, so per-project settings (which flake to evaluate, for instance) cannot reach the server from a Claude Code plugin. Answering `workspace/configuration` (with the plugin's settings, or `null` per item) would close that.
````

## (5) 新規 issue（`shutdown` の `params: {}`）

Bug report のフォームの欄に合わせる。Title: `LSP client sends shutdown with params: {}; rust-analyzer rejects it and the client never sends exit`

````markdown
**What's Wrong?**

The LSP client sends the `shutdown` request with `"params": {}`. LSP defines `shutdown` with no params. rust-analyzer (the `lsp-server` crate deserializes the params as unit) answers with an error, `-32602 Failed to deserialize shutdown: invalid type: map, expected unit; {}`, Claude Code logs `Failed to stop LSP server '…': …` and disconnects without sending `exit`. The server is left to notice the closed pipe on its own. gopls, pyright and nixd accept the extra `{}`, so this is visible only with strict servers, but any plugin that runs rust-analyzer hits it on every session end.

**What Should Happen?**

Send `shutdown` without `params` (or with `null`), and send `exit` after the `shutdown` response whether it succeeded or not, as the specification's shutdown sequence requires.

**Error Messages/Logs**

From `--debug-file`, 2.1.266:

```text
[LSP PROTOCOL plugin:…:rust-analyzer] Sending request 'shutdown - (2)'.
[LSP PROTOCOL plugin:…:rust-analyzer] Received response 'shutdown - (2)' in 2ms. Request failed: Failed to deserialize shutdown: invalid type: map, expected unit; {} (-32602).
[ERROR] Failed to stop LSP server 'plugin:…:rust-analyzer': Failed to deserialize shutdown: invalid type: map, expected unit; {}
```

The raw request, recorded on the server's stdin: `{"jsonrpc":"2.0","id":2,"method":"shutdown","params":{}}`. No `exit` follows.

**Steps to Reproduce**

1. A plugin whose `.lsp.json` runs rust-analyzer (any version; 2026-08-03 here) for `.rs`, and a Cargo project.
2. `claude -p "Use the LSP tool once: findReferences on src/lib.rs line 1 column 8" --plugin-dir <plugin> --allowedTools LSP --debug-file /tmp/cc.log`
3. `grep -n shutdown /tmp/cc.log`. To see the raw request, put a recorder in the server's place: https://github.com/tagawa0525/lsp-det/tree/main/scripts/claude-code/tee-probe

**Claude Model** any (the LSP client is model-independent; Sonnet here)

**Is this a regression?** No (same on 2.1.259, 2.1.261, 2.1.263, 2.1.266)

**Claude Code Version** 2.1.266

**Platform** CLI. **Operating System** Linux (NixOS). **Terminal/Shell** bash
````

## (6) 新規 issue（`didClose` を送らない。#64276 はロック済み）

Title: `LSP client never sends textDocument/didClose (follow-up to #64276, closed by the stale bot and locked)`

````markdown
**What's Wrong?**

#64276 reported this with a Lean server (one worker process per open file, 51 workers, ~100 GB RSS). It was closed as not planned by the stale bot without a maintainer decision and is now locked, so I am filing the follow-up with a fresh measurement.

On 2.1.266 the client sends `textDocument/didOpen` for each file the LSP tool touches and never `textDocument/didClose`: over a session the notifications on the server's stdin are `initialized` and `didOpen` only (recorded with a wrapper in the server's place; the same on 2.1.259 and 2.1.261 across 29 debug logs, https://github.com/tagawa0525/lsp-det/blob/main/docs/research/claude-code-dogfooding.md, round 4). Beyond the resource growth in #64276, this has a correctness consequence: LSP says that while a document is open its content is owned by the client, so the server must not read the file from disk. A file the agent opened once through the LSP tool and later changed with Bash, git, or a formatter stays at its `didOpen` content for the rest of the session, even for a server that would otherwise re-read it.

**What Should Happen?**

Send `didClose` when the tool is done with a document (or keep a bounded set of open documents and close the least recently used), so that servers can release per-document state and go back to the file on disk.

**Error Messages/Logs**

None; the notification is simply absent. Recorded stdin of the server for one `findReferences` on 2.1.266: `initialize`, `initialized`, `textDocument/didOpen`, `textDocument/references`, `shutdown`.

**Steps to Reproduce**

1. Any plugin `.lsp.json` (rust-analyzer here) and a project.
2. Put a recorder in the server's place (https://github.com/tagawa0525/lsp-det/tree/main/scripts/claude-code/tee-probe) and run `claude -p "Use the LSP tool: findReferences on …, then hover on …" --plugin-dir <plugin> --allowedTools LSP`.
3. `grep -c didClose <log>` is 0.

**Claude Model** any (Sonnet here). **Is this a regression?** No. **Claude Code Version** 2.1.266. **Platform** CLI. **Operating System** Linux (NixOS). **Terminal/Shell** bash
````
