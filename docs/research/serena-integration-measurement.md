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

## 上流 HEAD での再測定（2026-09-09、提出前の準備 6）

`docs/upstream-submissions.md` の準備 6。上の観測（`7fcbca7`、2026-08-20 の oraios/serena#1848 のマージ）から上流は 29 コミット進んだ。提出する不具合が HEAD に残っているか、oraios/serena#1978（2 回目以降の横断要求でも進行中の indexing を待つ）と oraios/serena#1988（言語サーバーの registry）が何を変えるかを確かめた。

### 方法

- Serena: `reference/serena` を upstream HEAD `701e7c84`（2026-09-08、oraios/serena#1998 のマージ。`serena-agent` 1.7.1.dev0）に進めた。最新リリース v1.7.0（2026-08-09）は前回の測定の起点より古いので測っていない。#1978（`85fe34a6`、OPEN）と #1988（`2f7946de`、OPEN）はブランチを取り、#1978 はクラッシュの手順を同じ probe で動かし、#1988 はコードを読んだ
- lsp-det: main `2b0e84e`（0.7.0）の release ビルド。pyright 1.1.412、typescript-language-server 5.3.0 + TypeScript 5.9.3（flake.nix）。fixture と `scripts/serena/probe.py` は上と同じ

### 結果

| 手順                            | 前回（`7fcbca7`）                               | HEAD（`701e7c84`）                                                                       | #1978                                 |
| ------------------------------- | ----------------------------------------------- | ---------------------------------------------------------------------------------------- | ------------------------------------- |
| pyright、lsp-det 経由           | `ready` 0.142 s、references 2 箇所（2.154 s）   | `ready` 0.147 s、2 箇所（2.158 s）                                                       | 対象外                                |
| tsls、lsp-det 経由              | `indexing` 0.171 s → `ready` 0.332 s、2 箇所    | `indexing` 0.113 s → `ready` 0.267 s、2 箇所（0.287 s）                                  | 対象外                                |
| tsls のクラッシュ、lsp-det 経由 | `{error, ready}`、`SolidLSPException`（-32803） | 同じ（`{error, ready}` 0.415 s、references を -32803 で拒否）                            | 同じ                                  |
| tsls のクラッシュ、lsp-det なし | **0 件を成功として返す**                        | **同じ**（WARNING "tsserver reported an abnormal exit" の後、0.886 s に 0 件、例外なし） | **同じ**（0.908 s に 0 件、例外なし） |

oraios/serena#1978 でも変わらない理由: #1978 は latch（`_has_waited_for_cross_file_references`）の後の要求で `_active_progress_tokens` が空でなければ `wait_for_indexing` を呼ぶ。tsserver が落ちると typescript-language-server は進行中の token に end を送るので、次の要求の時点で集合は空、待ちに入らず、`_crash_message` を見る `_raise_if_crashed`（HEAD では `wait_for_indexing` と `_wait_for_indexing_start_or_completion` の 3 箇所、`typescript_language_server.py:163, 183, 192`）を通らない。

### 残る不具合（HEAD での所在）

| 不具合                                                     | HEAD での所在                                                                                                                                                                                                                                                 | 変化           |
| ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------- |
| (a) クラッシュ検知が 2 回目以降の横断要求に効かない        | `_wait_for_cross_file_references_if_needed`（`typescript_language_server.py:576-588`）が latch で即 return。`_raise_if_crashed` は待ちの中だけ                                                                                                                | なし。実測は上 |
| (b-1) 打ち切りが素の `TimeoutError`                        | `ls_process.py:101-107`（`queue.Empty` → `TimeoutError`）。再起動と再試行は `SolidLSPException` の `is_language_server_terminated()` だけ（`tools_base.py:382-397`）。エージェントには "Tool execution timed out after N seconds."（`tools_base.py:430-433`） | なし           |
| (b-2) 書き込み失敗の握りつぶし                             | `ls_process.py:660-667`（`BrokenPipeError` / `ConnectionResetError` / `OSError` を `log.error` して return。応答待ちは残る）                                                                                                                                  | なし           |
| (b-3) 再 open の全文 `didChange` が `version` を増やさない | `ls.py:141-158`（`LSPConstants.VERSION: self.version` のまま。編集の `didChange` は `ls.py:1378, 1420` で増やす）                                                                                                                                             | なし           |
| (b-4) hover の予算切れをエージェントに伝えない             | `symbol.py:687-694`（予算超過で `info = None`、`log.debug` 1 回）。集計も `log.debug`（715-721）。既定 10 秒（`serena_config.py:944`。docstring の "5s by default" は古い）                                                                                   | なし           |

### (a) の修正（fork の `tsserver-crash-on-request-path`）

`_wait_for_cross_file_references_if_needed` の冒頭、latch の前で `_raise_if_crashed()` を呼ぶ。1 行と、latch 後にクラッシュを観測した状態で例外になるテスト 1 件（`test_wait_for_cross_file_references_raises_after_the_latch_when_a_crash_was_observed`）、CHANGELOG の 1 項目。上流 HEAD で `test_typescript_timeout_policy.py` 30 件、`test/solidlsp/typescript` 20 件、ruff と ty が通る。probe の `CRASH=1 VIA_LSP_DET=0` は 0.880 s に `TypeScriptServerCrashedError`（"tsserver exited abnormally: … Signal: SIGKILL"）になる。#1978 の上に同じ 1 行を置いても成立する（32 件通過、probe は同じ例外）。同じ関数を触るので、提出は #1978 の帰趨を見てから rebase する。

`TypeScriptServerCrashedError` は `is_language_server_terminated()` が偽なので、この修正でも Serena は再起動しない（以後のツールは理由付きで失敗し続ける）。typescript-language-server 側の修正（typescript-language-server/typescript-language-server#1125）が通れば LS 本体が tsserver と一緒に落ちて `LanguageServerTerminatedException` の経路に乗り、Serena が再起動する。2 つは補い合う。

### #1988 の registry の形

- `LanguageServerRegistry`（`ls_config.py`）が組み込みの `LanguageServerId` と外部の `ExternalLanguageServerId(key, matcher, implementation, priority)` を 1 つの表に持つ。`implementation` は `SolidLanguageServer` の子クラス
- 外部の登録は Python パッケージの entry point（group `solidlsp.language_server_registration`）か、Serena を起動する自前のスクリプトからの `register()`。`allow_override=True` で既存の key を置き換えられる
- 登録した key は `project.yml` の `language_servers` にそのまま書ける。文書は `docs/03-special-guides/external_language_server_registration.md`

lsp-det を載せる形は「`SolidLanguageServer` の子クラスを持つ Python パッケージ」になる。組み込みの adapter（`PyrightServer` 等）を継承して起動コマンドを `lsp-det -- …` にし、readiness の待ちを `experimental/serverState` の読み取りに置き換えるものが 1 言語 1 クラス。提案の草案は `docs/upstream-submissions.md`。

### 一般化してはならない点

- fixture は前回と同じ 2 ファイル。上の「一般化してはならない点」はそのまま当てはまる
- #1978 と #1988 は OPEN で、マージ時に形が変わりうる。特に #1988 の `ExternalLanguageServerId` の引数と entry point の group 名はマージ後に読み直す
- (b) の 4 件はコードを読んで所在を確かめたもので、動かして測ったのは (a) だけ
