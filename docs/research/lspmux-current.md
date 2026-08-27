# lspmux（旧 ra-multiplex）現行版の調査

調査日: 2026-08-28。比較対象は `reference/ra-multiplex/`（GitHub 最終コミット `d01f84d "move repository"` = v0.2.6 相当）と `reference/lspmux/`（Codeberg 現行 HEAD `18861f9` = v0.3.0 + Unreleased 変更を含む）。いずれも浅い clone のためコミット履歴は 1 件のみで、差分は CHANGELOG とソースツリーの diff から特定した。

## 要約

- **`$/cancelRequest` の no-op 問題（チェックリスト #2）は現行版でも未修正**。通知は id 書き換えなしで素通しされるため、タグ付けされたリクエスト id と一致せずキャンセルは実質効かないまま。設計根拠は成立し続ける。
- **initialize 応答前のメッセージで handshake が bail する問題（チェックリスト #1）は修正済み**。ただし「無視して待つ」実装であり、pre-initialize 通知をクライアントへ転送はしない（作者自身が TODO/FIXME で不完全と認めている）。設計文書の出典表記は「lspmux（Unreleased）で bail は修正済みだが転送はされない」に更新すべき。
- **shutdown 横取り（偽 null 応答）は不変**。フレーミングも**完全パース + 再シリアライズ + `deny_unknown_fields` のまま**（`lsp/jsonrpc.rs`・`lsp/transport.rs` は両版でバイト単位一致）。lsp-det の「原文バイト転送」方針の根拠（4.5 節）も成立し続ける。

## 1. 旧版の既知問題の現状

### 1(a) `$/cancelRequest` — 未修正（no-op のまま）

現行 lspmux でも cancel を特別扱いするコードは存在しない。`grep -ri cancel` でヒットするのは設計メモのコメント 1 箇所のみ:

- `lspmux/src/lsp.rs:26`（`ra-multiplex/src/lsp.rs:26` と同一）: 「Cancel notifications - contains an `id` property again, so we could multiplex this like any other request」— 「できるはず」のまま実装されていない。

動作機序も旧版と同一:

- クライアント発リクエストは id が `client_id:<N>:n:<id>` 形式の文字列にタグ付けされてサーバーへ送られる（`lspmux/src/client.rs:385`、タグ実装は `lspmux/src/lsp/ext.rs:24`-35。旧版は `ra-multiplex/src/client.rs:369`）。
- 一方 `$/cancelRequest` は通知であり、汎用の Notification アーム（`lspmux/src/client.rs:422`、旧版 `ra-multiplex/src/client.rs:406`）で `params.id` を書き換えずに素通しされる。サーバー側にはタグ付き id しか存在しないため一致せず、キャンセルは no-op。

**結論: lsp-det 設計のチェックリスト #2 の根拠は現行版でも成立する。**

### 1(b) initialize 応答前の通知で handshake が bail — 修正済み（ただし通知は破棄）

CHANGELOG（`lspmux/CHANGELOG.md` の Unreleased / Fixed）に「avoid throwing an error when receiving a pre-initialize notification from server (for example progress notification)」と明記されている。

- 旧版 `ra-multiplex/src/instance.rs:525`: 最初のメッセージが initialize 応答でなければ `bail!("first server message was not initialize response")`。
- 現行版 `lspmux/src/instance.rs:718`-732: ループに変更され、initialize 応答が来るまで他のメッセージを `warn!` ログ付きで**無視**する。

ただし作者自身が不完全さを注記している:

- `lspmux/src/instance.rs:702`-709 の TODO: 仕様上 `window/showMessage`・`window/logMessage`・`telemetry/event`・`window/showMessageRequest`・`$/progress` は initialize 前に送られ得るので本来クライアントへ転送すべきだが、していない（サーバーが初期化中は無応答に見える）。
- `lspmux/src/instance.rs:710`-717 の FIXME: handshake 待機中ずっと `InstanceMap` のロックを保持しており、（kotlin-language-server のように initialize 応答までインデックスをブロックするサーバーだと）**全クライアントの新規接続がブロックされる**という別の構造問題も残る。

**結論: チェックリスト #1 の「bail する」という出典は古くなった。ただし「handshake 中の通知を正しく扱うのは落とし穴」という教訓自体は有効で、lspmux も転送はできていない。**

### 1(c) shutdown 横取り（偽 null 応答）— 不変

- `lspmux/src/client.rs:371`-382（旧版 `ra-multiplex/src/client.rs:355`-368 と実質同一）: クライアントの `shutdown` リクエストを横取りし、`ResponseSuccess::null(req.id)`（`lspmux/src/client.rs:378`）で偽の null 応答を返して当該クライアントの接続だけを閉じる。サーバーには `shutdown` を伝えない（他クライアントが接続中の可能性があるため。コメントで ra-multiplex issue #5 を参照）。

多重化プロキシとしては合理的な設計判断だが、「shutdown/exit を上流へ忠実に伝播する」lsp-det 4.6 節の方針との対比点として不変。

## 2. フレーミング・転送方式 — 変更なし

`src/lsp/jsonrpc.rs` と `src/lsp/transport.rs` は両版で **diff ゼロ（完全一致）**。

- 全メッセージを `serde_json::Value` ベースの構造体へ完全パースし、転送時に再シリアライズする方式のまま。`RawValue` 化などの変更はない。
- `deny_unknown_fields` も全メッセージ型に残存: `lspmux/src/lsp/jsonrpc.rs:21`（Request）、`:31`（Notification）、`:40`（ResponseError）、`:48`（ResponseSuccess）、`:68`（Error）。旧版 `ra-multiplex/src/lsp/jsonrpc.rs` の同一行番号と一致。
- `Message` は `#[serde(untagged)]` の enum（`lspmux/src/lsp/jsonrpc.rs:12`-19）なので、未知のトップレベルフィールドを含むメッセージは 4 バリアントすべてにマッチせずパース失敗する構造も不変。

**結論: lsp-det 4.5 節「ボディは原文バイトのまま転送」の根拠（完全パース + 再シリアライズの危険、`deny_unknown_fields` の実例）は現行版にもそのまま当てはまる。**

## 3. 新機能・構造変更の概要（v0.2.6 → 現行 HEAD）

CHANGELOG（`lspmux/CHANGELOG.md` の Unreleased / v0.3.0 / v0.2.6）とコード差分より。

| 区分 | 変更 | 根拠 |
| --- | --- | --- |
| 改名 | `ra-multiplex` → `lspmux`（v0.3.0）、Codeberg 移転・EUPL-1.2 化（v0.2.6）。環境変数も `RA_MUX_SERVER` → `LSPMUX_SERVER` | `lspmux/CHANGELOG.md`、`lspmux/src/main.rs:24` |
| 新機能 | `sync` サブコマンド: 全クライアントの開いているファイルをディスクから読み直し、全文 `textDocument/didChange` をサーバーへ送って状態を再同期（複数クライアントの競合編集で desync した時の復旧手段） | `lspmux/src/main.rs:58`-67、`lspmux/src/client.rs:180`-219、`lspmux/src/instance.rs:442`-488 |
| 修正 | `InitializeParams.workspaceFolders` の null を許容（neovim 対応）。null → 空 vec、欠落 → None にマップするカスタムデシリアライザ | `lspmux/src/lsp.rs:83`-105 |
| 修正 | workspace root の同一性判定を dev/inode 比較に変更（大文字小文字非区別 FS 対応）。`WorkspaceRoot` 型を新設 | `lspmux/src/instance.rs:38`-105 |
| 修正 | `InitializeParams.processId` を lspmux サーバー自身の PID に差し替え。仕様通りクライアント PID を監視するサーバーが、最初のクライアント終了と同時に死ぬのを防止 | `lspmux/src/instance.rs:639`-643 |
| 修正 | 最初のクライアントを spawn 時点で clients マップに登録するよう構造変更。handshake 直後の `workspace/configuration` 等の早期サーバーリクエストが「クライアント不在」で落ちる問題を修正（deno 起動時のハング対策） | `lspmux/src/instance.rs:650`-658、`lspmux/src/client.rs:249`-252 |
| 修正 | `client/registerCapability` のキャッシュ更新をブロードキャスト前に移動（新規接続クライアントへの capability 再生との競合対策） | `lspmux/src/instance.rs:958`-966 |
| 変更 | `pass_environment` が glob + 否定フィルタ対応（既定 `"*"` = 全部通す） | `lspmux/src/config.rs:86`-121、`lspmux/src/proxy.rs:18`-41、`lspmux/defaults.toml` |
| 変更 | stdout_task のロック粒度を細分化（clients ロックをメッセージ処理全体で保持しない）。遅いクライアントが全体をブロックする問題の緩和 | `lspmux/src/instance.rs:860`-863 ほか |
| 変更 | 依存削減（`time` crate 削除 → 単調時計 `elapsed_seconds`、clap/tracing-subscriber の default-features off）、`status` の出力簡略化（`-v` で詳細） | `lspmux/Cargo.toml`、`lspmux/src/instance.rs:172`-176、`lspmux/src/main.rs:44`-47 |

構造の骨格（client.rs / instance.rs / proxy.rs / server.rs の役割分担、id タグ方式、TCP 経由の proxy/server 分離）は不変。`server.rs` は diff ゼロ。

## 4. 結論: lsp-det 設計文書（docs/v0.1-design.md 7 章）への影響

| チェックリスト | 現行版での状態 | 設計根拠の成否 |
| --- | --- | --- |
| #1 initialize 応答前の `window/showMessage` 等で handshake が壊れる（ra-multiplex #89） | **修正済み**（`lspmux/src/instance.rs:718`-732 で無視ループ化）。ただし通知の転送はせず破棄。作者も TODO/FIXME で不完全と明記 | **出典の更新が必要**。「bail する実例」としては過去形にする。一方「handshake 中の通知を許容するパーサ構造」という lsp-det の対処方針は、lspmux の現行実装（破棄）より一歩先を行くもので、方針自体は有効 |
| #2 `$/cancelRequest` 素通しで no-op | **未修正**。cancel 特別扱いコードなし、id タグとの不整合も不変（`lspmux/src/client.rs:385` vs `:422`） | **成立し続ける**。修正不要 |
| （4.5 節）完全パース + 再シリアライズ + `deny_unknown_fields` の危険 | **不変**（`lsp/jsonrpc.rs`・`lsp/transport.rs` は両版で完全一致） | **成立し続ける** |
| （参考）shutdown 横取り | **不変**（`lspmux/src/client.rs:371`-382） | 対比点として不変 |

推奨アクション: v0.1-design.md の 7 章 #1 の出典を「ra-multiplex #89（lspmux Unreleased で bail は修正済み。ただし pre-initialize 通知は転送されず破棄される — `lspmux/src/instance.rs:702` の TODO 参照）」の形に補足更新する。#2 と 4.5 節の根拠は現行版の実コードで再確認済みであり、変更不要。
