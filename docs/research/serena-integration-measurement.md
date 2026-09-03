# Serena を下流の被験者にした観測（2026-09-03、M7）

ADR 0010 の M7。Serena（solidlsp）の言語サーバー起動コマンドを設定だけで lsp-det 経由に向け、Serena 自身の readiness 判定・打ち切り時間と lsp-det の下流側の保留がどう重なるか、そして Serena が自前で持つサーバー別の補正コードを本プロトコルがどこまで置き換えるかを測った。

## 結論

| 項目                                          | 結果                                                                                                                                                                                                                                                                                                          |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 設定だけで lsp-det を挟めるか                 | **できる**。`ls_specific_settings.<言語>.ls_base_cmd` に `["lsp-det", "--", "pyright-langserver", "--stdio"]`（TypeScript は `["lsp-det", "--", "typescript-language-server", "--stdio"]`）。Serena 側のコード変更なし                                                                                        |
| Serena の readiness 待ちとの重なり（pyright） | Serena は `window/logMessage` の "Found N source files" を正規表現で待つ（上限 60 秒）。lsp-det はログを原文のまま流すので Serena の待ちはそのまま成立し、lsp-det の `ready` と同じ瞬間（0.278 秒）に解ける。references は 2 箇所（`b.py` の import と呼び出し）                                              |
| 同（typescript-language-server）              | Serena は `$/typescriptVersion` で "ready"、`$/progress` のトークンが空になるまで "indexing" を待つ（上限 10 秒 / 30 秒）。ファイルを開くまで progress は出ないので Serena の待ちは即座に解け、references の際に Serena が自分でファイルを開いてからロードを待つ。lsp-det の保留はその間に効き、結果は 2 箇所 |
| tsserver のクラッシュ（lsp-det なし）         | Serena の新しい検知（`_TSSERVER_EXITED_PATTERN`、PR #1848）は WARNING を出すが、直後の `request_references` は **0 件を成功として返す**（検知が効くのは `wait_for_indexing` の中だけ）                                                                                                                        |
| 同（lsp-det あり）                            | lsp-det が health `error` にし、`request_references` は `SolidLSPException`（"caused by lsp-det: the language server reports health: error ([tsserver] Exited. Code: null. Signal: SIGKILL) (-32803)"）になる。**空応答の嘘が消える**                                                                         |

## 測定環境

- Serena: `reference/serena`（`7fcbca7`、serena-agent 1.7.1.dev0）を `uv run --frozen` で実行。solidlsp を直接呼ぶ Python スクリプト（`SolidLanguageServer.create` → `start_server_context` → `request_references`）。MCP は挟んでいない
- lsp-det: main `1ba4bae`（M6 完了）の release ビルド。pyright 1.1.412、typescript-language-server 5.3.0 + TypeScript 5.9.3（flake.nix）
- fixture: Python は `a.py`（`def target()`）と `b.py`（import と呼び出し）、TypeScript は `tsconfig.json` と `a.ts` / `b.ts`（同じ形）

## 時系列

pyright（lsp-det 経由、`start_server_context` 開始を 0 とする）:

| 時刻   | 出来事                                                                                                                       |
| ------ | ---------------------------------------------------------------------------------------------------------------------------- |
| 0.079s | lsp-det: 起動ログで pyright 1.1.412 の写像を選び `{completeness, freshness}` を宣言。`initialize` 応答に `serverInfo` はない |
| 0.081s | Serena: "Waiting up to 60.0s for Pyright to complete initial workspace analysis..."                                          |
| 0.142s | Serena: "Pyright workspace scanning complete"（"Found 2 source files" を正規表現で検出）。同時に lsp-det: `{unknown, ready}` |
| 0.149s | `start_server_context` が返る                                                                                                |
| 2.154s | `request_references("a.py", 0, 4)` → 2 箇所（`b.py` の 0 行目と 3 行目）。2 秒は Serena 側の処理                             |

typescript-language-server（同）:

| 時刻   | 出来事                                                                                                              |
| ------ | ------------------------------------------------------------------------------------------------------------------- |
| 0.051s | lsp-det: 起動ログで typescript-language-server（TypeScript 5.9.3）の写像を選び保証を宣言                            |
| 0.052s | Serena: "TypeScript server is ready" → "TypeScript project indexing complete"（開いたファイルがなく progress なし） |
| 0.058s | `start_server_context` が返る                                                                                       |
| 0.171s | `request_references` の中で Serena がファイルを開く → lsp-det: `{unknown, indexing}`                                |
| 0.332s | lsp-det: `{ok, ready}`。Serena: "TypeScript cross-file indexing complete"                                           |
| 0.355s | references → 2 箇所                                                                                                 |

クラッシュ（lsp-det 経由）: references の後に tsserver の 2 プロセスへ SIGKILL → 0.01 秒後に lsp-det が `{error, ready}`（message は Exited のログ）、Serena も同じログを見て WARNING → 直後の `request_references` は lsp-det の RequestFailed（-32803）で `SolidLSPException`。lsp-det なしでは同じ手順で **0 件、例外なし**。

## Serena の補正コードのうち本プロトコルが置き換えるもの

Serena のサーバー別コード（`src/solidlsp/language_servers/`）のうち、readiness と health の判定に当たる部分を数えた。lsp-det の写像がこれらの信号を仕様の値に写し、Serena が `experimental/serverState` と `serverStateChanged` を読めば、これらは不要になる。

| ファイル                                  | 範囲（行）                                                                                      | 内容                                                                                                                                                     | 行数（概算） |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| `pyright_server.py`（243 行）             | 132〜175、236〜242                                                                              | "Found N source files" の正規表現、`pyright/*Progress` の追跡、60 秒の打ち切り                                                                           | 約 55        |
| `typescript_language_server.py`（610 行） | 71〜248、`_start_server`（391〜531）内の `$/typescriptVersion` と `$/progress` の処理、558〜607 | クラッシュ検知（`_TSSERVER_EXITED_PATTERN`、`TypeScriptServerCrashedError`）、indexing の待ち（10 秒 / 30 秒 / 猶予）、cross-file 用の事前オープンと待ち | 約 230       |

置き換えないもの: 依存の解決（`DependencyProvider`）、`initialize` の params、ファイル種別の判定。これらは readiness と無関係である。

## 一般化してはならない点

- 2 ファイルの fixture なので、Serena の打ち切り（60 秒 / 10 秒 / 30 秒）に lsp-det の保留が掛かる場面は測っていない。大規模ワークスペースでは、Serena の待ちが先に打ち切られて "proceeding anyway" のまま references が lsp-det に届き、lsp-det が `ready` まで保留する、という順になる（Serena の待ちは lsp-det の保留を妨げない。逆も同じ）。その場合 Serena 側の `DEFAULT_LS_REQUEST_TIMEOUT` に掛かる可能性があり、要観測
- Serena は本プロトコルを宣言しないので、下流側が代行している。Serena が `experimental.serverState` を宣言すれば代行は止まり（ADR 0002 決定 3）、Serena 自身が状態を読んで待つことになる。その実装は Serena 側の作業
- クラッシュの比較は typescript-language-server だけ。pyright のクラッシュは接続の終了で伝わり、Serena も lsp-det も同じものを見る
- Serena の行数は「readiness / health に関わる部分」を目視で切った概算で、正確な差分は Serena に PR を出すときに出る
