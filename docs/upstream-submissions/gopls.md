# gopls への提出

[../upstream-submissions.md](../upstream-submissions.md) の戦略に沿って gopls に出した文面と、提出後の反応・再測定の記録。出した文面は一字も変えずに残す。現在地（何を出し、何が返ってきたか）は同文書の一覧を見る。

## (a) golang/go#78273 へのコメント

先: [golang/go#78273](https://github.com/golang/go/issues/78273)（"x/tools/gopls: gopls sends verbose error messages via $/progress instead of window/showMessage"。open。adonovan が "an abridged version displayed in the client UI" に同意）。2026-09-09 にユーザーの確認を得て[提出](https://github.com/golang/go/issues/78273#issuecomment-5595205676)。

本文:

```markdown
A related observation from the client side (gopls v0.23.0; the code is the same at master). The critical "Error loading workspace" status is the only signal that the workspace failed to load, and how it reaches the client depends on `window.workDoneProgress`:

- With it: a `$/progress` begin with title "Error loading workspace", held open until the problem is fixed ("Done.").
- Without it: `progress.Tracker.Start` falls back to `window/showMessage` with `type: 4` (Log), carrying only the message (the title is dropped), and the resolution arrives as `type: 3` (Info) "Done.". "Loading packages..." arrives as Log and "Finished loading packages." as Info through the same fallback.

So for a client that does not implement progress (Claude Code, for example, declares no `window` capability at all) a workspace load failure looks like any other log line, and carries a lower severity than the "Finished loading packages." that precedes it. The requests themselves are honest meanwhile: `textDocument/references` and `textDocument/definition` answer `no package metadata for file …` as errors while the workspace is broken (`workspace/symbol` answers `null`), so what is missing is only the severity and identity of the status.

Suggestion, in the spirit of the abridged message above: when `Tracker.Start` falls back to `showMessage`, let the severity carry what the title carried (`Error` for `WorkspaceLoadFailure`, `Info` for the others) and keep the full text in the server log. Reproduction (a two-file module whose go.mod has an unterminated `require (` block, a stdio client with no `window` capability) and the message logs: https://github.com/tagawa0525/lsp-det/blob/main/docs/research/gopls-health-measurement.md.
```

## (b) golang/go に新規 issue

2026-09-09 にユーザーの確認を得て golang/go#81400 として提出。

タイトル: `x/tools/gopls: after a workspace load error is fixed, every later go.mod change makes requests fail with "no package metadata" for about a second`

本文（golang/go の issue template の見出しに沿う）:

```markdown
### gopls version

golang.org/x/tools/gopls v0.23.0 (go1.26.7 linux/amd64). The code involved (`MetadataForFile` and `clone` in `internal/cache/snapshot.go`) is the same at master.

### What did you do?

1. A two-file module: `go.mod` (`module fixture` / `go 1.21`), `a.go` with `func Target() {}`, `b.go` with `func Caller() { Target() }`. Open `a.go` over stdio and wait for "Finished loading packages.".
2. Break `go.mod` on disk (append an unterminated `require (` block) and send `workspace/didChangeWatchedFiles` (Changed). "Error loading workspace" appears, and requests on `a.go` fail with `no package metadata for file …/a.go` (expected).
3. Fix `go.mod` on disk and send `didChangeWatchedFiles`. "Done." arrives; a second later `textDocument/references` on `Target` works again.
4. Append a comment to `go.mod`, send `didChangeWatchedFiles`, and immediately send `textDocument/references` (or `definition`) on `a.go`, repeating every 50ms.

### What did you see happen?

For about 1.0s after each `go.mod` change, every request on `a.go` fails with `no package metadata for file file:///…/a.go`; then they succeed again. This repeats on every later `go.mod` change for the rest of the session (three changes in a row, two runs).

### What did you expect to see?

The same as in a session that never had a load error. There, step 4 answers correctly at once (16ms after the change in my runs), because `MetadataForFile` loads the file's package inline (`s.load(ctx, NoNetwork, fileLoadScope(uri))`) when its metadata was invalidated.

### Why, reading the source

While the workspace was broken, `MetadataForFile` found no package for `a.go` and added it to `snapshot.unloadableFiles`. A URI leaves that set only through a metadata-affecting change to that `.go` file itself (`clone`: "typing in a file doesn't necessarily make it loadable"); fixing `go.mod` does not remove it. After the recovery the file is loadable again, but it stays marked unloadable, so on every later `go.mod` change (`reinit` in `clone` invalidates the metadata) `MetadataForFile` skips the inline load (`(shouldLoad || len(pkgs) == 0) && !unloadable`) and returns the error until the background reload has run.

A possible fix: clear `unloadableFiles` on the `reinit` path of `clone` (go.mod / go.work / go.sum changed on disk), since a workspace-level change is exactly the kind of change that can make a file loadable again.

Logs of both kinds of session and the probe script: https://github.com/tagawa0525/lsp-det/blob/main/docs/research/gopls-health-measurement.md and https://github.com/tagawa0525/lsp-det/blob/main/scripts/gopls/health-probe.py.
```

## 提出後の反応（2026-09-09）

両方とも、付いたのは Go チームの自動ボット gabyhelp の "Related Issues" だけで、メンテナの返答はまだない。golang/go#81400 には gopherbot が `gopls` / `Tools` のラベルと milestone Unreleased を付けた。ボットが挙げた issue のうち開いているものは全部読み、こちらの主張に関わるのは次の 3 点。

- golang/go#81400 の重複はない。golang/go#36589（go.mod 変更時の metadata 無効化を差分で判定する提案）は 2026-08 に obsolete とされ、golang/go#79585 は branch 切り替え後に `shouldLoad` が残って go.mod を書き換える別の不具合、golang/go#68002 は読み込みの範囲を絞る親 issue で、回復後に `unloadableFiles` に残る件には触れていない
- [golang/go#50885](https://github.com/golang/go/issues/50885) で findleyr が 2022 年に、critical error status を `$/progress` を開いたまま保持する手法は "tricky, and may be problematic" で各クライアントへの影響を調べるべきと書いている。golang/go#78273 に出した「`window.workDoneProgress` を宣言しないクライアントには Log に落ちる」はその影響の一例
- [golang/go#76137](https://github.com/golang/go/issues/76137)（closed）で adonovan が、"Finished loading packages." は metadata の取得完了であって型検査の完了ではない、準備完了を知るにはサーバーを黒箱として扱いその状態でしか答えられない要求を投げて待て、と答えている。gopls の提出を readiness ではなく health に縮めた判断（[research/gopls-health-measurement.md](../research/gopls-health-measurement.md)）と整合する

ユーザーの指示で、ボットへの返信として上を要約したコメントを 2 件に出した（[golang/go#81400 のコメント](https://github.com/golang/go/issues/81400#issuecomment-5599635641)、[golang/go#78273 のコメント](https://github.com/golang/go/issues/78273#issuecomment-5599635886)）。

golang/go#81400 への本文:

```markdown
For the record, none of the related issues above covers this one: #36589 was marked obsolete last month; #79585 is about `shouldLoad` entries surviving a branch switch (a different map, and its symptom is gopls writing to go.mod, not requests failing); #68002 is about how loads are scoped, not about a file staying in `unloadableFiles` after the workspace has recovered. The closest prior discussion is #76137, where the answer was that requests such as `references` block on the initial load — which is exactly what makes this session-long window surprising: a session that never had a load error answers at once after a go.mod change, and only a session that once recovered from one keeps failing.
```

golang/go#78273 への本文:

```markdown
Two of the related issues above bear on the fallback observation. In #50885 findleyr noted in 2022 that holding a critical status open as a hanging progress notification "is tricky, and may be problematic" and that its ramifications on various clients should be looked into; the Log-severity fallback for clients without `window.workDoneProgress` is one such ramification, and it affects exactly the clients that cannot see the hanging notification. In #76137 adonovan pointed out that "Finished loading packages." only means package metadata was obtained, not that type-checking is complete. That is why the suggestion above is limited to the severity of the failure status: it does not ask gopls for a readiness signal, only that a failure not arrive with a lower severity than the success message before it.
```

## 上流の修正 CL の検証（2026-09-12、提出済み）

golang/go#81400 に Go チームの Hana Kim が [CL 830924](https://go.dev/cl/830924)（`clone` の `reinit` で `unloadableFiles` を空にする）を出し、レビューで Peter Weinberger が [CL 830844](https://go.dev/cl/830844)（`didChangeWatchedFiles` の 50 ms デバウンス）が入ると回帰テストが空振りすると指摘した。4 つのビルドで検証した結果と機構は [research/gopls-health-measurement.md](../research/gopls-health-measurement.md) の「提出後: 上流の修正 CL の検証」。

下は golang/go#81400 へのコメント。2026-09-12 にユーザーの承認を得て[提出](https://github.com/golang/go/issues/81400#issuecomment-5644035663)。レビュー（2026-09-12）で、報告書の「診断が届くまで要求を繰り返す形」を英文が "no deterministic form" と否定していた食い違いを直し、probe の path を直接書いた。

```markdown
I built gopls at CL 830924 patch set 2 (a373bba32) and at its parent on master (249605012), and both again with CL 830844 patch set 2 (the `didChangeWatchedFiles` debounce) cherry-picked on top, and ran `scripts/gopls/health-probe.py` from the repository linked above against each (go1.27.0 linux/amd64; two runs each; three go.mod changes after the recovery, each followed by `textDocument/references` or `definition` on `a.go` repeated every 50ms).

The fix works with and without the debounce. At the parent, the first request after each change fails with `no package metadata for file …/a.go` and the first success comes 1.015–1.018s after the change. At the parent plus debounce, the request sent 1ms after the notification succeeds, answered from the snapshot before the change; the failures start at 52ms, after the flush, and last until 1.065–1.072s. With CL 830924, with or without the debounce, no request fails, and the first answer after each change comes within 18ms.

On the regression test in CL 830924 (a `didChangeWatchedFiles` for go.mod followed at once by `References`), which under the debounce would be answered from the snapshot before the change, as above: I traced with a logging build where the window closes. It closes at the first `AwaitInitialized` after the reinit, which is normally the diagnostics pass after `diagnosticsDelay` (the window follows that setting: 0.356s at `300ms`, 2.03s at `2s`). But `Session.DidModifyFiles` also reaches it: `invalidateViewLocked` calls `prevSnapshot.AwaitInitialized` before cloning, so on today's master any editor operation that arrives after the go.mod notification reloads the workspace synchronously inside that operation's handler, and a request sent after it succeeds on every build. With the debounce the same editor operation flushes the go.mod change into the same modification pass, the snapshot that `AwaitInitialized` waits for is the one before the change, and the request that follows does see the window. So "notification, then request" is meaningful today and vacuous under the debounce, and "notification, editor operation, request" is the other way round; no single fixed sequence exercises the fix on both trees.

Two forms that do not depend on this. The state the fix changes is `unloadableFiles` across one `clone` with a reinit, so a test in package `cache` that applies one go.mod change to a snapshot holding an unloadable file and asserts that `unloadableFiles` is empty afterwards covers it, without either timer. End to end, the `publishDiagnostics` for go.mod arrives after the reload on all four builds, so a test that sends the notification and then repeats the request until the diagnostics for the changed go.mod arrive, failing on any `no package metadata` in between, fails on both trees without the fix (from 1ms without the debounce, from 52ms with it) and passes on both with it.

Also unchanged at CL 830924: requests while go.mod is broken on disk still fail with the explicit error, and a session that never had a load error still answers at once after a go.mod change.
```

## デバウンス CL 830844 が通知の直後の要求に変更前の答えを返す（2026-09-12、提出済み）

golang/go#81408（重複する `go list`）に対する Peter Weinberger の [CL 830844](https://go.dev/cl/830844)（`didChangeWatchedFiles` の 50 ms デバウンス）は、通知の後の要求を変更前の snapshot から答える。測定は [research/gopls-health-measurement.md](../research/gopls-health-measurement.md) の「デバウンス CL 830844 は通知の直後の要求に古い答えを返す」。lsp-det の準拠テスト 7.3.2 はこの CL で落ちる。

下は golang/go#81408 へのコメント。2026-09-12 にユーザーの承認を得て[提出](https://github.com/golang/go/issues/81408#issuecomment-5644035909)。他のサーバーの例はソースと実測で確かめた範囲に限り（pyright は `tests/conformance.rs` の 7.3.2、rust-analyzer は `dispatch.rs` の `ContentModified` と main loop の `Retry`）、CL の答えを「正しくない」とは言わず、クライアントが「送った通知が適用されたか」を知れることを求める形にした。プロキシ（lsp-det）の説明はこのスレッドの文脈の外なので置かない。レビュー（2026-09-12）で nil の例を外した。nil は flake.lock の変更で end を即座に出し 100 ms 後に begin を出すが、その間は古い情報のまま `ready` に見える（[research/nil-readiness-measurement.md](../research/nil-readiness-measurement.md)。nil への提案 (3)）ので、この CL と同じ穴であり手本にならない。代わりに、CL 自身が `gopls mcp` の `fileOf` では `session.DidModifyFiles` を直接呼んで debounce を迂回している事実（コミットメッセージ "so snapshot queries are not delayed by the debounce timer"）を足した。

```markdown
A measurement on CL 830844 patch set 2 (dacbdbd80), cherry-picked onto 249605012, against 249605012 itself, both built with go1.27.0 linux/amd64. Two-file module: `a.go` with `func Target() {}` (open in the editor), `b.go` with `func Caller() { Target() }` (not open). A client over stdio removes the call from `b.go` on disk, sends `workspace/didChangeWatchedFiles` (Changed) for it, and immediately starts asking `textDocument/references` on `Target` every 10ms.

Today, the first request, sent right after the notification, already answers 0 references: gopls handles the notification before the request that follows it. With the CL, the requests sent up to 40ms after the notification answer 1 reference, the call that no longer exists; the first to answer 0 is the one sent at 50ms or 60ms, once the debounce has flushed. Sending the notification 20 times 30ms apart (a burst, the case the CL is for) after putting the call back, the requests sent up to 500ms, the `debounceMax`, answer 0, while the file on disk has had the call the whole time; the one sent at 510ms answers 1. Two runs each, same numbers (times are send times; the requests go out on a fixed 10ms schedule regardless of when the answers arrive, and the burst notifications are sent from the same loop, a notification first when both are due at the same time).

So the CL changes what a client can rely on: after the change, a request that follows `didChangeWatchedFiles` on the same connection may be answered from the snapshot before the change, and nothing tells the client which it got. The CL flushes pending changes when an editor operation arrives, to keep on-disk files and overlays ordered, but a request does not flush them. That matters most for the client that only edits on disk and then asks, which is what tools driving gopls without an editor do. The CL itself treats one such client differently: `gopls mcp`'s `fileOf` now applies its file events through `session.DidModifyFiles` directly, bypassing the debounce, "so snapshot queries are not delayed by the debounce timer". An LSP client in the same position has no such path.

For reference, two ways other servers coalesce work while letting a client tell what its answer reflects. pyright applies the notification at once and defers only the analysis: the watched-file change marks the file dirty synchronously and schedules the reanalysis on a timer (`markFilesDirty` / `scheduleReanalysis` in `packages/pyright-internal/src/analyzer/service.ts`), and in my measurement a request right after the notification answered with the new content. rust-analyzer hands the change to its VFS loader and coalesces VFS events per loop turn, but a request whose snapshot is invalidated underneath it is retried or answered with `-32801 ContentModified` (`handlers/dispatch.rs`), never from the old snapshot, and for a change that triggers a reload `experimental/serverStatus` reports `quiescent: false` and then `true`. In gopls terms the first pattern would be keeping the snapshot invalidation in order and coalescing only the reload; the smallest change would be draining the pending changes at the start of request handling too, as `didModifyFiles` does for editor operations: a burst with no request in the middle is still one modification pass, and a request in the middle costs one flush, which is what it costs today.

Whichever way this goes, what the client above needs is to be able to tell whether a notification it sent has been applied. Today's ordering gives that implicitly. A flush on request, a `ContentModified` instead of an answer from before the change, or a notification when pending changes have been applied would each give it explicitly. An answer from before a change the client has already reported, with no signal, is the one outcome it cannot handle.

The probe is `scripts/gopls/health-probe.py --scenario stale-after-watched-change` at https://github.com/tagawa0525/lsp-det.
```
