# pyright の readiness と名乗りの実測（2026-09-03）

M5（pyright の写像、ADR 0010）の前提を実サーバーで確かめた。結果は ADR 0010 の前提と 2 点で食い違い、ADR 0011 で写像の選択方法と readiness の信号を決め直した。本文書はその根拠である。

## 結論

| 項目                       | ソースの読み                                                                                                                                                      | 実測                                                                                                                                                                |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `serverInfo`               | `InitializeResult` は `capabilities` だけ（`languageServerBase.ts` の `initialize()`）                                                                            | pyright は **`serverInfo` なし**。basedpyright は `{"name":"basedpyright","version":"1.39.8"}`                                                                      |
| 名乗りに相当する信号       | コンストラクタで `console.info` に `${productName} language server ${version} starting`（`languageServerBase.ts:231`）。設定の読み込み前なので抑制されない        | 最初の通知 `window/logMessage`（type 3）"Pyright language server 1.1.412 starting" / "basedpyright language server 1.39.8 starting"。`initialize` 応答より先に届く  |
| 起動直後の横断リクエスト   | references は追跡中のファイル一覧 `getSourceFileInfoList()` を走査する（`referencesProvider.ts:432`）。一覧はタイマーで少しずつ列挙される（`service.ts:627-640`） | 3001 ファイルで、`initialize` 直後の references は **0 件**（期待 3000 ファイル分）。「無言の嘘」そのもの                                                           |
| 列挙完了の信号             | `SourceEnumerator._finish()` が `console.info` に "Found N source files" または "No source files found."（`sourceEnumerator.ts:305-308`）                         | "Found 3001 source files" の後に投げ直した references は全件（6000 箇所 = import と呼び出し）                                                                       |
| `$/progress` の意味        | 解析待ちファイル数が 0 になったときに end（`languageServerBase.ts:1396`）。解析は開いたファイルが対象（既定の `checkOnlyOpenFiles`）                              | 2 ファイルの fixture では起動から 40 秒で一度も出ない。references の実行中に title "Finding references" の begin / end が出る                                       |
| 再列挙の開始               | `console.log` に "Searching for source files"（`sourceEnumerator.ts:83`）。既定の logLevel は Info（`languageServerBase.ts:393`）で、log レベルは送られない       | 既定設定では観測できない                                                                                                                                            |
| 複数ワークスペースフォルダ | フォルダごとに `AnalyzerService` を作り "Starting service instance \"name\"" を info に出す。列挙もフォルダごと                                                   | 3 フォルダで "Starting service instance" が 3 回、完了ログ（"Found N" または "No source files found."）がフォルダごとに 1 回。空フォルダは "No source files found." |

## 測定環境

- pyright 1.1.412、basedpyright 1.39.8（どちらも nixpkgs、flake.nix で固定）。起動は `pyright-langserver --stdio` / `basedpyright-langserver --stdio`
- 経路: 偽クライアント（Python スクリプト、スレッド化した読み手）→ 言語サーバー直結。lsp-det は挟んでいない（写像がまだないため）
- `initialize` は `rootUri` と `workspaceFolders` を渡し、`window.workDoneProgress: true` を宣言
- fixture: `a.py` に `def target()`、`m0000.py`〜`m2999.py` が `from a import target` と `target()` の呼び出しを持つ（3001 ファイル）

## 時系列（3001 ファイル、`initialize` 送信を 0 とする）

| 時刻   | 出来事                                                                                            |
| ------ | ------------------------------------------------------------------------------------------------- |
| 0.080s | `window/logMessage` "Pyright language server 1.1.412 starting"                                    |
| 0.082s | `window/logMessage` "Starting service instance \"pybig\""、`initialize` 応答（`serverInfo` なし） |
| 0.082s | `initialized`、`didOpen a.py`、references #1 を送る                                               |
| 0.144s | 設定のログ（"No include entries specified" 等）                                                   |
| 0.272s | references #1 の応答: **0 件**。同じ瞬間に "Found 3001 source files"。references #2 を送る        |
| 0.481s | references #2 の応答: 6000 件                                                                     |

## 一般化してはならない点

- 0 件になる窓（約 0.2 秒）は fixture の規模と本機の速さでの値。列挙はタイマーで分割実行されるので（`maxAnalysisTime.noOpenFilesTimeInMs`）、大規模ワークスペースでは長くなる
- 単一スレッドで 1 バイトずつ読む計測スクリプトでは最後のフォルダの完了ログが届かないように見えた。スレッド化した読み手では全フォルダ分が届いたので、前者は計測側の問題である。複数フォルダの規則（ADR 0011）はスレッド化した読み手の結果に基づく
- 7.2 / 7.3 の通過は本文書では測っていない。M5 の準拠テスト（`tests/conformance.rs` の `pyright_*` ignored）で測り、通った版だけ `TESTED_VERSIONS` に載せる
- クライアントが `logLevel` を Warning 以上に設定すると "Found" も届かない。Claude Code の公式プラグインは設定を送らず、Serena の `initializationOptions` にも `logLevel` はない（`pyright_server.py`）。他のクライアントでは要確認
