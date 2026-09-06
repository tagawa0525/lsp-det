# jdtls（Eclipse JDT Language Server）の readiness の実測（M23）

ADR 0020 決定 C の M23。仕様 10 章の jdtls の行（`language/status` の `ServiceReady` と `ProjectStatus`）は v0.1 から「見込み」だった。201 クラスの被験体で測り、`ServiceReady` の後の `references` が完全で、変更（`didChange`、Created / Changed / Deleted の通知）が次の問い合わせに反映されると確かめた。jdtls も要求を索引の完了まで待たせる（JDT の検索は `WAIT_UNTIL_READY_TO_SEARCH`）ので、起動直後の要求も完全な結果で答える。写像は `ServiceReady` で `ready`、health は `ProjectStatus` と、プロジェクト自身に付く診断（"missing required library"）から。`serverInfo` に版があるので、通した版に保証を宣言する。

## 方法

- nixpkgs の jdt-language-server 1.60.0（`serverInfo.version` は "1.60.0-SNAPSHOT"）と jdk21（`flake.nix` の `servers`）。`jdtls -data <空のディレクトリ>`。2026-09-06
- 被験体: Eclipse のプロジェクト記述（`.project` に javanature、`.classpath` に `src` と JRE コンテナ。Maven や Gradle は使わない。ネットワークが要らない）、`src/app/Lib.java`（`public static int target()`）、`src/app/F0.java`〜`F199.java`（各 30 メソッドと `Lib.target()` を 1 回呼ぶメソッド）。ビルドファイルのないフォルダ（jdtls の "invisible project"）でも動くが、ソースルートの推定が `src/app` になり "declared package does not match" の診断が全ファイルに付いたので、記述を置いた
- 道具: scratchpad の `lsp_probe.py`。クライアントは `window.workDoneProgress` と `workspace.didChangeWatchedFiles.dynamicRegistration` を宣言する
- 走行: (1) 起動して `Lib.java` を開き `target` の `references` を送り続ける、(2) `ready` 後に開いている `F0.java` に `didChange`（全文）で呼び出しを足す、(3) `G.java` を作って `didChangeWatchedFiles` Created、(4) 開いていない `F1.java` をディスクで書き換えて Changed、(5) `F2.java` を消して Deleted、(6) (3) を通知なしで、(7) `.classpath` に存在しない jar を書いて起動、(8) `ready` 後に `.classpath` をそう書き換えて Changed
- 裏付けに `reference/eclipse.jdt.ls`（`JDTLanguageServer.java`、`InitHandler.java`、`ProjectsManager.java`、`ReferencesHandler.java`）と JDT core の `BasicSearchEngine.java` を読んだ

## 結果

### 語彙

| 信号                                       | 内容                                                                                                                                                                                                                                                                                                                                                                                              |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `InitializeResult.serverInfo`              | `{"name": "JDT Language Server (Standard)", "version": "1.60.0-SNAPSHOT"}`。版が語彙に現れる                                                                                                                                                                                                                                                                                                      |
| `language/status`                          | `{type, message}`。`type` は `ServiceStatus` enum（`Starting`、`Started`、`Message`、`Error`、`ServiceReady`、`ProjectStatus`）。起動では `Starting` "Init..."、"N% Starting Java Language Server"（複数回）、`ProjectStatus` "OK"、`Started` "Ready"、`ServiceReady` "ServiceReady" の順。`ServiceReady` は `JDTLanguageServer.initialized` がプロジェクトの取り込みとバンドルの同期を終えて送る |
| `ProjectStatus`                            | `ProjectsManager.reportProjectsStatus`: プロジェクトの問題マーカーの最大重大度が error なら "WARNING"、それ以外は "OK"。呼ばれるのはプロジェクトの取り込み（起動）、`ProjectsImported` の後、ビルドファイルの更新の後、設定の更新の後                                                                                                                                                             |
| `$/progress`（token は UUID、都度 create） | Eclipse のジョブの名前が title（"Building"、"Initialize Workspace"、"Refreshing workspace"、"Synchronizing projects"、"Validate documents"、"Publish Diagnostics"、"Searching..." 等）。`references` の処理も "Searching..." として出る                                                                                                                                                           |
| `client/registerCapability`                | `workspace/didChangeWatchedFiles` を `**/*.java` 等で登録する                                                                                                                                                                                                                                                                                                                                     |
| `textDocument/publishDiagnostics`          | ファイルのほか、**プロジェクト自身の URI**（ワークスペースのフォルダ）に付く。存在しない jar を `.classpath` に書くと severity 1 "Project 'x' is missing required library: 'missing.jar'"                                                                                                                                                                                                         |

### 起動（走行 1）

| 時刻（秒） | 出来事                                                                                           |
| ---------- | ------------------------------------------------------------------------------------------------ |
| 1.04       | `initialize` 応答。`didOpen Lib.java`。`references` を送り始める                                 |
| 1.10       | `ProjectStatus` "OK"、`Started` "Ready"                                                          |
| 1.12       | `ServiceReady`                                                                                   |
| 1.13〜1.56 | "Building"、"Refreshing workspace" の `$/progress`                                               |
| 1.56〜2.91 | "Searching..."（`references` の処理）                                                            |
| 3.12       | 1.04 秒から送っていた `references` の応答がまとめて来る。**すべて 200 件**（`F0`〜`F199`。完全） |

`ServiceReady` の前に答えられる要求はなく、答えは索引の完了を待ってから出る（`ReferencesHandler` → JDT の `SearchEngine.search` → `BasicSearchEngine.findMatches` が `IJavaSearchConstants.WAIT_UNTIL_READY_TO_SEARCH` で `performConcurrentJob` する）。空応答も部分応答もない。7.1 の前提「`ready` の前の結果は不完全」は jdtls では起きない。

### 変更の取り込み（走行 2〜6）

| 引き金（`ready` 後）                                            | 結果                                                                                                                                        |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| 開いている `F0.java` に `didChange` で呼び出しを足す（走行 2）  | 直後の `references` が **201 件**（0.14 秒後。"Validate documents" の後）                                                                   |
| `G.java` を作って Created（走行 3）                             | 次の `references` が **201 件**（0.24 秒後）                                                                                                |
| 開いていない `F1.java` をディスクで書き換えて Changed（走行 4） | 次の `references` が **201 件**（0.24 秒後）。"Building" の `$/progress` はその後に出る（索引の更新はビルドではなく通知の処理で同期に済む） |
| `F2.java` を消して Deleted（走行 5）                            | 次の `references` が **199 件**（0.24 秒後）                                                                                                |
| 同じ作成を通知しない（走行 6）                                  | 変わらない（200 件のまま）。ディスクは自分では監視しない（登録した watcher に頼る）                                                         |

`didChangeWatchedFiles` の後に発行した要求はどれも変更を反映した。反映の前に古い答えが返る窓は観測できなかった（変更の時点で処理中だった要求は古い件数で答える。LSP の順序どおり）。

### health（走行 7、8）

| 条件                                                         | 結果                                                                                                                                                                                                                                               |
| ------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `.classpath` に存在しない jar を書いて起動（走行 7）         | `ProjectStatus` は **"OK"**（1.16 秒。ビルドがマーカーを付ける前に `reportProjectsStatus` が呼ばれる）。1.35 秒にプロジェクトの URI へ severity 1 "Project 'j9' is missing required library: 'missing.jar'" の診断。`references` は 200 件で答える |
| `ready` 後に `.classpath` をそう書き換えて Changed（走行 8） | 0.3 秒後にプロジェクトの URI へ同じ診断（空の診断で消してから）。`ProjectStatus` は 8 秒待っても来ない                                                                                                                                             |

`ProjectStatus` "WARNING" はソースにある語彙だが、この被験体では出せなかった。壊れた classpath は、プロジェクト自身の URI に付く診断として現れる。仕様 6 章 5 項の「読み込めないワークスペースは `health` で」に当たるのはこちらである。

## 写像（設計）

- **識別**: `serverInfo.name` "JDT Language Server (Standard)"。版は `serverInfo.version`（"1.60.0-SNAPSHOT"）
- **readiness**: `initializing` から、`language/status` の `type: "ServiceReady"` で `ready`。`$/progress` は readiness に写さない（"Building" は診断のためのコンパイルで索引ではなく、索引の更新は通知の処理で同期に済み、検索は索引の完了を待つ。写すと完全な結果を遅らせるだけになる）
- **先読み**: しない。サーバーが要求を待たせる
- **health**: `language/status` の `ProjectStatus` "OK" で `ok`、"WARNING" で `warning`、`type: "Error"` で `error`。加えて、`.java` でない URI（プロジェクト自身、ビルドファイル）への `publishDiagnostics` に severity 1 があれば `warning`、その URI の診断が空になれば戻す（仕様の `warning` は "partly functional (missing dependencies and the like)" で、"missing required library" がそのもの）
- **coverage / freshness**: 7.1〜7.3 を通した版（"1.60.0-SNAPSHOT"）に `coverage: {scope: "workspace", incomplete: {}}` と `freshness: {fileChanges: ["Created", "Changed", "Deleted"]}` を宣言する
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: `ProjectStatus` "WARNING" がビルドの後に送られない（`reportProjectsStatus` がマーカーの前に呼ばれる）。classpath の壊れを `language/status` でも伝えるよう、ビルドの完了後に `reportProjectsStatus` を呼ぶ提案

## コーパスへの反映

「`ServiceReady` 受信 → `ready`」はそのとおり。`ProjectStatus` "WARNING" は実際にはほぼ出ず、壊れた classpath はプロジェクトの URI の診断に出る。加えて Dart、Sorbet と同じく「要求をサーバー自身が待たせるので、観測者の保留がなくても嘘は出ない」。コーパスの未確認事項「再インポート時に `ServiceReady` / `ProjectStatus` が再送されるか」は、Eclipse の記述の被験体では `.classpath` の変更で `ProjectStatus` は再送されなかった（Maven / Gradle は測っていない）。
