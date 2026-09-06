# clangd の readiness の実測（M24）

ADR 0020 決定 C の M24。仕様 10 章の clangd の行は v0.1 から「信号なし」で、8.2 の 3 も clangd を「readiness の語彙を持たないサーバー」の例に使っている。しかし LLVM のソース（`clang-tools-extra/clangd/ClangdLSPServer.cpp`、D73218）は `window.workDoneProgress` を宣言したクライアントに背景索引の `$/progress` を送る。402 ファイルの被験体で測り、**信号はある**と確かめた。token `"backgroundIndexProgress"`、title "indexing" の begin → report（"N/402"）→ end で、begin〜end の間の `references` は **空応答から始まり件数が増え続ける部分応答**（0 → 17 → 117 → 217 → 316 → 400）である。仕様 10 章の行と 8.2 の 3 の例は事実として誤りで、訂正はユーザーの承認の後（ADR 0020 決定 F）。

一方、`compile_commands.json` がなければ背景索引は始まらず信号も出ない（`references` はずっと空）。開いている文書の `didChange` は 40〜80 ms のあいだ他ファイルからの問い合わせに古い答えを返し、その完了を示す信号はない。ディスク上の変更は `didChangeWatchedFiles` を送っても送らなくても取り込まれない（登録もしない）。

## 方法

- nixpkgs の clang-tools 21.1.8（`serverInfo.version` は "clangd version 21.1.8 linux x86_64-unknown-linux-gnu"。`flake.nix` の `servers`）。`clangd`（`--background-index` は既定で有効）。2026-09-06
- 被験体: `lib.h`（`int target();`）、`lib.cpp`（定義）、`f0.cpp`〜`f399.cpp`（各 30 関数と `target()` を 1 回呼ぶ関数。`#include "lib.h"` のみで標準ヘッダは使わない）、`compile_commands.json`（402 エントリ。`clang++ -c`）
- 道具: scratchpad の `lsp_probe.py` と、`didChange` の後に固定の間隔で要求を送る `order_probe.py`。クライアントは `window.workDoneProgress` と `workspace.didChangeWatchedFiles.dynamicRegistration` を宣言する
- 走行: (1) 起動して `lib.cpp` を開き `target` の `references` を送り続ける（5 ms 間隔でも）、(2) 索引の完了後に開いている `f0.cpp` に `didChange`（全文）で呼び出しを足し、0〜320 ms の間隔で要求を送る、(3) `g.cpp` を作って `didChangeWatchedFiles` Created、(4) 開いていない `f1.cpp` をディスクで書き換えて Changed、(5) `f2.cpp` を消して Deleted（(3)〜(5) は 10 秒待つ）、(6) (3) を通知なしで、(7) `compile_commands.json` なし、(8) `didOpen` なし
- 裏付けに LLVM の `ClangdLSPServer.cpp`（`onBackgroundIndexProgress`）を読んだ（コーパスの出典と同じ）

## 結果

### 語彙

| 信号                                                                  | 内容                                                                                                                                                                                    |
| --------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `InitializeResult.serverInfo`                                         | `{"name": "clangd", "version": "clangd version 21.1.8 linux x86_64-unknown-linux-gnu"}`。版が語彙に現れる（文字列の中）                                                                 |
| `window/workDoneProgress/create`（token `"backgroundIndexProgress"`） | 背景索引の待ち行列が空から非空になったときに 1 度                                                                                                                                       |
| `$/progress`（同 token）                                              | begin（title "indexing"、percentage 0）→ report（message "N/402"、percentage）→ end。最初の `didOpen` で `compile_commands.json` を見つけたときに始まる（`initialized` の直後ではない） |
| `client/registerCapability`                                           | **なし**。`workspace/didChangeWatchedFiles` を登録しない                                                                                                                                |
| エラー応答 `-32602` "trying to get AST for non-added document"        | 開いていない文書への要求                                                                                                                                                                |

health の信号はない。`window/logMessage` も出ない（ログは stderr）。

### 起動（走行 1）

| 時刻（秒）   | 出来事                                                                                    |
| ------------ | ----------------------------------------------------------------------------------------- |
| 0.014        | `initialize` 応答。`didOpen lib.cpp`                                                      |
| 0.017        | create → begin "indexing"。report "0/1" → "0/402" → …                                     |
| 0.022        | `references` の最初の応答が **0 件**（空配列。エラーではない）                            |
| 0.038〜0.087 | 5 ms 間隔の応答が **17 → 117 → 217 → 316 → 317 件**（索引が埋まるにつれて増える部分応答） |
| 0.089        | end                                                                                       |
| 0.19         | 以後 **400 件**（`f0`〜`f399`。完全）                                                     |

無言の嘘そのもの。観測者が begin〜end を `indexing` として保留すれば、最初の答えから完全になる。

### 変更の取り込み（走行 2〜6）

| 引き金（`ready` 後）                                           | 結果                                                                                                                                             |
| -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| 開いている `f0.cpp` に `didChange` で呼び出しを足す（走行 2）  | `didChange` の 0、5、10、20、40 ms 後の要求は **400 件（古い）**、80 ms 以降は 401 件。`$/progress` は出ない（動的索引の更新に完了の信号がない） |
| `g.cpp` を作って Created（走行 3）                             | 10 秒待っても 400 件のまま。`$/progress` も出ない                                                                                                |
| 開いていない `f1.cpp` をディスクで書き換えて Changed（走行 4） | 同じ                                                                                                                                             |
| `f2.cpp` を消して Deleted（走行 5）                            | 同じ（消したファイルの参照が残る）                                                                                                               |
| 同じ作成を通知しない（走行 6）                                 | 同じ                                                                                                                                             |

ディスク上の変更は再起動（または該当ファイルの `didOpen`）まで織り込まれない。`didChangeWatchedFiles` は登録せず、送っても効かない。

### 信号が出ない条件（走行 7、8）

| 条件                         | 結果                                                                                                              |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `compile_commands.json` なし | `$/progress` は出ない。`references` は **ずっと 0 件**（開いている文書の外は見えない）                            |
| `didOpen` なし               | `$/progress` は出ない（索引は最初の `didOpen` でデータベースを見つけて始まる）。`references` は `-32602` のエラー |

背景索引の信号は「データベースがあり、文書を開いた」ときにしか出ない。データベースがない場合、信号の不在と「まだ begin が来ていない」を観測者は区別できない（begin は `didOpen` の 2 ms 後に来る）。

## 写像（設計）と未決の点

- **識別**: `serverInfo.name` "clangd"。版は `serverInfo.version` の文字列（"clangd version 21.1.8 linux x86_64-unknown-linux-gnu"）をそのまま `TESTED_VERSIONS` に置く
- **readiness**: `initializing` から、token `"backgroundIndexProgress"` の begin で `indexing`、end で `ready`。`ready` 後の begin（待ち行列が再び非空になったとき）で `indexing`、end で `ready`
- **先読み**: しない。`didChange` の後の古い答えの窓（40〜80 ms）は完了の信号がなく、予測は ADR 0014 追補の決定 D の条件を満たさない
- **health**: 信号がなく `unknown`
- **coverage / freshness**: 7.1 / 7.2 を通した版に `coverage: {scope: "workspace", incomplete: {}}` を宣言する。`freshness` は宣言しない（`didChange` に窓があり、ディスク上の変更は取り込まれない）。実サーバーの結合テストは 7.1 と 7.2 だけを当て、7.3 は測定の記録にとどめる
- **未決（ユーザーの判断）**: `compile_commands.json` がないワークスペースでは begin が来ず、`initializing` の写像は横断リクエストを永久に保留する（クライアントのタイムアウトになる）。8.2 の 3 は「信号のないサーバーを `initializing` に留めるな」と言うが、clangd の信号は条件付きで、観測者には不在と未着の区別がつかない。選択肢は (a) `initializing` のまま（データベースがある通常の場合は正しく、ない場合はタイムアウト）、(b) 初期状態を `unknown` にする（データベースがある場合に `didOpen` と begin の 2 ms の隙間で空応答が漏れる）、(c) 観測者が `compile_commands.json` / `compile_flags.txt` の有無をファイルシステムで見る（Nextflow と同じ「サーバーの挙動の再現」だが、`--compile-commands-dir` や `.clangd` の CompileFlags を再現しきれない）。本 M では (a) で実装し、ADR 0020 の追補で決めてもらう
- **仕様の訂正（ユーザーの承認の後）**: 10 章の clangd の行を「背景索引の `$/progress`（title "indexing"）。begin〜end の間は空応答から増え続ける部分応答」に、8.2 の 3 の例を clangd 以外（pyrefly など信号のないサーバー）に
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: (a) 索引中の横断リクエストを待たせるか部分応答であることを示す。(b) `workspace/didChangeWatchedFiles` を登録して読み、ディスク上の変更を再索引する。(c) データベースがないときにそれを伝える通知

## コーパスへの反映

コーパスの「begin〜end の間が `indexing`、end 後が `ready`」はそのとおりで、Serena が無視している信号を lsp-det は読む。加えて「索引中の答えは空から増え続ける」「`didChange` の後に信号のない古い窓がある」「ディスク上の変更は取り込まれない」「データベースがなければ信号も出ない」を記す。
