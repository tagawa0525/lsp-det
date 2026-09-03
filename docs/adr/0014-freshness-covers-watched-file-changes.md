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
