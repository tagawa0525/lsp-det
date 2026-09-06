# Nextflow 言語サーバーの readiness の実測（M12）

ADR 0019 決定 F の M12。コーパス（[readiness-vocabulary-corpus.md](readiness-vocabulary-corpus.md)）は Nextflow を「`references` は都度同期する（per-request の同期）」型に置き、グローバルな readiness なしで済むかを疑問にしていた。実測とソースで、**`references` は同期しない**（`updateNow()` も `awaitUpdate()` も呼ばない。同期するのは completion と formatting だけ）。走査は `$/progress` "Initializing" の中では走らず、その後の最初の更新で走り、完了の信号は「ワークスペースの全ファイルに `publishDiagnostics` が出る」ことだけ。観測者はワークスペースのファイル集合を自分で再現すれば時間なしで写像できる。版は語彙に現れないので保証は宣言しない。

## 方法

- nextflow-io/language-server の release v26.04.3 の `language-server-all.jar`（sha256 `20cfa34f…`、release の digest と一致）。OpenJDK 21.0.12。`java -jar language-server-all.jar`（`--help` も `--version` も出力なし。`--version` はサーバーとして起動して待つ）。2026-09-06
- 被験体: `main.nf`（`include { GREET } from './modules/greet.nf'` と `workflow { GREET(...) }`）、`modules/greet.nf`（`process GREET`）、`nextflow.config`。大きい被験体は同じ構成に `w_001.nf` … `w_400.nf`（それぞれ GREET を include して 1 回呼ぶ）を足した 403 ファイル
- 道具: scratchpad の `lsp_probe.py`（Metals、Expert と同じ。`didChangeConfiguration`、開いている文書の `didChange`、`ready` 後の設定の再送を足した）。設定は Serena と同じ `{"nextflow": {"errorReportingMode": "errors", "files": {"exclude": ["work", ".nextflow"]}}}`
- 走行: (1) 設定なし、(2) 設定あり `didOpen modules/greet.nf`、(3) 403 ファイル、0.1 秒間隔、(4) 設定あり `didOpen` なし、(5) `ready` 後に開いている `greet.nf` に `didChange` で呼び出しを足す、(6) `ready` 後に `w_new.nf` を作って Created、(7) `ready` 後に開いていない `main.nf` に足して Changed、(8) `didOpen` なしで completion を 1 回送る、(9) 403 ファイルで `main.nf` を開く、(12) `ready` 後に除外の違う設定を再送する
- 裏付けにソース（`NextflowLanguageServer.java`、`services/LanguageService.java`、`services/script/ScriptAstCache.java`、nextflow 本体の `util/PathUtils.java`）を読んだ

## 結果

### 語彙

| 信号                                                       | 内容                                                                                                                                                                                                                                                                                                                                                  |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `$/progress`（token は文字列 `"initialize"`、create の後） | title "Initializing"、message "Initializing workspace..." の begin → report("Initializing workspace: name (i / total)") → end。**中身は設定の差し替えとキャッシュのクリアだけ**（`LanguageService.initialize`。begin から end まで 5 ms）。ワークスペースの走査はここでは走らない。設定差分のある `workspace/didChangeConfiguration` でしか始まらない |
| `textDocument/publishDiagnostics`                          | 更新（`update0`）のたびに、更新したファイル全部に出る（診断が 0 件でも出る）。初回の走査ではワークスペースの全ファイルに一斉に出る（403 ファイルで 7 ms）。設定の再送で "Initializing" の中にも出る（クリア。診断 0 件）                                                                                                                              |
| `window/workDoneProgress/create`                           | token `"initialize"`                                                                                                                                                                                                                                                                                                                                  |

`serverInfo` は **null**。`window/logMessage` も起動時の名乗りもない。語彙の中で名乗りに当たるのは `InitializeResult.capabilities.executeCommandProvider.commands` の `nextflow.server.previewDag` 等（4 つ、すべて `nextflow.server.` で始まる）だけで、**版はどこにも現れない**。health の信号はない。

### 設定がなければ何も始まらない（走行 1、4、8）

| 条件                                                   | 結果                                                                                                                                                                          |
| ------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `didChangeConfiguration` を送らない（走行 1）          | "Initializing" なし。`references` は 60 秒間 30 回とも空配列                                                                                                                  |
| 設定あり、`didOpen` なし（走行 4）                     | "Initializing" は来るが走査は始まらない。`references` は 20 秒間空配列                                                                                                        |
| 設定あり、`didOpen` なし、completion を 1 回（走行 8） | completion は `updateNow()` で同期更新し、そこで走査が走る。要求の 0.15 秒後に `main.nf` と `greet.nf` の診断、その次の `references` が 2 件（`ready` に当たるのは 0.877 秒） |

ソース: `didChangeConfiguration` は `errorReportingMode` / `files.exclude` / `pluginRegistryUrl` のいずれかが**前の値と違う**ときだけ `initializeWorkspaces()` を呼ぶ（`shouldInitialize`）。サーバーの既定値は `errorReportingMode: "warnings"`、`excludePatterns: []`（VS Code 拡張の既定値 `["work", ".nextflow"]` とは違うので、拡張の既定値を送れば差分になる。`pluginRegistryUrl` は文字列を `!=` で比べているので、既定値と同じ文字列を送っても差分になる）。それまでは各サービスが `initialized = false` で、更新は何もせず戻る。Serena は `errorReportingMode: "errors"` を「サーバーの既定値と違う値」としてわざと送っている（`_SCAN_TRIGGERING_ERROR_REPORTING_MODE`）。

走査は `initialize()` の中では走らず、`scanned = false` にするだけ。次の更新（`update0`）で「変更されたファイルがない」ときに `getWorkspaceFiles()` で走る。更新の引き金は `didOpen` / `didChange` / `didClose`（1 秒のデバウンス）と completion / formatting（同期）。`references`、`definition`、`hover` は引き金にならない。

### 時系列（走行 3、403 ファイル）

| 時刻（秒）   | 出来事                                                                                                                                            |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0.372        | `initialize` 応答。`initialized`、`didChangeConfiguration`、`didOpen modules/greet.nf`                                                            |
| 0.376〜0.383 | "Initializing" begin → end（7 ms）                                                                                                                |
| 0.389〜2.542 | `references` は**空配列を 22 回**返す（"Initializing" は終わっていて、未完了トークンはない）                                                      |
| 1.523        | `greet.nf` だけの `publishDiagnostics`（`didOpen` のデバウンス後の更新。開いたファイルだけを解析し、`scanned` が false なので次の更新を予約する） |
| 2.622〜2.629 | **402 ファイルの `publishDiagnostics` が一斉に**（走査）                                                                                          |
| 2.644        | `references` が初めて 802 件（400 ファイル × 2 + `main.nf` の 2）                                                                                 |

小さい被験体（走行 2）でも同じ形で、`didOpen` の 1.1 秒後に開いたファイルの診断、2.0 秒後に走査の診断、その直後に `references` が 2 件。

**「Initializing」の end の後、走査までの 2 秒間、サーバーは空配列の成功応答を返す。** 未完了トークンがないので、トークンだけを見る観測者は `ready` と言ってしまう。

### 走査の完了を示すのは診断の集合だけ（走行 3、9）

走査の前の更新は、開いたファイルと**その include 先**を解析して診断を出す。走行 9（403 ファイルで `main.nf` を開く）では、走査の前に `greet.nf`（開いていない。`main.nf` の include 先）と `main.nf` の診断が 1.431 秒に出て、走査の 402 件は 2.488 秒。つまり「開いていないファイルの診断が出た」は走査の完了ではない。走査の完了と区別できるのは「ワークスペースの `*.nf` 全部に診断が出た」ことだけで、その集合はサーバーが `PathUtils.visitFiles` で root を歩いて作る（除外は `path == pattern || path.endsWith("/" + pattern)` をディレクトリとファイルに当てる。symlink は辿らない。`*.nf` だけ。`workspaceFolders` に入っていないファイルは既定のサービスに落ち、そのサービスは初期化されないので永久に解析されない）。

### 開いている文書の `didChange` は 1 秒古い（走行 5）

| 時刻（秒）   | 出来事                                                                                    |
| ------------ | ----------------------------------------------------------------------------------------- |
| 2.462        | `greet.nf` に GREET の呼び出しを 1 つ足す `didChange`                                     |
| 2.564〜3.375 | `references` は**古い 2 件を 9 回**返す                                                   |
| 3.471        | `greet.nf` と `main.nf` の `publishDiagnostics`（デバウンス後の更新。include 元も再解析） |
| 3.573        | `references` が 3 件                                                                      |

`references` は同期しないので、7.3 の 1 はグローバルな readiness（変更したファイルの診断が出るまで `indexing`）でしか通らない。診断は変更したファイルに必ず出る（`ScriptAstCache.analyze` は変更した URI をそのまま含める）ので、先読みの条件（ADR 0014 追補 決定 D）を満たす。

### 監視対象の変更は取り込まない（走行 6、7）

| 引き金（`ready` 後）                                        | 30 秒以内の信号 | 結果                       |
| ----------------------------------------------------------- | --------------- | -------------------------- |
| `w_new.nf`（GREET を 1 回呼ぶ）を作って Created（走行 6）   | なし            | `references` は 2 件のまま |
| 開いていない `main.nf` に呼び出しを足して Changed（走行 7） | なし            | `references` は 2 件のまま |

`didChangeWatchedFiles` はソースでも debug ログを出すだけ（`didCreateFiles` / `didDeleteFiles` / `didRenameFiles` も同じ）。走査は 1 回きり（`scanned`）で、以後の更新は変更された開いている文書だけを解析する。`freshness.fileChanges` は空になる。

### 設定の再送はキャッシュを捨てる（走行 12）

除外の違う設定を `ready` 後に送ると、"Initializing" が再び begin し、**その中で** `main.nf` と `greet.nf` に診断 0 件の `publishDiagnostics`（クリア）が出て end。以後 `references` は空配列（8 秒間 15 回）。キャッシュを捨てて `scanned = false` に戻すが、走査の引き金は来ないので、次に何かを開くか編集するまで空応答が続く。観測者はトークンの中の診断を走査に数えてはならない。

## 写像（設計）

- **識別**: `serverInfo` がなく、`capabilities.executeCommandProvider.commands` に `nextflow.server.` で始まる命令があれば Nextflow の言語サーバー。版は取れない
- **readiness**: 起動時は `initializing`。"Initializing" の begin で `initializing`（キャッシュを捨てるので）。end の時点で、クライアントの `initialize` の `workspaceFolders` を歩き（サーバーと同じ規則: `*.nf`、除外は `didChangeConfiguration` の `nextflow.files.exclude` を `path == pattern || path.endsWith("/" + pattern)` で、symlink は辿らない）、その集合を「走査で診断が出るべきファイル」とする。トークンの外で来た `publishDiagnostics` で集合から消し、空になったら `ready`。集合が最初から空（`*.nf` がない、または `workspaceFolders` がない）なら end で `ready`（走査するものがない。`workspaceFolders` のないクライアントには既定のサービスが初期化されず何も答えないが、ワークスペースに対しては完全）。トークンの中の診断（クリア）は数えない
- **先読み**: `workspaceFolders` の下の文書の `didOpen` / `didChange` / `didClose` で、その文書の `publishDiagnostics` が出るまで `indexing`（`ready` 後）。走査の前に来た分は走査の集合と同じ扱い（診断が出れば消す）。`didChangeWatchedFiles` の Deleted は走査の集合から外す（歩いた後に消えたファイルは走査で診断が出ない）。Created / Changed は取り込まれないので先読みしない
- **health**: 信号がなく `unknown`
- **coverage / freshness**: 宣言しない（`serverStateProvider: {}`）。写像の規則は 26.04.3 のソースと実測に基づくが、**版が語彙に現れない**ので、準拠テストを通した版だけに宣言する（仕様 8.2 の 5）ことができない。7.2 と 7.3 の 1 は 26.04.3 で通ることを実サーバーテストで測るが、約束はしない
- **設定を送らないクライアント**には `initializing` のまま（保留のログが理由を出す）。観測者が設定を注入することはしない。決定 G の注入は「信号の有効化」であって、サーバーの初期化そのものを肩代わりすることではない。上流に「`initialized` で初期化する」「`serverInfo` を返す」「走査を進捗に出す」を求めるのが根本の解決で、`docs/upstream-submissions.md` に候補として置く

## コーパスへの反映

「`references` は都度同期する」は誤り（Serena の実装が同期していると読んだのは `awaitUpdate()` を呼ぶ codeLens 等の経路で、`references` は呼ばない。コーパスの別の行にはそう書いてあった）。per-request の同期でグローバルな readiness を省ける型の実例は、Nextflow では取れなかった。疑問は「走査の完了を示す信号がなく、観測者がファイル集合を再現するしかない」に置き換わる。
