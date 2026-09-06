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

| 引き金（`ready` 後）                                       | 信号                                                                                                                                                                                                                | 結果                                                                                            |
| ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `C.scala` を作って Created（走行 2、2.606 秒）             | 0.145 秒後に "Compiling" begin → 初回の "Indexing" end（2.951）→ "Loading presentation compiler" → "Compiling" end / 再 begin（3.080）→ end（3.129）→ "Importing build"（3.133〜3.143）→ "Indexing"（3.144〜3.816） | 3.816 の後の `references` が `B.scala` と `C.scala` の 2 件（4.620 秒）。Created は織り込まれる |
| `project.scala` に依存を足して Changed（走行 4、2.704 秒） | 初回の "Indexing" end（2.846）→ " Indexing complete!"（2.847。誤り）→ **0.33 秒の隙間** → "Importing build"（3.179〜3.207）→ "Indexing"（3.207〜3.929）と "Compiling"（3.215〜3.609）                               | 再取り込み中の 0.33 秒、未完了トークンは 0                                                      |

通知から最初のトークンの begin までに 0.15〜0.33 秒の隙間がある。その間の問い合わせが古い答えになるかは測っていない（7.3 の 3 の実サーバーテストが確かめる）。

### 壊れたビルド定義（走行 5）

"scala-cli bspConfig" → "Importing build"（その間に type 2 のログ "Empty build targets"）→ "Indexing" → end → " Indexing complete!" → **`metals/status {text: "no target", level: "error", tooltip: "No build target for file found."}`**（module）。以後 `references` は 60 秒間ずっと空配列。readiness の信号は正常時と同じ形で進むので、`health` を `metals/status` の `level` から取らないと「壊れたサーバーの成功風応答」になる。

### 既定のクライアント（走行 3）

`statusBarProvider` を渡さなくても `$/progress` と module の `metals/status` は届く。届かないのは metals の `metals/status`（" Indexing complete!"）だけで、写像には要らない。

## 写像（設計）

- **readiness**: `initialize` 直後は `initializing`。"Importing build"、"Indexing"、"Compiling "（前方一致）、"* bspConfig"（後方一致）の begin で `indexing`。**ready の条件は「未完了トークンが 0」かつ「直近に end したトークンが "Indexing"」**。これで初回の隙間（"Indexing" がまだ一度も end していない）と、再取り込みの途中の隙間（直近の end が "Compiling" や "Importing build"）の両方で `ready` を名乗らない。"Loading presentation compiler" は readiness に写さない（1 ms で閉じるリクエスト処理相当）
- **先読み**（ADR 0014 追補 決定 D と同じ）: クライアントの `workspace/didChangeWatchedFiles` で `.scala` / `.sc` / `.sbt` / `project.scala` / `build.sbt` / `build.mill` の Created / Changed / Deleted を見たら `indexing` にし、次の "Indexing" の end で戻す。通知から最初の begin までの 0.15〜0.33 秒の隙間を埋める
- **health**: `metals/status`（module）の `level` から。`error` → `error`（message は `tooltip` か `text`）、`warn` → `warning`、`info` → `ok`。最初の module status までは `unknown`
- **時間は使わない**。Serena の静穏期間は要らない
- **coverage / freshness**: 7.2 / 7.3 の実サーバーテストで決める。`didChange`（保存なし）を織り込むかは Metals の presentation compiler の挙動次第で、織り込まなければ `freshness` は宣言できない

## コーパスへの反映

コーパスの Metals の行は「idle の信号がないので `unknown` に倒すべき」としていたが、実測では "Indexing" の end が取り込みの終端を毎回示し、隙間は規則で越えられる。「時間でしか終わりを言えない」は誤りで、正しくは「未完了トークン 0 を ready と読むと隙間で嘘になる」。`metals/status`（metals）の " Indexing complete!" は再取り込みの前にも出るので、文書どおり actionable でない。
