# rust-analyzer の quiescent 実測（2026-08-29）

lsp-det 経由で実 rust-analyzer に接続し、`experimental/serverStatus` の `quiescent` が
どの操作で `false` に戻るかを測定した。ADR 0006 決定 3 が M2 の宿題としていた項目。

## 結論

| 操作                                  | `quiescent` の往復 | インデックス反映まで       |
| ------------------------------------- | ------------------ | -------------------------- |
| ソースファイルへのディスク書き込み    | **起きない**       | 0.02〜0.04 秒              |
| `didOpen` + `didChange`（インメモリ） | **起きない**       | 0.02 秒                    |
| `Cargo.toml` の変更                   | **起きる**         | 約 0.25 秒で `true` に復帰 |

通常編集では `quiescent` は動かない。`--gate-mode hold` でも編集→横断リクエストの連打は
保留されない。往復するのはワークスペース構成の変更（`Cargo.toml`、ブランチ切り替え等）だけで、
これは v0.1-design 4.3 が元から想定していた条件と一致する。

## 測定環境

- 対象ワークスペース: lsp-det 自身（ソース 5 ファイル、依存 4 クレート）
- rust-analyzer: rustup の `stable-x86_64-unknown-linux-gnu` コンポーネント
- 経路: 偽クライアント → lsp-det（`--adapter rust-analyzer`）→ rust-analyzer
- 偽クライアントは `experimental.serverStatusNotification` を宣言していない。lsp-det の
  capability 注入（設計 4.5）だけで通知が届いており、注入の実地検証も兼ねている
- 初回 `quiescent: true` 到達は 2.28〜2.30 秒

## 測定方法

1. `initialize` → `initialized` を送り、`quiescent: true` を待つ
2. 操作を加え、`experimental/serverStatus` の到着を監視する
3. インデックス反映は `workspace/symbol` に新しいシンボル名を問い合わせて判定する
   （ヒットすれば反映済み。250ms 間隔でポーリング）

## 測定結果

### ディスク書き込み

新しい `pub fn` を追記して 8 秒監視。`serverStatus` の到着は 0 件（2 回の独立した実行で再現）。

反映までの時間は 4 試行すべて**最初の問い合わせでヒット**した。

| 試行           | 書き込みからの経過 | 問い合わせ回数 |
| -------------- | ------------------ | -------------- |
| 即ポーリング a | 0.04 秒            | 1              |
| 即ポーリング b | 0.02 秒            | 1              |
| 8 秒待機後 a   | 8.02 秒（待機分）  | 1              |
| 8 秒待機後 b   | 8.02 秒（待機分）  | 1              |

`readiness` は全期間 `ready` のままで、かつ結果は最新だった。

### didOpen + didChange（インメモリ）

`serverStatus` の到着は 0 件。反映は 0.02 秒。

### Cargo.toml の変更（陽性対照）

コメント 1 行の追記で `quiescent: false` → `true` を観測。2 回とも往復し、
`false` から `true` までは 250ms / 260ms。復元時も同様に往復した。

この対照がなければ「往復しない」という観測は測定系の不備と区別できない。

## ソース上の裏付け

`quiescent` の実体は `is_fully_ready()`（`crates/rust-analyzer/src/reload.rs`）:

```rust
fn is_fully_ready(&self) -> bool {
    self.is_quiescent() && !self.prime_caches_queue.op_in_progress()
}
```

`is_quiescent()` が見るのは `vfs_done` とワークスペース取得系のキュー（`fetch_workspaces_queue`、
`fetch_build_data_queue`、`fetch_proc_macros_queue`）である。

- `vfs_done` は VFS の**一括ロード進捗**（`vfs::loader::Message::Progress`）でのみ更新される
  （`main_loop.rs`: `self.vfs_done = state == Progress::End;`）。個別ファイルの変更通知では動かない
- `prime_caches_queue.request_op` の呼び出しは 1 箇所だけで、`became_quiescent`（`false` から
  `true` へ移る辺）でのみ発火する（`main_loop.rs`）。`quiescent` が `false` にならない限り
  再プライミングも起きない

つまり通常編集は salsa の invalidation を起こすだけで、readiness の 2 つの入力
（VFS 一括ロード、ワークスペース取得）のどちらにも触れない。測定結果はこの構造と一致する。

## 設計への含意

- v0.1-design 4.3 の元の記述（再インデックスはブランチ切り替えや `Cargo.toml` 変更で起きる）が正しい。
  通常編集でも起きるとした ADR 0006 決定 3 の前提は誤り（→ ADR 0007）
- 平滑化・デバウンス等のフラップ対策は不要
- ただし所要時間は本ワークスペースの規模に依存する。大規模プロジェクトでは `Cargo.toml` 変更後の
  保留が 250ms では済まない。「往復しない条件」は構造由来なので規模に依らないが、
  「往復したときの長さ」は規模依存であり、この測定値を一般化してはならない

## 未測定の項目

- ブランチ切り替え（`git switch`）での往復と所要時間
- 大規模ワークスペースでの `Cargo.toml` 変更後の保留時間
- `rustup` のツールチェーン切り替え、`build.rs` の変更

## 起動直後の `quiescent` と field 追加案の実測（2026-09-09、提出前の準備 4）

`docs/upstream-submissions.md` の準備 4（rust-analyzer に `experimental/serverStatus` への field 追加案を用意する）のための追加測定。`quiescent` は「背景の仕事が進行中か」の答えであって、最初のロードの**前**は何も進行していないので自明に `true` になる。これを「ready」と読むクライアントは、いちばん大事な瞬間に読み違える。この「自明な静穏」が実際に送られるかを確かめた。

### 測定方法

`scripts/rust-analyzer/status-probe.py <rust-analyzer> [--empty]`。偽クライアントが stdio で rust-analyzer を起動し、`experimental.serverStatusNotification` を宣言して `initialize` → `initialized` を送り、`experimental/serverStatus` を時刻付きで全部記録する。`--empty` は Cargo.toml のない空のディレクトリ。時間で判定はしない。

- 素の版: nixpkgs の `2026-08-03`（flake の開発環境）
- field 版: fork `tagawa0525/rust-analyzer` の `server-status-readiness`（719b151b、上流 master 70d74f4 起点）を `scripts/upstream/build-rust-analyzer.sh` で組んだもの。`serverInfo.version` は `0.0.0 (719b151b70 2026-09-09)`

### 結果

| 条件                      | 素の版（`2026-08-03`）                                                                                                                 | field 版（`server-status-readiness`）                                                                                                                       |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1 クレートのプロジェクト  | 5 ms `{ok, quiescent: false}` → 1.7 s `{ok, quiescent: true}`                                                                          | 4 ms `{quiescent: false, readiness: initializing}` → 0.2 s `{quiescent: false, indexing}` → 0.54 s `{quiescent: true, ready}`                               |
| Cargo.toml のない空の場所 | 5 ms `{warning, quiescent: true, "Failed to discover workspace. …"}` → 6 ms `{error, quiescent: true, "… Failed to load workspaces."}` | 4 ms `{warning, quiescent: true, initializing, "Failed to discover workspace. …"}` → 5 ms `{error, quiescent: true, ready, "… Failed to load workspaces."}` |

2 回ずつ走らせて同じ並びだった（時刻は ±10 ms）。field 版のプロジェクトの `health` は `warning`（ソースビルドの rustc に `rust-src` がなく sysroot の読み込みに失敗する。`readiness` の並びには関係しない）。

### 読み取り

- **自明な静穏は実際に送られる**。空の場所では、最初の通知が `quiescent: true` で、しかもロードはまだ始まっていない。`GlobalState::run`（`main_loop.rs`）は `update_status_or_notify()` を `fetch_workspaces_queue.request_op("startup")` より**前**に呼ぶので、`health` が初期値（`ok`）と違えば、何もロードしていない状態の `quiescent: true` がそのまま出る。プロジェクトがあるときに出ないのは、初期の `last_reported_status` が `{ok, quiescent: true}` で計算結果と等しく、差分がないから送られないだけである。つまり素の版は「通知が来ないうちは ready」を前提にしていて、それは逆さまの前提である
- **要求の形がない**。`serverStatus` は通知だけなので、遅れて接続したクライアントや通知を取りこぼしたクライアントは次の通知まで何も知れない。field 追加案（案 A）もこれは埋めない。別通知案（案 B、`server-state`）は要求 `experimental/serverState` を持つ
- **field 版では 3 値が観測できる**。最初のロードは `initializing`（フェッチ中でワークスペースがまだ空）→ `indexing`（ワークスペースは入ったがビルドスクリプトとキャッシュのプライミングが進行中）→ `ready`。ロードの失敗は `health: error` で `readiness: ready`（失敗は決着している。仕様 6 章 5 項どおり health の軸）
- lsp-det の写像（`src/adapter/rust_analyzer.rs`）は field があればそれをサーバー自身の言葉として読み、なければ従来どおり `quiescent` から導く。受け入れ条件は `tests/upstream_dev.rs` の `rust_analyzer_reports_readiness_in_server_status`（素の版で失敗、field 版で通る）

### 一般化してはならない点

- 時刻はこのマシンと 1 クレートの fixture のもの。`initializing` → `indexing` → `ready` の並びは構造由来だが、各段の長さは規模依存
- 空の場所で最初の通知が `quiescent: true` になるのは `health` が初期値と違うからで、他の警告（設定エラー等）でも同じ経路で出る。プロジェクトがあれば最初の通知は `quiescent: false` である
