# ADR 0014: `freshness` の対象に、受信した `workspace/didChangeWatchedFiles` を加える

- 日付: 2026-09-04
- 状態: 採用
- 関連: [ADR 0003](0003-extension-s-zero-based.md) 決定 3（鮮度は契約文言で保証する）、[ADR 0006](0006-external-review-fixes.md) 決定 2（freshness のテストはクロスファイル必須）、[ADR 0013](0013-coverage-instead-of-completeness.md)、[research/disk-edit-propagation-measurement.md](../research/disk-edit-propagation-measurement.md)、[research/serena-processing-around-lsp.md](../research/serena-processing-around-lsp.md) 2 章

## 経緯

仕様 6 章 2 項の `freshness` は「`ready` のとき、受信済みの `textDocument/didChange` をすべて織り込んでいる」と約束する。これはエディタの編集経路（メモリ上の変更を `didChange` で伝える）だけを対象にしている。

コーディングエージェントはディスク上でファイルを書き換える。LSP でその変更をサーバーに伝える手段は `workspace/didChangeWatchedFiles`（クライアントがファイルを監視し「これらが変わった」と知らせる）で、サーバーは `client/registerCapability` で見てほしい glob を登録する。Serena はツールのたびに全ソースファイルの mtime を走査してこの通知を送り（[research/serena-processing-around-lsp.md](../research/serena-processing-around-lsp.md) 2 章）、その理由を「外部の編集は warm なサーバーに見えず、古いインデックスから答える」と書いている。つまり「編集を織り込まない応答」という無言の嘘のうち、ディスク上の編集に由来するものが、今の `freshness` の外にある。

4 サーバーで測った（research/disk-edit-propagation-measurement.md）。`didChangeWatchedFiles` を受ければ 4 サーバーとも次の応答から織り込む。非同期に再インデックスする rust-analyzer（新規ファイル）と pyright（再列挙）は、lsp-det が既に読んでいる信号（`quiescent: false`、"Found N source files" の再発行）で `indexing` に戻るので、`ready` の後の応答が織り込んでいるという形で約束できる。

## 決定

### A. 6 章 2 項の対象に、受信した `workspace/didChangeWatchedFiles` を加える

> **鮮度**（`freshness` 宣言時）: `readiness` が `"ready"` かつ `health` が `"error"` でないとき、それまでに受信した `textDocument/didChange` と `workspace/didChangeWatchedFiles` はすべて織り込み済みでなければならない。

約束するのは「知らされた変更」までである。サーバーが自前のファイル監視で拾った変更は、観測者（lsp-det）には何も通らないので検証できず、約束に入れない。サーバー本体が本プロトコルを自ら実装する場合に自前の監視を含めて約束するかは、そのサーバーの判断であり、仕様は求めない。

### B. 7.3 に第 2 のテストを足す

1. （従来）ファイル A への `didChange` の後、`ready` で B 起点の横断問い合わせが A の変更を反映している
2. （追加）ファイル A をディスク上で変更し `didChangeWatchedFiles`（`Changed`）を送った後、および新しいファイル C を作り `didChangeWatchedFiles`（`Created`）を送った後、`ready` で B 起点の横断問い合わせがそれぞれの変更を反映している

どちらも A・C を開かずに行う（開くと `didOpen` の経路になり、ディスクの変更を検証しない）。`Deleted` は「古い参照が残る」方向の嘘で、実害の報告がないので今は含めない。

### C. 観測者の宣言

観測者が `freshness` を宣言する条件（8.2 の 5）は変わらず、7.3 の 2 つのテストを当該版に当てて通った場合に限る。4 サーバーの写像は、実装の段で第 2 のテストを通してから宣言を保つ。

### 却下した案

| 案                                                               | 却下理由                                                                                                              |
| ---------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| サーバー自身のファイル監視で拾った変更も約束に含める             | 観測者には見えず、テストできない。rust-analyzer と tsls では真、gopls と pyright では偽で、観測者に区別する手段がない |
| 上記を「サーバー本体が宣言する場合の追加」として非規範で書く     | 実装が現れてから書けばよい。今書くと検証できない文が仕様に入る                                                        |
| `Deleted` もテストに含める                                       | 「古い参照が残る」方向で、削除とリネームの安全性に関わる実害の報告がまだない。必要になったら足す                      |
| クライアントが通知を送らない問題（Claude Code）をこの ADR で扱う | 仕様の約束と、クライアントの欠落の代行は別の主題。代行は ADR 0015                                                     |

## 影響

### 仕様（docs/spec/server-state.md）

- 6 章 2 項の文と、7.3 の 2 項目化
- 10 章の表の `freshness` は、実装の段で第 2 のテストを通した版に更新する（通るまでは現状の記述のまま）

### 実装（ADR 0015 の後にまとめて）

- `tests/conformance.rs` に 7.3 の第 2 のテスト（偽上流と実サーバー 4 種）。偽上流に `didChangeWatchedFiles` を受けて再インデックスの信号を出す振る舞いを足す
- 写像の変更は不要の見込み（再インデックスの信号は既に読んでいる）。実サーバーで通らなければそのとき写像を直す

### クライアント

Serena は既に通知を送るので、この約束の中に入る。Claude Code は送らないので、ADR 0015 の代行で通知に変える。

## 追補（2026-09-06）: 実装で分かった Created / Deleted の窓と、写像ごとの扱い

第 2 のテストを実サーバーに当てたところ、Changed は 4 サーバーとも通り、Created は pyright と typescript-language-server が通らなかった（[research/disk-edit-propagation-measurement.md](../research/disk-edit-propagation-measurement.md) の追記）。通知を受けたサーバーは新しいファイルを非同期に取り込み、その開始を伝える信号は、通知の直後に届いた問い合わせより後に来る。観測者はその瞬間 `ready` と言うしかない。

サーバーごとに、通知の後に完了の信号が**必ず**来るかを測った。

| サーバー      | Created / Deleted の後の完了の信号                                                                   | 信号が来ない場合                                                                                      |
| ------------- | ---------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| rust-analyzer | ワークスペース内の `.rs` と Cargo のファイルなら、crate に入らないものでも `quiescent: false → true` | 監視の対象でないファイル（見込み。監視の登録は `**/*.rs`、`Cargo.{toml,lock}`、`rust-analyzer.toml`） |
| gopls         | 同期的に取り込む（信号は要らない）                                                                   | —                                                                                                     |
| pyright       | 列挙する集合が変わるときだけ "Found N source files"                                                  | 除外されたファイル（`**/.*`、`pyrightconfig.json` の `exclude`）、`.py` 以外                          |
| tsls          | なし。TypeScript の再帰ディレクトリ監視が Linux では 1 秒のタイマーで動く                            | 常に                                                                                                  |

### 決定 D. クライアントの通知を先読みの引き金にしてよいのは、完了の信号が測定で確かめられた写像だけ

写像は、クライアントから上流へ向かう `workspace/didChangeWatchedFiles` を観測してよい（`Mapping::observe_client`）。観測した通知から「サーバーはこれから再インデックスする」と先読みして `readiness` を `indexing` にしてよいのは、その通知に対してサーバーが完了の信号を必ず出すことが測定で確かめられた場合に限る。信号が来なければ `indexing` のまま止まり、時計を持たない lsp-det には戻る手段がないからである。

- rust-analyzer: Created / Deleted のうち、rust-analyzer 自身が監視を登録する glob（`**/*.rs`、`**/Cargo.toml`、`**/Cargo.lock`、`**/rust-analyzer.toml`）に一致するファイルの通知で `indexing` にし、`quiescent: true` で `ready` に戻す。Changed は先読みしない（信号が来ない。送信中の要求は rust-analyzer が -32801 で拒み、次の要求は正しい）
- gopls: 先読みしない（要らない）
- pyright と typescript-language-server: 先読みしない（信号が来ない場合がある）

### pyright と typescript-language-server の扱い

Created / Deleted の直後の窓（pyright は約 0.04 秒、tsls は約 1 秒）は観測者に見えず、埋める手段がない。この事実をどう宣言に表すかは、宣言の形そのものを見直す [ADR 0016](0016-declaration-shape.md) で決める（`freshness` は織り込む `FileChangeType` の一覧になり、この 2 つは `["Changed"]` を宣言する）。`didChange` と Changed では鮮度が成り立っていることは research に記録してある。上流への働きかけ（`docs/upstream-submissions.md`）: pyright には列挙の開始をログに出す（または本プロトコルを話す）変更、tsls には `useClientFileWatcher` で Created が同期になるかの測定を経た提案。
