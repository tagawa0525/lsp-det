# Expert（Elixir）の readiness の実測（M10）

ADR 0019 決定 F の M10。コーパス（[readiness-vocabulary-corpus.md](readiness-vocabulary-corpus.md)）の Elixir の行は Serena の実装から「ビルドは `didOpen` まで始まらない」と見ていた。実測では **エンジンの起動とビルドは `initialized` で始まり、`didOpen` は要らない**。時間なしで写像できる。一方、`workspace/didChangeWatchedFiles` は `**/*.{ex,exs}` を動的登録しておきながら Created も Changed も取り込まず、`freshness.fileChanges` は空になる。

## 方法

- Expert 0.1.9（[elixir-lang/expert](https://github.com/elixir-lang/expert) の release の `expert_linux_amd64`。静的リンクの burrito バイナリで NixOS でそのまま動く。`--stdio` が必須）。Elixir 1.18.5、Erlang/OTP 28（nixpkgs）。`erl` と `elixir` が PATH に要る（ないと "Failed to find an erl executable, shutting down" で終了する）。2026-09-06
- 被験体: `mix new fixture` に `lib/a.ex`（`defmodule A do def target, do: 1 end`）と `lib/b.ex`（`A.target()` を 1 回呼ぶ）。`git init` だけ
- 道具: scratchpad の `lsp_probe.py`（Metals と同じ）。`initializationOptions` は Serena と同じ `{mix_env: "dev", mix_target: "host"}`
- 4 走行: (1) `--stdio` なし（起動しない。記録のみ）、(2) コールドキャッシュ、`didOpen lib/a.ex`、`ready` 後に `lib/c.ex` を作って Created、(3) `didOpen` なし、(4) ウォーム、`ready` 後に開いていない `lib/b.ex` に参照を足して Changed

## 結果

### 語彙

| 信号                                                                | 内容                                                                                                                                                                                                                                                                                                |
| ------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `$/progress`（token は整数、`window/workDoneProgress/create` の後） | title は "[project] Starting engine node"、"[project] Preparing engine"（end の message "Engine is ready"）、"Building project"、"Indexing source code"（percentage 付き、end の message "Completed in N ms"）。リクエスト処理の "Finding Completion Candidates"、"Loading search index" も同じ機構 |
| `window/logMessage`                                                 | "Server initialized, registering capabilities"、"[project] Starting project"、"Compiled project in N ms"、"Received request textDocument/references before engine for project was initialized. Ignoring."（type 3）。`erl` がなければ type 1 の "Failed to find an erl executable, shutting down"   |
| `client/registerCapability`                                         | `workspace/didChangeWatchedFiles` を `**/*.{ex,exs}` と `**/mix.lock` で動的登録する                                                                                                                                                                                                                |

`serverInfo` は `{"name": "Expert", "version": "0.1.9"}`。`capabilities.experimental` はない。health の信号はない。

### 時系列（走行 2、コールドキャッシュ。エンジンのビルドを含む）

| 時刻（秒）     | 出来事                                                                                                                                            |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0.118          | `initialize` 応答。`initialized` の直後に "[fixture] Starting project"（`didOpen` の有無に関係なく。走行 3 で確認）                               |
| 0.126〜0.133   | "Starting engine node" と "Preparing engine" の begin                                                                                             |
| 0.127〜24.2    | `references` は**空配列を 13 回**返す。同時にログ "Received request textDocument/references before engine for fixture was initialized. Ignoring." |
| 25.656         | "Preparing engine" end（"Engine is ready"）                                                                                                       |
| 26.249         | "Starting engine node" end（"Engine node started"）                                                                                               |
| 26.25〜27.25   | **隙間**: 未完了トークン 0、`references` は空配列                                                                                                 |
| 27.255〜27.342 | "Building fixture" begin → end                                                                                                                    |
| 27.410〜27.421 | "Indexing source code" begin → end                                                                                                                |
| 28.339         | `references` が初めて `b.ex` の 1 件を返す                                                                                                        |

ウォームキャッシュ（走行 3、4）ではエンジンの起動が 1.4 秒、隙間は 1.0 秒（1.4 → 2.4 秒）、"Indexing source code" の end が 2.6 秒。同じ被験体を再び開くと（走行 6、lsp-det 越し）、"Building" の後は "Loading search index"（保存された索引の読み込み）だけで **"Indexing source code" は来ない**。索引の完了は 2 つの title のどちらかの end。

**エンジンの初期化前の要求に、ログで「無視した」と言いながら空配列の成功応答を返す。** 空応答の嘘の最も分かりやすい例で、`initializing` の間の保留が直接効く。

### 監視対象の変更は取り込まない（走行 2、4）

| 引き金（`ready` 後）                                     | 45 秒以内の信号 | 結果                       |
| -------------------------------------------------------- | --------------- | -------------------------- |
| `lib/c.ex`（参照 1 つ）を作って Created（走行 2）        | なし            | `references` は 1 件のまま |
| 開いていない `lib/b.ex` に参照を足して Changed（走行 4） | なし            | `references` は 1 件のまま |

`**/*.{ex,exs}` を動的登録しているが、通知を受けてもビルドも索引の更新も起きない。開いている文書の `didChange` は 7.3 の 1 の実サーバーテストで確かめる。

### 開いていないファイルからの問い合わせ（走行 3）

`didOpen` を送らずに `lib/a.ex` の位置で `references` を問うと、索引の後も空配列（60 回）。問い合わせ元の文書は開いていなければならない。これは readiness ではなくリクエスト単位の挙動で、写像には関係しない（準拠テストは開いてから問う）。

## 写像（設計）

- **readiness**: `initialize` 直後は `initializing`。"… Starting engine node"、"… Preparing engine"（後方一致）、"Building "（前方一致）、"Indexing source code"、"Loading search index" の begin で `indexing`。ready の条件は「未完了トークンが 0」かつ「直近に end したトークンが索引の段階（"Indexing source code" または "Loading search index"）」。エンジンの起動とビルドの間の 1 秒の隙間（直近の end が "Starting engine node"）で `ready` を名乗らない。"Finding Completion Candidates" はリクエスト処理で写さない。走行 6 で "Loading search index" を無視していたため lsp-det が `indexing` に留まり続け、実サーバーテストが 4 件とも 30 秒で失敗した
- **先読みはしない**。監視対象の変更を Expert が取り込まないので、取り込みの完了信号がなく、先読みすると永久に保留する（ADR 0014 追補 決定 D の条件を満たさない）
- **health**: 信号がなく `unknown`
- **coverage / freshness**: 7.2 と 7.3 の 1 の実サーバーテストで決める。`fileChanges` は空

## コーパスへの反映

Elixir の行の「ビルドが `didOpen` まで始まらない」は Serena が古い版（v0.1.0-rc.6）で見た挙動か、Serena 側の都合で、0.1.9 では `initialized` で始まる。疑問は「監視対象の変更を取り込まない」に置き換わる。
