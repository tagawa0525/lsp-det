# haxe-language-server の readiness の実測（M20）

ADR 0019 決定 F の M20。コーパス（[readiness-vocabulary-corpus.md](readiness-vocabulary-corpus.md)）は Haxe を「進捗の機構をリクエスト処理にも使い回す」型に置き、「起動系のタイトルだけで `ready` を言えるか」を疑問にしていた。実測とソースで、**言える**。`$/progress` の title は `"Haxe: " + 名前 + "..."` の 1 つの機構で、起動系は "Building Cache"、"Parsing Classpaths"、"Building Refactoring Cache…" の 3 つ（並行する）、リクエスト系は "Collecting Diagnostics"、"Performing Refactor Operation…"、"Performing Rename Operation…" の 3 つで、名前が固定なので区別できる。ただし起動系のトークンの間、要求はコンパイラ（`haxe --wait`）の待ち行列で止まり、完了してから完全な答えが返るので、空応答も部分応答もない。サーバーはクライアントが `workspace/didChangeConfiguration` を送るまでコンパイラを起動せず、それまでは `references` に `-32601`（Unhandled method）を返す（正直）。health は `window/showMessage`（Error）と `window/logMessage` "Haxe connected!" で分かる。開いている文書の `didChange` は `references` に織り込まれず、ディスクに書いて `didSave` を送ったときだけ反映される。

## 方法

- vshaxe 2.34.2 の VS Code 拡張（Open VSX の `nadako.vshaxe-2.34.2.vsix`）に同梱の `server/bin/server.js`（haxe-language-server。単体の release も npm もない）。Node.js 24、Haxe 4.3.7（nixpkgs）。`node server.js --stdio`。2026-09-06
- 被験体: `build.hxml`（`-cp src`、`-main Main`、`--interp`）、`src/A.hx`（`A.target()`）、`src/B.hx`（`A.target()` を呼ぶ）、`src/Main.hx`。大きい被験体は `A.target()` を 30 回呼ぶ `C001` … `C300` を足した 303 ファイル（`references` 9001 件）
- `initializationOptions` は `{"displayArguments": ["build.hxml"]}`（Serena と同じ）。`displayServerConfig` は既定（`path: "haxe"`）
- 走行: (1) 設定なし、(2) `didChangeConfiguration {"haxe": {}}` あり、(3) 303 ファイルで 0.05 秒間隔、(4) 開いていない `B.hx` をディスクで書き換え、通知なし、(5) 開いている `B.hx` を `didChange`、(6) ディスクに書いて `didChange` と `didSave`、(7) ディスクだけ 25 秒、(8) ディスクを変えずに `didChange` と `didSave`、(9) `displayServerConfig.path` を存在しない実行ファイルにする
- 裏付けにソース（`src/haxeLanguageServer/Context.hx`、`Configuration.hx`、`server/HaxeServer.hx`、各 feature の `startProgress` の呼び出し）を読んだ

## 結果

### 語彙

| 信号                                      | 内容                                                                                                                                                                                                                                                                                                                                                           |
| ----------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `$/progress`（token は整数、create の後） | `Context.startProgress(name)` が `"Haxe: " + name + "..."` を title にする 1 つの機構。起動系: "Building Cache"（`HaxeServer`）、"Parsing Classpaths"（同）、"Building Refactoring Cache…"（`RefactorCache`。前 2 つと並行）。リクエスト系: "Collecting Diagnostics"（`DiagnosticsFeature`）、"Performing Refactor Operation…"、"Performing Rename Operation…" |
| `window/logMessage`（type 4）             | コンパイラの起動で "Haxe Path: haxe"、"Using --server-connect"、**"Haxe connected!"**、起動系の終わりに "Done."                                                                                                                                                                                                                                                |
| `window/showMessage`（type 1）            | "Haxe version check failed: …"（実行ファイルがない・古い）、"Invalid compiler argument '…' detected. …"（3 回クラッシュしたとき）                                                                                                                                                                                                                              |
| `haxe/haxeKeepsCrashing`（通知）          | 3 回クラッシュし、引数の誤りでもないとき（未実測。ソース `HaxeServer.onExit`）                                                                                                                                                                                                                                                                                 |
| `client/registerCapability`               | `initialize` の直後は **空の登録**（`registrations: []`）。コンパイラが起動してから typeDefinition / implementation / codeAction / codeLens を登録する。`references` 等は `initialize` の capability にはなく、`didChangeConfiguration` の前は `-32601` "Unhandled method textDocument/references"                                                             |

`serverInfo` は **null**。`InitializeResult.capabilities` は `textDocumentSync` と、動的登録を宣言しなかった機能の provider だけ。名乗りに当たるのは "Haxe Path: " のログ（設定の後）と "Haxe: …" の title。版はどこにも現れない。

### 設定がなければコンパイラを起動しない（走行 1）

`didChangeConfiguration` を送らないと、`references` も `workspace/symbol` も 20 秒間ずっと `-32601`。ソース: `Configuration.onDidChangeConfiguration` → `onDidChange(User)` → `Context.restartServer` → `haxeServer.start` の中で `FindReferencesFeature` 等が作られる。`settings.haxe` は `{}` でよい。空応答ではなくエラーなので嘘ではないが、Nextflow（M12）と同じく設定を送らないクライアントは永久に使えない。

### 起動（走行 2、3）

303 ファイル、`didChangeConfiguration {"haxe": {}}`、`didOpen src/A.hx`、0.05 秒間隔。

| 時刻（秒）   | 出来事                                                                                                                                                                          |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0.048        | `initialize` 応答（`serverInfo` なし）、`initialized`、`didChangeConfiguration`、`didOpen`                                                                                      |
| 0.057〜0.067 | "Haxe Path: haxe"、"Using --server-connect"、"Haxe connected!"                                                                                                                  |
| 0.068        | "Haxe: Building Cache..." begin。typeDefinition 等の動的登録                                                                                                                    |
| 0.267        | 同 end。"Haxe: Parsing Classpaths..." begin、"Haxe: Building Refactoring Cache…..." begin（並行）                                                                               |
| 2.102        | "Building Refactoring Cache…" end                                                                                                                                               |
| 2.922        | "Parsing Classpaths" end、"Done."。**同じ瞬間に最初の `references` が 9001 件**（0.05 秒間隔で送った 28 件は登録前の `-32601`、以後の要求はコンパイラの待ち行列で止まっていた） |

起動系のトークンの間に答えられた要求はなく、end の後の答えは完全。3 ファイルの被験体（走行 2）では 0.132 秒で "Done."、0.654 秒の最初の `references` が 1 件（完全）。

### 変更の取り込み（走行 4〜8）

| 引き金（`ready` 後）                                            | 結果                                                                           |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| 開いていない `B.hx` をディスクで書き換え、通知なし（走行 4、7） | 25 秒たっても 1 件のまま。監視の登録もない                                     |
| 開いている `B.hx` を `didChange`（走行 5）                      | 1 件のまま（8 秒）。他のファイルの `references` は開いている文書の中身を見ない |
| ディスクを変えずに `didChange` と `didSave`（走行 8）           | 1 件のまま                                                                     |
| ディスクに書いて `didChange` と `didSave`（走行 6）             | 0.5 秒後の `references` が 2 件。`didSave` でコンパイラがディスクから読み直す  |

7.3 の 1（`didChange` の鮮度）は通らない。`freshness.fileChanges` も空。反映されるのは「ディスクに書いて保存の通知」の組だけで、これは LSP の語彙では `didSave`。

### health（走行 9）

`displayServerConfig.path` を存在しない実行ファイルにすると、設定の直後に `window/showMessage` type 1 "Haxe version check failed: \"/bin/sh: … haxe-missing: command not found\""。以後 `references` は `-32601` のまま（起動しないので登録もされない）。コンパイラの起動は "Haxe connected!" のログで分かる。クラッシュは `HaxeServer.onExit` が 3 回まで再起動し、それ以上は引数の誤りなら `showMessage` Error、そうでなければ `haxe/haxeKeepsCrashing`（未実測）。

## 写像（設計）

- **識別**: `serverInfo` がなく、`window/logMessage` が "Haxe Path: " で始まれば haxe-language-server（設定の後に出る。`identity_from_notification` の経路）。設定を送らないクライアントには出ず、両軸 `unknown` のまま（要求は `-32601` の正直なエラー）。版は取れない
- **readiness**: 識別の時点で `initializing`。起動系の title（"Haxe: Building Cache..."、"Haxe: Parsing Classpaths..."、"Haxe: Building Refactoring Cache…..."）の begin で（`ready` だったなら）`indexing`、起動系のトークンが 1 つも開いていない end で `ready`。リクエスト系の title（"Haxe: Collecting Diagnostics..."、"Haxe: Performing Refactor Operation…..."、"Haxe: Performing Rename Operation…..."）は写さない。要求はコンパイラの待ち行列で止まるので保留は嘘を消すというより、待ち行列の見える化になる
- **health**: `window/showMessage` type 1 で "Haxe version check failed" または "Invalid compiler argument" で始まれば `error`（message は本文）。`window/logMessage` "Haxe connected!" で `ok`（コンパイラが起動した観測）。`haxe/haxeKeepsCrashing` で `error`
- **coverage / freshness**: 宣言しない（`{}`）。版が語彙に現れず、`didChange` の鮮度（7.3 の 1）も通らない
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: `serverInfo`。`didChangeConfiguration` なしでもコンパイラを起動する（または `initialize` で capability を宣言する）。開いている文書の `didChange` を他ファイルの `references` に織り込む

## コーパスへの反映

「進捗の機構をリクエスト処理にも使い回す」型のとおりで、疑問「起動系のタイトルだけで `ready` を言えるか」の答えは「言える。名前が固定で 3 対 3 に分かれる」。Serena の「診断が届いたらトークンが空なら ready」は要らない。新しく分かったのは、設定を送るまで起動しないことと、`didChange` が他ファイルの `references` に反映されないこと。
