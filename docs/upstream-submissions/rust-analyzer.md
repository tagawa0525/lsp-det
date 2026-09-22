# rust-analyzer への提出

[../upstream-submissions.md](../upstream-submissions.md) の戦略に沿って rust-analyzer に出した文面と、提出後の反応・再測定の記録。出した文面は一字も変えずに残す。現在地（何を出し、何が返ってきたか）は同文書の一覧を見る。

## issue（両案）

issue 先: `rust-lang/rust-analyzer`。ブランチ: `tagawa0525/rust-analyzer` の `server-status-readiness`（案 A、1 コミット）と `server-state`（案 B、3 コミット）。PR は相手が選んだ方だけを出す。第 2 段（準備 5〜6 の後）。題名: `experimental/serverStatus: tell clients when answers to workspace-wide requests are complete (a readiness field, or a successor notification)`

本文:

````markdown
## Summary

`experimental/serverStatus` is documented as a status line for the end user, and `quiescent` answers "is there pending background work?". A client that needs to know whether an answer to `references`, `workspace/symbol`, `rename`, … is complete has to read `quiescent` as "ready", and that reading is wrong at the moment it matters most: before the first workspace has been loaded, the server is trivially quiescent. rust-lang/rust-analyzer#10888 asked for a readiness notification and was closed as "already exists" pointing at `serverStatus`; this issue is about the part that does not exist yet, with two implementations to choose from. I will open a PR for whichever you prefer.

## What a client sees today

Measured with rust-analyzer 2026-08-03 (nixpkgs) over stdio, the client declaring `experimental.serverStatusNotification` (script: https://github.com/tagawa0525/lsp-det/blob/main/scripts/rust-analyzer/status-probe.py):

- A directory without `Cargo.toml`: the first notification, 5 ms after `initialized`, is `{health: "warning", quiescent: true, message: "Failed to discover workspace. …"}`. Nothing has been loaded and the fetch has not started (`GlobalState::run` reports the status before `fetch_workspaces_queue.request_op("startup")`). `quiescent: true` here means "nothing in flight", not "ready". 1 ms later: `{health: "error", quiescent: true, …}`.
- A one-crate project: `{quiescent: false}` at 5 ms, `{quiescent: true}` at 1.7 s. The initial `last_reported_status` is `quiescent: true`, so a client that missed a notification (there is no request form) has to assume the server is ready.
- The doc says "this functionality is intended primarily to inform the end user … Clients are discouraged from but are allowed to use the `health` status to decide if it's worth sending a request." There is no field a client may rely on for completeness.

Coding agents are the client that needs this: they send `references` right after starting the server and take an empty answer as a fact (anthropics/claude-code#76870). Zed already reads `serverStatus` (`crates/project/src/lsp_store/rust_analyzer_ext.rs`), so there is a consumer for whichever shape is chosen.

## Option A: a `readiness` field in `ServerStatusParams`

Branch: https://github.com/tagawa0525/rust-analyzer/tree/server-status-readiness

```typescript
interface ServerStatusParams {
    health: "ok" | "warning" | "error",
    quiescent: boolean,
    /// initializing: no workspace loaded yet.
    /// indexing: workspaces are being (re)loaded or caches primed; answers may be incomplete.
    /// ready: fully loaded; answers are complete.
    readiness: "initializing" | "indexing" | "ready",
    message?: string,
}
```

Computed from the same facts as `quiescent`: `initializing` while `workspaces` is empty and no load has failed, `ready` when `is_fully_ready()`, `indexing` otherwise. The initial `last_reported_status` starts at `initializing`, so the notification traffic does not change. The doc paragraph is amended: this field, unlike the others, is meant for clients that decide whether to trust an answer. `quiescent`, the VS Code extension and every other client keep working as they are.

Measured on the branch: `initializing` (4 ms) → `indexing` (0.2 s) → `ready` (0.54 s) on the one-crate project; `initializing` → `{health: "error", readiness: "ready"}` on the directory without `Cargo.toml` (a failed load is reported on the health axis, and readiness says the failure is settled).

Smallest change. What it does not give: a request form (a client that attaches late waits for the next notification), and a declaration of what `ready` covers.

## Option B: a successor, `experimental/serverState`

Branch: https://github.com/tagawa0525/rust-analyzer/tree/server-state

A request `experimental/serverState` answering `{health, readiness, message}` at any time after `initialize`; a notification `experimental/serverStateChanged` sent when `health` or `readiness` changes, if the client declares `experimental.serverState`; and a server capability `serverStateProvider` that declares the guarantees by naming what is missing (`coverage: {scope: "workspace", incomplete: {"workspace/symbol": <workspace.symbol.search.limit>}}`, `freshness: {fileChanges: ["Created", "Changed", "Deleted"]}`). `serverStatus` is untouched. An undiscovered workspace is `health: "error"` there (nothing workspace-wide can be answered), which A leaves at `warning` for compatibility.

The vocabulary follows the server state protocol specification (https://github.com/tagawa0525/lsp-det/blob/main/docs/spec/server-state.md), written as a candidate for LSP itself; `serverStatus` is the closest existing vocabulary and the one it was modelled on.

## Either way

lsp-det, a transparent proxy that derives the same three values from `quiescent` today, reads the field when present (A) or passes the notification through untouched (B), so agents that already sit behind it are covered by both. Both branches pass `cargo xtask tidy` and the `rust-analyzer` lib tests, with `lsp-extensions.md` and its hash updated.
````

## 提出後の反応（2026-09-14）

rust-lang/rust-analyzer#23331 に ChayimFriedman2（メンテナ）が 2026-09-13 に返答した:

> I'd go with simplified option A: a single `ready: bool` field. It should be `false` when loading workspace, and `true` after it's loaded, including during cache priming, because it doesn't affect clients - it might mean the server will be slower to respond, but responses will still be correct.

読み取り:

- 案 A（`serverStatus` への field 追加）が選ばれ、案 B（後継の `experimental/serverState`）は選ばれなかった
- 3 値の `readiness` ではなく 2 値の `ready: bool`。境界は「ワークスペースの読み込み」で、priming は含めない（priming 中の答えは遅いが正しい）。この主張はソースと一致する（priming は lazy な計算の事前実行で、要求はその有無に関係なく同じ問い合わせをその場で計算する。[research/rust-analyzer-quiescent-measurement.md](../research/rust-analyzer-quiescent-measurement.md) の末尾）
- ロードの失敗（`health: error`）の扱いにはコメントは触れていない。3 値版と同じく「失敗は決着している」の `ready: true` のまま

対応（2026-09-14）:

- fork の `server-status-ready` に作り直した（最初は 7d79ab49d1、上流 master f312032107 起点。提出時は上流 master 83449b2f18 に rebase した dd80082bde + テストの c6b47b9bef の 2 コミット）。`ServerStatusParams` に `ready: bool`（`reload.rs` の `is_ready` = `!nothing_loaded_yet && is_quiescent()`）、初期の `last_reported_status` は `ready: false`、`lsp-extensions.md` の field と注記と hash、`editors/code` の型。`cargo xtask tidy` と lib tests 99 件が通る。`server-status-readiness`（3 値版）は issue が指すので残す
- 実測（同報告の末尾）: 1 クレートで `{quiescent: false, ready: false}`（11 ms）→ `{quiescent: false, ready: true}`（0.56 s、priming 中）→ `{quiescent: true, ready: true}`（1 ms 後）。空の場所で `{warning, quiescent: true, ready: false}` → `{error, quiescent: true, ready: true}`
- lsp-det の写像は field があれば `true` → `ready`、`false` → `indexing`。受け入れ条件は `rust_analyzer_reports_ready_in_server_status`。fork の版で準拠テスト 7.1 / 7.2 / 7.3 も通る

fork での素振り（2026-09-15、ユーザーの指示「fork の GH Actions で練り、警告を潰してから本家に」）:

- rust-analyzer の CI は Rust のジョブに `if: github.repository == 'rust-lang/rust-analyzer'` があり fork では走らない。条件を fork 名に差し替えた 1 コミットだけを載せた CI 専用ブランチ `server-status-ready-ci` から fork の PR（tagawa0525/rust-analyzer#2。#1 は条件に気づく前のもので閉じた）を出し、全ジョブ（Rust 3 OS、cross 3 種、clippy、rustfmt、typo、miri、analysis-stats、coverage、proc-macro-srv、TypeScript 2 OS）を通した。annotation の 2 件は上流の `Cargo.toml` の未使用依存の警告で無関係
- fork の Copilot が 3 点を挙げた。(1) `cargo.autoreload = false` で reload が queued のままのとき `ready: true` になる → queued は「読み込み中」ではない。利用者が手動 reload まで今の workspace を使うと選んだ状態で、`quiescent` と同じく `health` の警告が伝える。そこで `false` にすると待つクライアントが手動 reload まで止まるので、field の目的に反する。返信して据え置き。(2) 複数 workspace の部分失敗で `ready: true` → 失敗は `health: error` の軸（仕様 6 章 5 項と同じ）。据え置き。(3) `ready` の遷移を観るテストがない → slow-tests に 2 件（1 クレート: 最初の status は `ready: false`、ロードは `ready: true` で終わり、以後戻らない。Cargo.toml なし: `ready: false` → `health: error` で `ready: true`）と支援関数 2 つを足した。`ServerStatusParams` に `Debug` を derive（hash 更新）。ローカルで 3 回、CI の 3 OS で通過

提出（2026-09-15）: rust-lang/rust-analyzer#23362（fork の `server-status-ready` c6b47b9bef、2 コミット）。issue #23331 には PR のリンクを 1 行で返した。出した本文（見出しは `## Summary` / `## Changes` / `## Tests`。帰属行なし。先行の tsls #1125 と同じ）:

題名: ``feat: report `ready` in experimental/serverStatus``

````markdown
## Summary

Implements the simplified option A from #23331: a single `ready: bool` field in `experimental/serverStatus`.

`quiescent` answers "is there pending background work?", which is not the question a client that wants complete answers to workspace-wide requests asks. Before the first workspace has been loaded the server is trivially quiescent (nothing is in flight yet), so a client that reads `quiescent` as "ready" reads it wrongly at exactly the moment it matters most; and the documentation reserves the notification for the end user.

## Changes

`ready` is `false` while the workspaces are being (re)loaded and `true` once they are, including while caches are being primed (priming only warms what a request computes on demand anyway, so answers during priming are slower but complete), as suggested in #23331. It is computed from the same facts as `quiescent` minus the priming queue, plus "at least one workspace has been loaded or the load has failed": a failed load is reported through `health`, and `ready` says the failure is settled. A reload that is only queued (`cargo.autoreload = false`) is not a load in progress: `ready` stays `true` and `health` carries the warning, as today. The initial `last_reported_status` starts at `ready: false`, so the notification traffic does not change.

The doc paragraph is amended: this field, unlike the others, is meant for clients that decide whether to trust an answer. `lsp-extensions.md` and its hash are updated, and `editors/code/src/lsp_ext.ts` gets the field. `ServerStatusParams` derives `Debug` for the test messages.

Measured over stdio with the script from the issue:

- one-crate project: `{quiescent: false, ready: false}` at 11 ms → `{quiescent: false, ready: true}` at 0.56 s (caches being primed) → `{quiescent: true, ready: true}` 1 ms later
- directory without `Cargo.toml`: `{health: "warning", quiescent: true, ready: false}` at 11 ms → `{health: "error", quiescent: true, ready: true}` at 12 ms

## Tests

Two slow tests observe the transitions: on a one-crate project the first status says `ready: false`, the load ends in `ready: true` and `ready` does not go back during the first load; on a directory without `Cargo.toml` the trivially quiescent first status says `ready: false`, and the failed load settles as `ready: true` with `health: error`. `cargo xtask tidy` and the `rust-analyzer` tests pass locally and on my fork's CI (all jobs, with the `github.repository` gate lifted there).
````

取り込まれて配布版に `ready` が載ったら、`TESTED_VERSIONS` にその版を足す前に準拠テストを当て、仕様 10 章の rust-analyzer の行を `ready` に改める（版を上げて 11 章に記す）

## 提出後の反応（2026-09-15）

PR #23362 に ChayimFriedman2 が 2026-09-15 にコメントした（2026-09-23 時点でそれ以外の動きはなく、`S-waiting-on-review` のまま）:

> CC @rust-lang/rust-analyzer for opinions on the new LSP extension.
>
> Also, @tagawa0525, I suspect you've used LLM for this. Please read [our AI policy](https://github.com/rust-lang/rust-analyzer/blob/master/AI_POLICY.md).

`AI_POLICY.md` の要点: AI を道具として使うのは可だが利用を開示する。メンテナへのコメント・PR 本文・質問への返答は人が自分の言葉で書き、AI の応答を貼らない。AI 由来の文脈は `>` の引用で開示し人の説明を添える。自律エージェントが開いた PR は閉じる。非母語話者には、母語で書いて AI 訳を引用ブロックで添える形を勧める（訳の言語は指定していない）。解析系クレートの規則と E-easy+E-has-instructions の規則は今回の変更には当たらない。

事実: issue #23331、PR #23362、issue への返信の文面はいずれも lsp-det 側（Claude）が起草し、ユーザーが判断と承認をした。開示はしていなかった。提出前に相手の AI 方針を確かめる工程がなかった（本文の規則に足した）。

Copilot のレビュー（2026-09-14、コメント生成 0 件、抑制 1 件）は「priming 中に `ready && !quiescent` の status があることを assert すべき」と言う。priming は `prefill_caches` かつ proc macro の読み込みが成功したときにだけ始まるので、この assert は環境次第で落ちる。入れない。

返信（2026-09-23）: ユーザーが日本語で書き、Claude は文法 1 箇所と用語 2 箇所（「LSP の状態」→「言語サーバーの状態」、「チェック」→「ワークスペースの読み込み」）の指摘と文体の調整、事実の確認（同じ環境での保留時間: rust-analyzer 2.7〜80.6 秒、gopls 0.67 秒、pyright 0.25 秒、tsls 0.2 秒。「Rust は読み込みに時間を要する」は実測と一致）だけを行った。投稿は日本語の本文をそのまま置き、「The following is an English translation by Claude.」の 1 行の下に訳を引用ブロックで添える形（方針が勧める形。訳の言語は方針が指定していない）。issue #23331 への 1 行の返信も Claude が書いたものだが、開示の文には含めなかった（ユーザーの判断）。投稿した本文:

> ご指摘のとおり、当該ポリシーを読んでおりませんでした。申し訳ありません。issue および PR の文面は Claude が書いたものです。私はその和訳を確認し、英訳を Claude に行わせました。
>
> 本変更の動機は次のとおりです。Claude Code などの LSP クライアントから、言語サーバーの状態をより詳しく把握したいと考えています。特に、ワークスペースの読み込みに時間を要する Rust では、参照検索のようなワークスペース全体にわたる要求の結果が揃っているかを確認したいのです。また、LSP 本体に取り込まれることが望ましいと考えています。
>
> 本コメントを含め、以後の日本語の文章は私自身が書きます。ただし、私の文章は Claude にレビューさせています。

次: 応答待ち（催促は 2 週間後に 1 回だけ）。閉じられたら fork で維持するかをそのとき決める（本文の規則）
