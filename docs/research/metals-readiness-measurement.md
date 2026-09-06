# Metals の readiness の実測（M9）

ADR 0019 決定 F の M9。コーパス（[readiness-vocabulary-corpus.md](readiness-vocabulary-corpus.md)）で唯一「時間でしか終わりを言えない」と見た Metals を、実サーバーで測った。結論は**時間なしで写像できる**。Serena の静穏期間（3 秒）が埋めていたのは、トークンの隙間を「未完了トークン 0 = ready」と読む誤りであり、規則を「最初の "Indexing" の end まで ready を言わない」と「build ファイルとソースの変更の通知から再インデックスを先読みする」に変えれば消える。

## 方法

- Metals 1.6.8（nixpkgs）、scala-cli 1.16.0（ビルドツール兼 BSP サーバー）、OpenJDK 21.0.12。2026-09-06
- 被験体: `git init` だけしたディレクトリに `project.scala`（`//> using scala 3.3.4`）、`A.scala`（`object A { def target: Int = 1 }`）、`B.scala`（`A.target` を 1 回参照）
- 道具: scratchpad の `lsp_probe.py`（汎用の probe。`initialize` → `initialized` → `didOpen A.scala` の後、`textDocument/references` を 2 秒ごとに問い、サーバーからの全通知と、サーバー → クライアントの要求（`window/workDoneProgress/create`、`workspace/configuration`、`client/registerCapability`、`window/showMessageRequest`）への応答を時刻付きで記録する）。クライアントは `window.workDoneProgress` を宣言する
- 5 走行: (1) コールドキャッシュ、`statusBarProvider: "on"`、(2) ウォーム、`ready` 後に `C.scala`（参照 1 つ）を作り `didChangeWatchedFiles`（Created）、(3) ウォーム、`initializationOptions` に `statusBarProvider` なし（既定のクライアント）、(4) ウォーム、`ready` 後に `project.scala` に依存を足して Changed、(5) 壊れたビルド定義（`//> using scala 9.9.9`）

## 結果

### 語彙

| 信号                                                                 | 内容                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| -------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `$/progress`（token は UUID、`window/workDoneProgress/create` の後） | title は "scala-cli bspConfig"（BSP の設定生成。ビルドツールにより変わる）、"Importing build"、"Indexing"、"Compiling <build target>"（percentage 付き）、"Loading presentation compiler"。begin / report / end で閉じる。**ビルドの取り込みは Importing build → Indexing の順で、初回もその後も必ず "Indexing" が最後に end する**                                                                                                                                                                                                                                                                                      |
| `metals/status`（`statusType: "module"`）                            | `initializationOptions` に関係なく届く。`{text, level: "info" /                                                                                                                                                                                                                                                                     "warn" / "error", show, tooltip, command, statusType}`。起動直後に `"importing..."`、取り込み後にビルドターゲット名と `tooltip: "No errors for the build target."`、ビルドターゲットが見つからないと `text: "no target", level: "error", tooltip: "No build target for file found."` |
| `metals/status`（`statusType: "metals"`）                            | `statusBarProvider: "on"` のときだけ。`" Indexing complete!"`（先頭にアイコン。`Indexer.scala` の文字列で人間向け）、その後 `hide: true` の空文字。**再インポートの直前にも出る**（走行 4 で 2.847 秒: 初回の Indexing の end の直後、変更による再取り込みの前）ので、完了の信号として信用できない                                                                                                                                                                                                                                                                                                                       |
| `window/logMessage`                                                  | 起動ログ（type 4）。壊れたビルド定義では type 2 の `"scala-cli 1 : Empty build targets. ..."`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `client/registerCapability`                                          | `workspace/didChangeWatchedFiles` を動的登録する（走行 1〜5 すべて、1.6 秒前後）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |

`serverInfo` は `{"name": "Metals", "version": "1.6.8"}`。`capabilities.experimental` は `{"rangeHoverProvider": true}` だけで、`experimental/serverStatus` はない。

### 時系列（走行 1、コールドキャッシュ）

| 時刻（秒）     | 出来事                                                                                                                   |
| -------------- | ------------------------------------------------------------------------------------------------------------------------ |
| 0.968          | `initialize` 応答                                                                                                        |
| 1.355          | `metals/status` "importing..."（module）                                                                                 |
| 4.4〜15.2      | `references` は**空配列**（6 回）。この間 `$/progress` は 1 本も開いていない（BSP サーバーの起動と依存の取得はログだけ） |
| 5.233〜5.246   | "scala-cli bspConfig" begin → end（0.013 秒）                                                                            |
| 15.313〜15.394 | "Importing build" begin → end                                                                                            |
| 15.395         | "Indexing" begin                                                                                                         |
| 15.531〜16.494 | "Compiling fixture_…" begin → report（5 % 刻み）→ end                                                                    |
| 17.287         | `references` が初めて `B.scala` の 1 件を返す（"Indexing" はまだ open）                                                  |
| 20.387         | "Indexing" end。直後に `metals/status` " Indexing complete!"（metals）                                                   |
| 20.393         | "Loading presentation compiler" begin → end（1 ms）                                                                      |
| 20.473         | `metals/status` ビルドターゲット名 + "No errors for the build target."（module、info）                                   |

**隙間**: "scala-cli bspConfig" の end（5.246）から "Importing build" の begin（15.313）まで 10 秒、取り込みの途中なのに未完了トークンが 0 になる。ウォームキャッシュ（走行 2〜4）ではこの隙間は "importing..." から "Importing build" までの 1 秒前後（トークンなし）に縮む。

### 再インデックス

| 引き金（`ready` 後）                                                              | 信号                                                                                                                                                                                                                | 結果                                                                                            |
| --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `C.scala` を作って Created（走行 2、2.606 秒）                                    | 0.145 秒後に "Compiling" begin → 初回の "Indexing" end（2.951）→ "Loading presentation compiler" → "Compiling" end / 再 begin（3.080）→ end（3.129）→ "Importing build"（3.133〜3.143）→ "Indexing"（3.144〜3.816） | 3.816 の後の `references` が `B.scala` と `C.scala` の 2 件（4.620 秒）。Created は織り込まれる |
| `B.scala`（開いていない）に参照を足して Changed（走行 6、lsp-det 越し、2.288 秒） | 0.07 秒後に "Compiling" begin → end。**取り込みの round（Importing build → Indexing）は来ない**                                                                                                                     | 走行 7（直、下記）で反映の時点を測る                                                            |
| `project.scala` に依存を足して Changed（走行 4、2.704 秒）                        | 初回の "Indexing" end（2.846）→ " Indexing complete!"（2.847。誤り）→ **0.33 秒の隙間** → "Importing build"（3.179〜3.207）→ "Indexing"（3.207〜3.929）と "Compiling"（3.215〜3.609）                               | 再取り込み中の 0.33 秒、未完了トークンは 0                                                      |

通知から最初のトークンの begin までに 0.15〜0.33 秒の隙間がある。

### コンパイル後の索引の更新には信号がない（走行 7、8）

開いていない `B.scala` に参照を足して Changed を送り、references を 0.5 秒（走行 7）と 0.1 秒（走行 8）ごとに問うた。

| 時刻（走行 8、秒） | 出来事                                                                             |
| ------------------ | ---------------------------------------------------------------------------------- |
| 2.197              | `B.scala` を書き換え、Changed を送る                                               |
| 2.284              | "Compiling" begin                                                                  |
| 2.315              | references → **0 件**（変更前は 1 件。変更されたファイルの結果を Metals が落とす） |
| 2.352              | "Compiling" end                                                                    |
| 2.454              | references → 2 件（新しい答え）                                                    |

"Compiling" の end の後、semanticdb の索引の更新が終わるまでに 0.1 秒の窓があり、その間の references は 0 件（空の成功応答）を返す。Metals のソース（`ForwardingMetalsBuildClient.buildTaskFinish`）では、compile の終了で診断と module status を更新してから `didCompile` で索引を更新する順で、索引の更新の完了を伝える通知はない。走行 6 では lsp-det 越しに同じ操作をし、"Compiling" の end で `ready` に戻した直後の 7.3 の 2 の実サーバーテストが 0 件を受け取って失敗した。

したがって **`freshness.fileChanges` には Changed / Created / Deleted のどれも入れられない**（`fileChanges: []`。`didChange` は presentation compiler が織り込み、7.3 の 1 は通る）。先読み（Changed で `indexing`、"Compiling" の end で `ready`）は、compile の間の空応答を止める分だけ効くので残す。

### 壊れたビルド定義（走行 5）

"scala-cli bspConfig" → "Importing build"（その間に type 2 のログ "Empty build targets"）→ "Indexing" → end → " Indexing complete!" → **`metals/status {text: "no target", level: "error", tooltip: "No build target for file found."}`**（module）。以後 `references` は 60 秒間ずっと空配列。readiness の信号は正常時と同じ形で進むので、`health` を `metals/status` の `level` から取らないと「壊れたサーバーの成功風応答」になる。

### 既定のクライアント（走行 3）

`statusBarProvider` を渡さなくても `$/progress` と module の `metals/status` は届く。届かないのは metals の `metals/status`（" Indexing complete!"）だけで、写像には要らない。

## 写像（設計）

- **readiness**: `initialize` 直後は `initializing`。"Importing build"、"Indexing"、"Compiling "（前方一致）、"* bspConfig"（後方一致）の begin で `indexing`。ready の条件は「未完了トークンが 0」に加えて、直近に end したトークンの種類で決める。**初回の取り込みは "Indexing" の end で初めて完了**とし（それまでの隙間で `ready` を名乗らない）、"Importing build" と "bspConfig" の end 単独では `ready` にしない（数 ms 後に "Indexing" が続く）。**取り込みの後は "Compiling" の end も `ready`** にする。ソースの変更は "Compiling" だけを走らせ、取り込みの round は来ない（走行 6: 直近の end が "Indexing" であることを条件にすると永久に保留し、lsp-det 越しの probe で 31 件が `shutdown` まで保留された）。"Loading presentation compiler" は readiness に写さない（1 ms で閉じるリクエスト処理相当）
- **先読み**（ADR 0014 追補 決定 D と同じ）: クライアントの `workspace/didChangeWatchedFiles` を変更の種類で分ける。ソース（`.scala` / `.sc`）の Changed は次の "Compiling"（または "Indexing"）の end で戻す。ソースの Created / Deleted と build ファイル（`project.scala`、`*.sbt`、`build.mill` / `build.sc`）の変更はビルドを変えるので、次の "Indexing" の end でだけ戻す（走行 2: Compiling → Importing build → Indexing の後に新しい答え）。通知から最初の begin までの 0.15〜0.33 秒の隙間を埋める
- **health**: `metals/status`（module）の `level` から。`error` → `error`（message は `tooltip` か `text`）、`warn` → `warning`、`info` → `ok`。最初の module status までは `unknown`
- **時間は使わない**。Serena の静穏期間は要らない
- **coverage / freshness**: 7.2（通過）と 7.3 の 1（通過。`didChange` は presentation compiler が織り込む）から `coverage: {scope: "workspace", incomplete: {…}}` と `freshness: {fileChanges: []}`。`workspace/symbol` の上限は 7.2 の 2 で測る。監視対象の変更は上の窓のため入れない

## コーパスへの反映

コーパスの Metals の行は「idle の信号がないので `unknown` に倒すべき」としていたが、実測では "Indexing" の end が取り込みの終端を毎回示し、隙間は規則で越えられる。「時間でしか終わりを言えない」は誤りで、正しくは「未完了トークン 0 を ready と読むと隙間で嘘になる」。`metals/status`（metals）の " Indexing complete!" は再取り込みの前にも出るので、文書どおり actionable でない。
