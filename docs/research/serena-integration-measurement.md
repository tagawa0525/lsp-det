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

`_wait_for_cross_file_references_if_needed` の冒頭、latch の前で `_raise_if_crashed()` を呼ぶ。1 行と、latch 後にクラッシュを観測した状態で例外になるテスト 1 件（`test_wait_for_cross_file_references_raises_after_the_latch_when_a_crash_was_observed`）、CHANGELOG の 1 項目。上流 HEAD で `test_typescript_timeout_policy.py` 30 件、`test/solidlsp/typescript` 20 件、ruff と ty が通る。probe の `CRASH=1 VIA_LSP_DET=0` は 0.880 s に `TypeScriptServerCrashedError`（"tsserver exited abnormally: … Signal: SIGKILL"）になりコード 0 で終わる（素の HEAD では "NO ERROR SURFACED" でコード 1。この終了コードが受け入れ条件）。#1978 の上に同じ 1 行を置いても成立する（32 件通過、probe は同じ例外）。lsp-det 経由でも同じ例外になる: lsp-det が流す `window/logMessage` を Serena 自身の検知が先に見て投げるので、要求は lsp-det に届かず、lsp-det の拒否（-32803）は Serena の検知が効かないときの受け皿になる。同じ関数を触るので、提出は #1978 の帰趨を見てから rebase する。

`TypeScriptServerCrashedError` は `is_language_server_terminated()` が偽なので、この修正でも Serena は再起動しない（以後のツールは理由付きで失敗し続ける）。typescript-language-server 側の修正（typescript-language-server/typescript-language-server#1125）が通れば LS 本体が tsserver と一緒に落ちて `LanguageServerTerminatedException` の経路に乗り、Serena が再起動する。2 つは補い合う。

### #1988 の registry の形

- `LanguageServerRegistry`（`ls_config.py`）が組み込みの `LanguageServerId` と外部の `ExternalLanguageServerId(key, matcher, implementation, priority)` を 1 つの表に持つ。`implementation` は `SolidLanguageServer` の子クラス
- 外部の登録は Python パッケージの entry point（group `solidlsp.language_server_registration`）か、Serena を起動する自前のスクリプトからの `register()`。`allow_override=True` で既存の key を置き換えられる
- 登録した key は `project.yml` の `language_servers` にそのまま書ける。文書は `docs/03-special-guides/external_language_server_registration.md`

lsp-det を載せる形は「`SolidLanguageServer` の子クラスを持つ Python パッケージ」になる。組み込みの adapter（`PyrightServer` 等）を継承して起動コマンドを `lsp-det -- …` にし、readiness の待ちを `experimental/serverState` の読み取りに置き換えるものが 1 言語 1 クラス。提案の草案は `docs/upstream-submissions.md`。

### 一般化してはならない点

- fixture は前回と同じ 2 ファイル。上の「一般化してはならない点」はそのまま当てはまる
- 再測定の時点（2026-09-09）で #1978 と #1988 は OPEN だった。#1988 は 2026-09-12 21:16 UTC にマージされた（`403ad0a5`。「(b-2) の実測」の節）。マージされた形は上の「#1988 の registry の形」のとおりで、`ExternalLanguageServerId` の引数と entry point の group 名は変わっていない。#1978 は OPEN のまま
- (b) の 4 件はコードを読んで所在を確かめたもの。動かして測ったのは (a) と、oraios/serena#2004 の問い返しを受けて測った (b-2)（下の節）だけ

## (b-2) の実測（2026-09-14 UTC、JST では 09-15。oraios/serena#2004 の問い返しへの答え）

opcode81 が #2004 に「実際にどう遭遇したのか。サーバーが本当にいないなら読み取りスレッドが検知して要求をキャンセルする」と問い返した（2026-09-14）。(b-2) は読んで見つけたもので動かしていなかったので、測った。道具は `scripts/serena/dead-write-probe.py`。測った checkout は `3bed94f3`（#2007 の枝）で、上流 main の HEAD `403ad0a5`（#1988 のマージコミット、2026-09-12 21:16 UTC）との `ls_process.py` の差分は #1988 による `ls_id` の型名と `get_key()` への置き換え 9 行だけ。以下の行番号は `403ad0a5` のもの。

### 方法

solidlsp 直接（lsp-det なし）、要求の打ち切りを 8 秒に設定（`SolidLanguageServer.create(..., timeout=8)`。Serena の既定は 235 秒で、窓の長さがそれに比例するだけ）。pyright の版: 最初の実行と #2004 への返信は `ls_base_cmd` を設定しておらず、Serena の既定の dependency provider が `uvx --from pyright==1.1.403` を起動していた（返信に書いた 1.1.412 は誤り。PR #107 のレビューの指摘）。checkout 済みの probe は `ls_base_cmd` で PATH の `pyright-langserver`（1.1.412）を指定し、再実行の結果は同じ（下）。機構は solidlsp 側なので pyright の版は結果に関わらない。`request_references` を 1 回 → 保留中の要求がない状態で pyright のプロセス（自分の子孫全部）を SIGKILL → stdout の読み取りスレッド（`LSP-stdout-reader:python`）の終了を join で待ち、そのキャンセルのログが "Cancelling 0 pending" だったことを確かめる（sleep では、読み取りスレッドの終了が遅れたときに要求 #2 が保留中に登録されてキャンセルされる競合が残る）→ `request_references` をもう 1 回。fixture は `a.py`（`def target()`）と `b.py`（import と呼び出し）。

### 結果（最初の実行。kill の後に 1 秒 sleep してから要求 #2 を送る版の probe。#2004 への返信に載せた数字はこれ）

| 時刻    | 出来事                                                                                                                                                    |
| ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 4.21 s  | `references` #1 → 2 箇所                                                                                                                                  |
| 4.34 s  | SIGKILL（保留なし）                                                                                                                                       |
| 4.53 s  | 読み取りスレッドが終了し、"Cancelling 0 pending language server requests"（`ls_process.py:629` → `326-335`。キャンセルはこの一度だけ）                    |
| 5.34 s  | `ls.is_running()` は False                                                                                                                                |
| 5.53 s  | `references` #2: "Failed to write to stdin: [Errno 32] Broken pipe" が 2 回（`didOpen` と要求本体。`ls_process.py:664-667` で `log.error` して return）   |
| 13.34 s | 素の `TimeoutError`（"Request timed out (timeout=8.0)"。打ち切りいっぱい）。`SolidLSPException` ではないので `tools_base.py:383` の再起動の判定に届かない |

同期版（checkout 済みの probe。sleep の代わりに読み取りスレッドの join と "Cancelling 0 pending" の確認。PATH の pyright 1.1.412）での再実行は、2.46 s SIGKILL → 2.46 s 読み取りスレッド終了（kill から 8 ms）と "Cancelling 0 pending" → 要求 #2（"Failed to write to stdin" が 2 回）→ 10.47 s 素の `TimeoutError`（8.00 s）。1 秒の間が消えただけで形は同じ。

### 読み

- 相手の主張は、プロセスが死んだ時点で要求が保留中の場合には正しい。読み取りスレッドが `_cancel_pending_requests` でその要求に `LanguageServerTerminatedException` を配る。#2004 の元の文面はこの場合まで「打ち切りまで待つ」と読めたので、返信で狭めた
- キャンセルは読み取りスレッドが終わる瞬間の一度きり。その後に `_send_request_once`（`ls_process.py:337-349`）で登録された要求は誰も失敗させない。保留が空のときに死に、同じツール呼び出しの中で次の要求を送る場面（`include_info` 付きの `find_symbol` の hover のループ、`open_file` → 要求の並び）で起きる。`_ensure_functional_ls`（`ls_manager.py`）はツール呼び出しの間でしか `is_running()` を見ない
- 直す場所は書き込みの失敗か、`_send_request_once` の冒頭の `is_running()`。どちらでも要求を `LanguageServerTerminatedException` で失敗させれば再起動の経路（`tools_base.py:383-389`）に乗る。probe の受け入れ条件はこれ（要求 #2 が打ち切りの前に `is_language_server_terminated()` の真な `SolidLSPException` になること。直し方は問わない。読み取りスレッドのキャンセルが #2 を拾う競合は、kill の後のキャンセルが "0 pending" 1 回だったことの検査で除く）。第三者の [oraios/serena#2030](https://github.com/oraios/serena/pull/2030) がまさにこの形（下）
- 返信の文面は [../upstream-submissions.md](../upstream-submissions.md) の「Serena: 提出後の反応（2026-09-15）」

### #2030 の枝での結果

[oraios/serena#2030](https://github.com/oraios/serena/pull/2030)（feiiiiii5、2026-09-13 01:42 UTC、OPEN。`_send_payload` の書き込み失敗で `_cancel_pending_requests(LanguageServerTerminatedException("Stdio send error", self.ls_id, cause=e))` を呼ぶ。偽の stdin のテスト 3 件付き）の head `6e9d8dcc` を checkout して同じ probe（PATH の pyright 1.1.412）を走らせると、要求 #2 は 0.00 秒で `SolidLSPException`（`is_language_server_terminated()` が真、原因 `LanguageServerTerminatedException`）になり、コード 0 で終わる。書き込み失敗のたびにキャンセルが走る（`didOpen` で "Cancelling 0 pending"、要求本体で "Cancelling 1 pending"）。

### 帰趨（2026-09-14 15:54 UTC）

返信の 30 分後に opcode81 が "Not an actual issue. Closing." で #2004 を閉じた。窓（保留が空のときに死に、同じツール呼び出しの中で次の要求を送る）は実測どおり存在するが、実運用で踏む頻度は低いという判断と読む。反論はしない。#2030 は第三者の PR なので、その帰趨も相手に任せる。

## (b-1) の実測（2026-09-14、oraios/serena#2003 の反論への答え）

opcode81 が #2003 に「打ち切りで再起動の経路に乗せるのは筋が通らない。打ち切りは待つと決めた時間を超えただけで、大きなコードベースでは正常でありうる。実際に何が起きたのか」と反論した（2026-09-14 15:51 UTC）。(b-1) も読んで書いたもので、題名（"bypasses the language-server restart path"）は打ち切りを再起動の根拠にするように読め、その読みでは相手が正しい。返信の前に、応答が打ち切りより遅れたときに実際に何が起きるかを測った。道具は `scripts/serena/delay-proxy.py`（指定したメソッドの要求は即座にサーバーへ渡し、その**応答**だけを止めて他は素通しする stdio プロキシ）と `scripts/serena/timeout-probe.py`。checkout は `3bed94f3`、行番号は `403ad0a5`。返信に載せた最初の実行は、要求をサーバーへ渡す**前**に止める版のプロキシだった。その版ではサーバーが要求を受け取っていないので「サーバーで計算が続く」は測れておらず（PR #107 のレビューの指摘）、応答側で止める版に作り直して測り直した。クライアントから見える時系列は両版で同じ: 打ち切りの時刻、`$/cancelRequest` が来ないこと、遅れた応答が 10.47 s に届いて `_pending_requests` から消えることまで一致する（旧版でも 8 秒後に要求を流して pyright が答えるので、遅れた応答は届く）。違うのは、打ち切りの時点で pyright が要求を受け取って答え終えているか（新版）、まだ受け取っていないか（旧版）だけ。

### 方法

pyright 1.1.412、solidlsp 直接、要求の打ち切り 5 秒（`SolidLanguageServer.create(..., timeout=5)`）。`ls_base_cmd` を `python delay-proxy.py --delay 8 -- pyright-langserver` にし、プロキシが `textDocument/references` の要求を即座に pyright へ渡し、pyright の応答（4 ms で返る）を 8 秒止めてからクライアントに流す。`request_references` → 打ち切り → 遅れた応答の到着を `_pending_requests` の変化で捉える → `request_references` をもう一度 → `request_document_symbols`。

### 結果

| 時刻    | 出来事                                                                                                                                                                                                                                                                                        |
| ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0.47 s  | `request_references` #1 開始。既定の latch（`ls.py:1624-1628`）が `sleep(2)` してから送信。`PyrightServer` は `_get_wait_time_for_cross_file_referencing` を上書きしていない                                                                                                                  |
| 2.47 s  | プロキシが id=2 を pyright へ渡し、pyright は 4 ms で応答。プロキシがその応答を保持                                                                                                                                                                                                           |
| 7.47 s  | 素の `TimeoutError`（"Request timed out (timeout=5.0)"。2 + 5 秒）。`is_running()` は True。`$/cancelRequest` はプロキシに届かない（`cancel_request` は `lsp_requests.py:557` に定義があるだけで呼び出しがない）。`_pending_requests` に id=2 が残る（`get_result` の `Empty` は pop しない） |
| 10.47 s | プロキシが応答を解放 → 遅れた応答が id=2 を pop し、誰も読まない `Request` のキューに `on_result`（`ls_process.py:416-425`）。ログは出ない。#2003 に書いた「未知の id として捨てられる」は、`Request` が残っているので当たらない                                                              |
| 15.47 s | `request_references` #2 も打ち切り（5.00 秒。latch は済み）。直後の `request_document_symbols` は 0.00 秒で成功                                                                                                                                                                               |

### 読み

- 打ち切りはサーバーの健全性について何も語らない。相手の主張のとおりで、再起動の話は取り下げた
- 測れた事実は 2 つで、どちらも小さい。(1) 打ち切った後も `$/cancelRequest` を送らない（プロキシに届かない）。LSP では任意の機能。(2) 放棄した `Request` は応答が来るまで `_pending_requests` に残り、応答が来れば静かに消える
- 測れていないこと。(a) 打ち切った要求の計算がサーバー側で続くか。pyright は 2 ファイルの fixture では 4 ms で答えるので、続く計算がそもそもない。「cancel を送らない以上、サーバーは止める術がない」は LSP の仕組みからの推論で、この実測の結論ではない。#2003 に出した文面はこの推論を "keeps running in the server" と事実のように書いていて、言い過ぎだった（投稿済みのコメントを訂正済み。2026-09-14 16:26 UTC）。(b) 永久に答えないサーバーで `_pending_requests` の項目が残り続けるか。プロキシは 8 秒後に流すので測っていない（ソースからは、pop するのは応答の到着だけ）
- ツール層では素の `TimeoutError` は `tools_base.py` の `except Exception` で `ToolCallError("TimeoutError: Request timed out (timeout=235.0)")` になる。#2003 に書いた "Tool execution timed out after N seconds" は外側の `tool_timeout`（240 秒）の方で、内側の `ls_timeout`（235 秒）が先に来るので通常は出ない。これも不正確だった
- 副産物: 起動時に "Found N source files" を待つ pyright でも、最初の横断要求は latch の `sleep(2)` を払う。起動時の待ちと要求経路の待ちが繋がっていないことの実例で、#1988 の issue の材料
- 返信して not planned で閉じた（2026-09-14 16:10 UTC 頃。[コメント](https://github.com/oraios/serena/issues/2003#issuecomment-5667011948)。文面は [../upstream-submissions.md](../upstream-submissions.md)）

### 一般化してはならない点

- pyright 1 つ、fixture 2 ファイル。`_send_payload` も `_send_request_once` も言語に依らない同じ関数だが、typescript-language-server では動かしていない
- 「保留中に死ぬ」場合は測っていない（ソースから読める）。(b-1) では、本当に固まった（永久に答えない）サーバーも、打ち切り後も計算が続くサーバーも作っていない。プロキシは答え終わった応答を 8 秒止めるだけ
- MCP は挟んでいない。要求の打ち切り（`ls_timeout`）はツール層の打ち切りから 5 秒引いたもの（`project.py:509`、`ls_timeout = tool_timeout - 5`。既定 240 → 235）なので、実運用で先に来るのは要求の打ち切り
