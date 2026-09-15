# typescript-language-server への提出

[../upstream-submissions.md](../upstream-submissions.md) の戦略に沿って typescript-language-server に出した文面と、提出後の反応・再測定の記録。出した文面は一字も変えずに残す。現在地（何を出し、何が返ってきたか）は同文書の一覧を見る。

## `fix: stop the server when tsserver is killed by a signal`

PR 先: `typescript-language-server/typescript-language-server`（master）。ブランチ: `tagawa0525/typescript-language-server` の `tsserver-exit-by-signal`（2 コミット。修正本体と Copilot の指摘への対応 6c21094、`src/lsp-server.ts` と `src/ts-client.test.ts`。fork の PR #1 で CI を通してある）。見出しは `## Summary / ## Changes / ## Tests`（PR 本文の見出しは常に英語。ユーザーの決定、2026-09-09）。文面は 2026-09-09 にユーザーの確認済み。同日 typescript-language-server/typescript-language-server#1125 として提出。

本文:

```markdown
## Summary

Since #305 the language server exits when tsserver crashes (#302), so that the client restarts it. The exit is only honoured when tsserver reports an exit code, though (`if (exitCode)` in the `onExit` handler in `lsp-server.ts`). A tsserver killed by a signal reports `exitCode: null` — SIGKILL from the OOM killer, or SIGABRT from Node's own out-of-memory abort (`FATAL ERROR: ... JavaScript heap out of memory` ends the child with `{ code: null, signal: 'SIGABRT' }` on Linux) — and the language server keeps running. Every later request is then answered with `ServerResponse.NoServer`, which the handlers turn into an empty success: `textDocument/references` returns `[]`, `textDocument/definition` returns `null`. A client cannot tell that from a real answer.

How to reproduce (6.0.0): open a file, `kill -9` the tsserver child processes, then send `textDocument/references`. The server logs `[tsserver] Exited. Code: null. Signal: SIGKILL`, answers `{"result": []}`, and is still running ten seconds later.

## Changes

The `exitCode` check dates from 507db40 (2022), a logging-only refactor made right after #536 had removed the exit altogether: at that time the `exit` listener of the tsserver process also fired for the server's own shutdown, which kills tsserver with SIGTERM (`code: null`), and the check only kept that from being logged as a crash. Since #585 the tsserver client disposes its exit handlers before killing the process, so `onExit` only runs for an exit the server did not ask for. This change stops the server on every such exit, with the same error as before.

## Tests

Two cases in `ts-client.test.ts` pin down the two facts the change rests on: a tsserver killed by a signal reaches `onExit` with a null exit code, and `shutdown()` does not reach `onExit`. `pnpm test`, `lint` and `typecheck` pass (CI on my fork: Linux, macOS, Windows × Node 22 / 24). End to end, with this change the server exits with code 1 right after the `kill -9` above.
```
