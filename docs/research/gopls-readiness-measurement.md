# gopls の readiness と health の実測（2026-09-03）

lsp-det 経由で実 gopls に接続し、設計 5.2 の写像（`$/progress` からの合成）が実サーバーで成り立つかを測った。ソースの読み（`server/general.go`・`server/workspace.go`・`server/diagnostics.go`・`progress/progress.go`）を実測で裏付けることが目的。

## 結論

| 項目                    | ソースの読み                                                                                                           | 実測                                                                                                                         |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| 起動時の readiness 信号 | ワークスペースフォルダごとに title "Setting up workspace" の progress が begin し、"Finished loading packages." で end | **一致**。`initializing` → `indexing` → `ready` を観測                                                                       |
| go.mod 変更での再発行   | `addFolders` 以外に発行箇所がなく、再発行されない                                                                      | **一致**。ディスク上で go.mod を変えて `didChangeWatchedFiles` を送っても 8 秒間 `serverStateChanged` は来ず、`ready` のまま |
| 7.2 完全性              | リクエストごとにスナップショットを取る                                                                                 | **通過**。`ready` 後の `references` がクロスファイルの呼び出しを返す                                                         |
| 7.3 クロスファイル鮮度  | `didChange` のオーバーレイをスナップショットに織り込む                                                                 | **通過**。`b.go` の呼び出しを `didChange` で消した後、`a.go` 起点の `references` が反映している                              |
| 安定性                  | —                                                                                                                      | 上記 4 件を 5 回連続で実行して全回通過（1 回あたり約 8 秒）                                                                  |

この結果を根拠に、gopls の写像は v0.23.0 に `{completeness: true, freshness: true}` を宣言する（`src/adapter/gopls.rs` の `TESTED_VERSIONS`）。範囲外の版には宣言しない。

## 測定環境

- gopls: v0.23.0（nixpkgs、`serverInfo.version` はビルド情報の JSON 文字列で `"Version":"v0.23.0"`。`Main.Version` は `(devel)`）
- go: go1.26.7 linux/amd64
- 経路: 偽クライアント → lsp-det（`-- gopls`）→ gopls。写像は `serverInfo.name` で選ばれる
- fixture: 一時的な Go モジュール（`go.mod`、`a.go` の `Target`、`b.go` の `Caller` から呼ぶ）。`initialize` に `rootUri` と `workspaceFolders` を渡す（フォルダなしだと "Setting up workspace" が出ない）
- テスト: `tests/conformance.rs` の `gopls_*`（`#[ignore]`、`cargo test --test conformance -- --ignored gopls_`）

## 測定方法

1. `initialize`（`experimental.serverState: true` を宣言）→ `initialized`。lsp-det は `window.workDoneProgress` を注入し、`window/workDoneProgress/create` に自ら応答する
2. `experimental/serverState` が `ready` でないことを確認してから、`serverStateChanged` で `ready` を待つ
3. 7.2: `a.go`・`b.go` を `didOpen` し、`a.go` の `Target`（line 2, character 5）の `references` に `b.go` の line 3 が含まれることを見る
4. 7.3: 同じ状態から `b.go` を呼び出しなしの内容で `didChange`（version 2）し、`ready` のまま `references` が空になることを見る
5. go.mod: `ready` 後に go.mod へコメントを追記し、`workspace/didChangeWatchedFiles`（type 2 = Changed）を送って 8 秒観測する

## 一般化してはならない点

- 「再発行されない」はソースの構造由来（発行箇所が `addFolders` だけ）で、規模に依らない。一方、初回ロードの所要時間は fixture（2 ファイル）での値であり、大規模モジュールには一般化できない
- go.mod 変更後の再ロード中に返る応答が完全か鮮度を保つかは測っていない。gopls の語彙にその区間を表す信号がないため、写像は `ready` のまま返す。これは gopls 側が本プロトコルを話すことでしか埋まらない
- 7.3 は `didChange`（インメモリ）で測った。Claude Code のようにディスク書き込みで編集するクライアントでは `didChangeWatchedFiles` 経路になり、別途の観測が要る
