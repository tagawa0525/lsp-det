# 言語サーバーの readiness 表出方法の調査

lsp-det の将来アダプタ設計のため、pyright / typescript-language-server / eclipse.jdt.ls が
「準備完了(ready)」をどのように外部へ表出するかを一次ソース(reference/ 配下の clone)で調査した。

調査対象のコミット:

| サーバー | コミット | 日付 |
| --- | --- | --- |
| pyright | 6a7d491 | 2026-08-26 |
| typescript-language-server | 19fce01 | 2026-08-23 |
| eclipse.jdt.ls | f118000 | 2026-08-27 |

## 要約

- **pyright**: 解析の進行を LSP 標準の work done progress(クライアントが
  `window.workDoneProgress` を宣言した場合)または独自通知
  `pyright/beginProgress` / `pyright/reportProgress` / `pyright/endProgress` で表出する。
  progress の end は「残り解析ファイル数が 0 になった」ことと厳密に一致する。
  `Found N source files` は `window/logMessage`(info)として出る。
  `experimental/serverStatus` を送るコードは**存在しない**(rust-analyzer 固有拡張)。
- **typescript-language-server**: tsserver の `projectLoadingStart` イベントで
  `$/progress` begin(タイトル `Initializing JS/TS language features…`)、
  `projectLoadingFinish` ほか複数の契機で end する。ただし **tsserver クラッシュ時にも
  progress は end される**ため、`$/progress` 単独では正常完了とクラッシュを区別できない。
  クラッシュは `window/logMessage` のエラーログと接続断で判別する。
- **eclipse.jdt.ls**: 独自通知 `language/status`(`{type, message}`)で状態遷移を明示的に送る。
  `type: "ServiceReady"` が「全機能利用可能」の確定シグナルで、3 サーバー中もっとも
  機械判定に適する。プロジェクトの健全性は `type: "ProjectStatus"` + `"OK"` / `"WARNING"` で届く。

---

## 1. pyright

### 1.1 progress の仕組み

進行報告の実装は `createProgressReporter()` にある
(`reference/pyright/packages/pyright-internal/src/server.ts:277-323`)。

- クライアントが `window.workDoneProgress` capability を宣言している場合
  (判定: `packages/pyright-internal/src/languageServerBase.ts:622`)、
  `connection.window.createWorkDoneProgress()` を使う。すなわち
  `window/workDoneProgress/create` リクエスト + `$/progress` (begin / report / end)。
- 宣言していない場合は後方互換の独自通知にフォールバックする
  (`server.ts:295`, `306`, `319`):

```text
pyright/beginProgress   (params なし)
pyright/reportProgress  (params: string 例 "3 files to analyze")
pyright/endProgress     (params なし)
```

progress のライフサイクルは解析完了コールバックが駆動する。

- `service.setCompletionCallback(...)` の登録:
  `languageServerBase.ts:333`
- `onAnalysisCompletedHandler` (`languageServerBase.ts:1352`) が診断送出後に
  `sendProgressMessage(results.requiringAnalysisCount.files, ...)` を呼ぶ (`:1373`)
- `sendProgressMessage` (`languageServerBase.ts:1396-1414`) は
  **`fileCount <= 0` なら `reporter.end()`、そうでなければ begin(未表示時)+ report** する

`requiringAnalysisCount` は `program.getFilesToAnalyzeCount()` に由来し
(`packages/pyright-internal/src/analyzer/analysis.ts:60`)、`AnalysisResults` の定義は
`analysis.ts:23-33`。つまり **progress の end = 解析待ちファイル数が 0 になった瞬間**であり、
readiness 判定の意味論として信頼できる。

注意点:

- 小規模ワークスペースで解析が一瞬で終わると、最初の結果が `fileCount == 0` となり
  begin なしで end(no-op)だけが起きる。**progress が一度も観測されないケースがある**。
- `ProgressReportTracker` (`packages/pyright-internal/src/common/progressReporter.ts:18-61`)
  が二重 begin / 迷子 report を抑止しているため、begin→end の対応は保証される。

### 1.2 "Found N source files" ログの出所

- 出力箇所: `SourceEnumerator._finish()`
  (`packages/pyright-internal/src/analyzer/sourceEnumerator.ts:305-308`)。
  `Found ${fileCount} source file(s)` を `console.info`、0 件なら `No source files found.`。
  列挙開始時には `Searching for source files` を `console.log` (`sourceEnumerator.ts:83`)。
- `SourceEnumerator` は `AnalyzerService._updateTrackedFileList()` で生成される
  (`packages/pyright-internal/src/analyzer/service.ts:1377-1392`)。起動時だけでなく、
  設定変更やファイル作成/削除で追跡ファイルリストを再構築するたびに再列挙・再ログされる。
- console は `new ConsoleWithLogLevel(connection.console)` (`server.ts:57`) であり、
  クライアントへは **`window/logMessage`** として届く(レベルは info。
  `logLevel` 設定で抑制されうる: `languageServerBase.ts:393`)。

つまりこのログは「列挙完了」のシグナルであって「解析完了」ではない。解析完了は 1.1 の
progress end を待つ必要がある。

### 1.3 experimental/serverStatus

`packages/pyright-internal/src` と `packages/pyright/src` を `serverStatus` で grep したが、
**該当コードは存在しない**(ヒットは無関係な `typeServerMultiConnection` のコメント等のみ)。
`experimental/serverStatus` は rust-analyzer の拡張であり、pyright には実装されていない。

---

## 2. typescript-language-server

### 2.1 $/progress の begin / end タイミング

progress は `ServerInitializingIndicator` が一元管理する
(`reference/typescript-language-server/src/ts-client.ts:107-138`)。

**begin**: tsserver の `projectLoadingStart` イベント受信時
(`src/ts-client.ts:450-451`)に `startedLoadingProject()` が呼ばれ、

- `lspClient.createProgressReporter()`
  (`src/lsp-client.ts:44-52`)→ `connection.window.createWorkDoneProgress()`。
  すなわち `window/workDoneProgress/create` + `$/progress` begin。
- タイトルは固定文字列 `'Initializing JS/TS language features…'` (`src/ts-client.ts:129`)。
- クライアントが `window.workDoneProgress` を宣言していない場合、vscode-languageserver は
  no-op reporter を返すため **何も観測できない**。
- TS のプロジェクトは逐次ロードされるため、新しい `projectLoadingStart` は前の progress を
  `reset()`(= end)してから開始する(`src/ts-client.ts:122-125` のコメント)。

**end**(`reset()` → `reporter.done()`)の契機は複数ある:

| 契機 | 根拠 |
| --- | --- |
| `projectLoadingFinish`(プロジェクト名一致時) | `src/ts-client.ts:453-454`, `133-137` |
| `syntaxDiag` / `semanticDiag` / `suggestionDiag` / `configFileDiag` イベント | `src/ts-client.ts:407-414`(「これらのイベントもロード完了をおおむね意味する」とのコメント) |
| `projectsUpdatedInBackground` イベント | `src/ts-client.ts:433-434` |
| `UpdateOpen` コマンドの応答完了 | `src/ts-client.ts:516-522` |
| tsserver プロセス終了 (`serviceExited`) | `src/ts-client.ts:400-405` |
| サーバー shutdown | `src/ts-client.ts:459-462` |

### 2.2 tsserver クラッシュ時と正常完了時の外部から見た違い

**`$/progress` の end はどちらでも発生する**(`serviceExited()` が indicator を reset するため)。
progress だけを見ていると、クラッシュを「ready になった」と誤判定する。

外部から観測できる違い:

- クラッシュ時は `window/logMessage`(error)に
  `[tsserver] Exited. Code: N. Signal: S` が出る(`src/ts-client.ts:378-382`。
  logger は `LspClientLogger` → `window/logMessage`:
  `src/utils/logger.ts:68`, `src/lsp-connection.ts:20`)。
- exit code 非 0 の場合、`onExit` コールバックが `shutdown()` 後に throw する
  (`src/lsp-server.ts:217-221`:
  `tsserver process has exited (exit code: N, signal: S). Stopping the server.`)。
  未捕捉例外として language server プロセス自体が落ち、**LSP 接続(stdio)が閉じる**。
- 非回復エラー(`fatalError`, `src/ts-client.ts:608-623`)では tsserver を kill して
  `ServerState.Errored` に遷移し、以降のリクエストは `ServerResponse.NoServer`
  として即座に失敗する(`src/ts-client.ts:598-605`)。

まとめると: 正常完了 = progress end のみ。クラッシュ = progress end + error ログ +
(多くの場合)接続断。判定には logMessage とプロセス/接続の生存監視の併用が必須。

### 2.3 projectLoadingStart/Finish の扱い

`src/ts-protocol.ts:293-294` で tsserver イベント名として定義され、
`src/tsServer/server.ts:508-512` で優先ディスパッチ、`src/ts-client.ts:450-454` で
progress に変換される。Finish はプロジェクト名が Start と一致した場合のみ end する。
再インデックス(tsconfig 変更等)では `projectLoadingStart` が再度発火し、progress が再 begin
される。バックグラウンド更新の完了は `projectsUpdatedInBackground` で通知され、このとき
progress の end と開いているファイルの診断再取得(`getErr`)が走る(`src/ts-client.ts:433-439`)。

---

## 3. eclipse.jdt.ls

### 3.1 language/status 通知の形式

独自通知として lsp4j で宣言されている
(`reference/eclipse.jdt.ls/org.eclipse.jdt.ls.core/src/org/eclipse/jdt/ls/core/internal/JavaClientConnection.java:50-51`):

```java
@JsonNotification("language/status")
void sendStatusReport(StatusReport report);
```

ペイロードは `StatusReport { type: string, message: string }`
(`.../internal/StatusReport.java:25-35`)。`type` は `ServiceStatus` enum の名前:

```java
// .../internal/ServiceStatus.java:15-16
public enum ServiceStatus {
    Starting, Started, Message, Error, ServiceReady, ProjectStatus
}
```

送出は `JavaClientConnection.sendStatus()`
(`JavaClientConnection.java:142-145`)で
`new StatusReport().withMessage(status).withType(serverStatus.name())` を送る。
クライアント capability による送出ゲートはなく、常に送られる
(`BaseJDTLanguageServer.java:52-56` は client 接続済みかのみ確認)。

なお同ファイルには `language/actionableNotification`(:58)、
`language/eventNotification`(:66、`ProjectsImported` 等)、
`language/progressReport`(:73)も宣言されている。

### 3.2 送出箇所とタイミング(標準モード)

時系列で:

1. `initialize` リクエスト受理時、内部 status を `Starting` にセット
   (`.../handlers/JDTLanguageServer.java:292`)。
2. ワークスペースインポートジョブ開始時:
   `sendStatus(ServiceStatus.Starting, "Init...")`
   (`.../handlers/InitHandler.java:258`)。
   インポート中はジョブ進捗が `language/status` の `type: "Starting"`、
   `message: "NN% Starting Java Language Server - ..."` として繰り返し届く
   (`.../handlers/ProgressReporterManager.java:320-327`)。
3. プロジェクトインポート完了:
   `sendStatus(ServiceStatus.Started, "Ready")` (`InitHandler.java:274`)。
   キャンセル時は `Error` + `"Initialization has been cancelled."`(`:277`)、
   例外時は `Error` + メッセージ(`:281`)。
4. `initialized` 通知の処理で初期化ジョブ完了を待った後、
   「Initialize workspace」ジョブ内で capability 登録・バンドル同期を済ませてから
   **`client.sendStatus(ServiceStatus.ServiceReady, "ServiceReady")`**
   (`JDTLanguageServer.java:326-328`)。この後にビルドジョブ待機
   (`waitForBuildJobs`, `:331` 付近)、`projectsBuildFinished`(`:340`)、
   ワークスペース診断の発行と続く。
5. プロジェクト健全性は `reportProjectsStatus()`
   (`.../managers/ProjectsManager.java:723-730`)が
   `sendStatus(ServiceStatus.ProjectStatus, "WARNING")`(プロジェクトに
   SEVERITY_ERROR のマーカーがある場合)または `"OK"` を送る。呼び出し箇所は
   インポート完了後(`ProjectsManager.java:131`)、`ProjectsImported`
   イベント送出後(`:246`)、プロジェクト設定更新後(`:520`)、
   設定変更反映後(`:817`)。

補足:

- `Started` の `"Ready"` は「インポート完了」であり、capability 登録前。完全な ready は
  `ServiceReady`(コメント too: `JDTLanguageServer.java:586-589` は ServiceReady 前に
  capability を有効化するとリクエストを処理できないと明記)。
- 軽量(syntax-only)モードでは `Started` + `"LightWeightServiceReady"` が送られる
  (`.../syntaxserver/SyntaxLanguageServer.java:258`)。
- `ServiceReady` はセッション中一度だけ。ビルド完了そのもの(`projectsBuildFinished`)は
  `language/status` では直接通知されず、`ProjectStatus` や診断で間接的に観測する。

---

## 4. まとめ: 機械的な ready / ready 解除の判定

### 4.1 ready 判定の最善手

| サーバー | ready 判定 | ready 解除(再インデックス)の検知 | 信頼度 | 注意点 |
| --- | --- | --- | --- | --- |
| pyright | `$/progress` end(または `pyright/endProgress`)。begin→end の 1 サイクル完了で「解析済み」 | 新たな `$/progress` begin / `pyright/beginProgress`。`window/logMessage` の `Searching for source files` / `Found N source files` 再出現は再列挙の傍証 | 高(end は残り解析数 0 と厳密一致) | 小規模 WS では progress が一度も出ないことがある。initialize 応答後に diagnostics/progress のどちらも来ない場合のタイムアウトフォールバックが必要 |
| typescript-language-server | `$/progress`(タイトル `Initializing JS/TS language features…`)の end。`UpdateOpen` 応答完了も同義 | `projectLoadingStart` 由来の progress 再 begin。`projectsUpdatedInBackground` 由来の end は背景更新完了 | 中 | **クラッシュでも end が来る**。`window/logMessage`(error)の `Exited. Code:` と接続断の監視を必須で併用。クライアントが workDoneProgress 非対応だと何も観測できない |
| eclipse.jdt.ls | `language/status` の `type: "ServiceReady"`(syntax モードは `Started` + `"LightWeightServiceReady"`) | `language/status` `type: "Starting"` の再出現(ジョブ進捗)、`ProjectStatus` の更新、`language/eventNotification` | 高(専用の明示的シグナル) | `Started`/`"Ready"` を ready と誤認しない。`ServiceReady` は一度きりなので再接続時以外は再送されない |

### 4.2 アダプタ設計への示唆

- 3 サーバーとも「ready」の一次シグナルは通知(progress または独自通知)であり、
  リクエスト/レスポンスでポーリングする標準手段はない。プロキシは通知ストリームの
  ステートマシンとして readiness を追跡するのが正しい。
- `$/progress` ベースの判定(pyright / tsls)は、プロキシ自身がクライアントとして
  `window.workDoneProgress` capability を initialize で宣言しないと機能しない
  (pyright は独自通知にフォールバックするが、tsls は無音になる)。
- 「end = ready」と単純化すると tsls のクラッシュを ready と誤判定する。
  progress と並行してプロセス生存・`window/logMessage`(error)を監視し、
  end の直後に一定時間エラーログ/接続断がないことを確認してから ready に遷移させるのが安全。
- jdt.ls のように ready 専用通知を持つサーバーはそれを最優先し、progress は補助に回す。
