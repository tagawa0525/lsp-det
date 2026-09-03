# Serena が MCP ツールと LSP の間で行う処理（2026-09-04）

調査対象: `reference/serena`（oraios/serena、clone は `7fcbca7`、2026-08-20）。上流 `main` の 2026-09-03 までの 21 コミットも見た。引用パスは `reference/serena/` からの相対、`:N` は行番号。

先行調査（[serena-solidlsp.md](serena-solidlsp.md) は言語別の readiness 判定とシンボル補正、[serena-initialize-caps.md](serena-initialize-caps.md) は `initialize` の capability、[serena-integration-measurement.md](serena-integration-measurement.md) は lsp-det を挟んだ実測）が扱わなかった、**ツール呼び出しから LSP リクエストまでの間と、応答からツール結果までの間**の処理を全数で読んだ。lsp-det に足す機能の候補は 8 章に置く（決定ではない。採るなら ADR）。

## 要約

- Serena は「編集を織り込まない応答」への対策を自前で持つ。ツールごとに全ソースファイルの mtime を走査し `workspace/didChangeWatchedFiles` を送り、新規ファイルは pyright のために open / close までする（2 章）。本プロトコルの `freshness` は受信済みの `didChange` しか対象にしていない
- 「インデックス未完了の空応答」への対策は 3 つ。初回の横断リクエスト前の固定 2 秒 sleep、`documentSymbol` の空応答をキャッシュしない、`isIncomplete` の補完を 30 回まで再取得（6 章）。references が空でも再試行はせず、エージェントには `{}` が事実として返る（3 章）
- 「壊れたサーバーの成功風の応答」への対策はプロセスの生死だけ。`is_running()` が偽ならアクセスのたびに無条件で再起動し、ツール実行中の終了は 1 回だけ再試行する。tsserver の終了はログの正規表現で拾うが再起動には繋がらない（5 章）
- リクエストの打ち切りは 1 件ごとに `tool_timeout - 5` 秒（既定 235 秒）。打ち切りは素の `TimeoutError` で、再起動の経路に乗らない。`$/cancelRequest` はどこからも送られない（1 章）
- `workspace/symbol` はフィルタもキャッシュもなく、rust-analyzer には `limit: 128` を渡している（4 章）。本プロトコルの `completeness` の対象に `workspace/symbol` が入っていることと突き合わせる必要がある（8 章）
- 上流の直近 2 週間にも、Dart 向けの readiness 待ち（60 秒で打ち切り）と言語サーバーの子孫プロセスの回収が個別に足された。Dart の analysis server は rust-analyzer と同じ `experimental/serverStatus` を話す（7 章）

## 1. リクエストの送受信

| 項目                | 事実                                                                                                                                                                                                                                                                                       |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 打ち切り            | `DEFAULT_LS_REQUEST_TIMEOUT = 300.0`（`src/solidlsp/ls_process.py:39-42`、2026-08-01 に 300 秒へ）。Serena 本体は `ls_timeout = tool_timeout - 5` で上書きし、既定 `tool_timeout = 240` なので **235 秒**（`src/serena/project.py:503-509`、`src/serena/config/serena_config.py:890`）     |
| 打ち切りの見え方    | `queue.Empty` → 素の `TimeoutError("Request timed out")`（`ls_process.py:102-108`）。`SolidLSPException` ではないので、後述の再起動と再試行の経路（`SolidLSPException` だけを捕まえる）に乗らない。エージェントには "Tool execution timed out after N seconds."（`tools_base.py:424-433`） |
| エラー応答          | `SolidLSPException(cause=LSPError)`（`ls_process.py:385`）。`InternalError`（-32603）は "This often occurs when requesting a symbol in a way the language server cannot resolve." の `RuntimeError` に読み替える（`ls.py:1487-1494`）                                                      |
| 再試行              | `ContentModified`（-32801）だけ、最大 3 回・0.2 秒間隔。対象メソッドは自分が `initialize` で宣言した `general.staleRequestSupport.retryOnContentModified` を読み返して決める（`ls_process.py:44-53, 373-383`、`ls.py:3247-3256`）。rust-analyzer では semanticTokens 3 種と hover          |
| キャンセル          | `$/cancelRequest` は定義だけあり（`lsp_protocol_handler/lsp_requests.py:557`）、**どこからも呼ばれない**。打ち切られたリクエストはサーバー側で生き続け、遅れて届いた応答は id 不明として捨てられる（`ls_process.py:421-423`）                                                              |
| 書き込み失敗        | `BrokenPipeError` 等はログして握りつぶす（`ls_process.py:664-667`）。応答待ちは残るので、読み取りスレッドが先に死んでいると打ち切りまで待つ                                                                                                                                                |
| `shutdown` / `exit` | `params` を省く（HLS と rust-analyzer が `params: {}` を拒む）。他のメソッドは `None` を `{}` にする（Delphi / FPC 対策、PR #851。`lsp_protocol_handler/server.py:100-117`）                                                                                                               |
| 応答の突き合わせ    | 整数 id。文字列で返すサーバーのために `isdigit()` なら `int` で再検索（`ls_process.py:415-419`）                                                                                                                                                                                           |

## 2. 文書の同期と編集の反映

| 項目                   | 事実                                                                                                                                                                                                                                                                                                                                                                                                                             |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `didOpen` / `didClose` | `open_file` の参照カウントで管理。初回 open で全文の `didOpen`、カウントが 0 に戻ったら `didClose`（`ls.py:122-168, 1280-1333`）                                                                                                                                                                                                                                                                                                 |
| 再 open 時の同期       | 既に開いているファイルを再 open するとき、ディスクの mtime が LS に渡した mtime と違えば **全文の `didChange`**（`range` なし）。`version` は増やさない（`ls.py:141-158`、2026-08-02）                                                                                                                                                                                                                                           |
| 編集                   | シンボル編集は `delete_text_between_positions` → `insert_text_at_position` の範囲付き `didChange` 2 回（`version` を増やす）で LS に伝え、その後にディスクへ書く（`ls.py:1358-1434`、`src/serena/code_editor.py:77-93, 295-308`）。**`didSave` は送らない**（capability では `didSave: true` を宣言している）。編集後の待機・再問い合わせ・検証はない。プロンプトも「再読不要」と指示（`resources/config/modes/editing.yml:25`） |
| 外部の編集の反映       | `LanguageServerFileChangeNotifier`（`src/serena/ls_manager.py:280-371`）が**全ソースファイルの mtime を走査**し、差分を 1 つの `workspace/didChangeWatchedFiles`（Created / Changed / Deleted）にして全サーバーへ送る。docstring: 外部の編集は「warm な言語サーバーには見えず、シンボリックな問い合わせが古いインデックスから答える」                                                                                            |
| 新規ファイル           | `didChangeWatchedFiles` だけでは不十分（「pyright で観測」）として、作成されたファイルは open / close を 1 回行い parse と bind を強制する（`ls_manager.py:358-369`）                                                                                                                                                                                                                                                            |
| 走査の呼び出し元       | `find_referencing_symbols`、`find_implementations`、`find_declaration`、`get_diagnostics_for_*`、`safe_delete_symbol` の冒頭（`src/serena/tools/symbol_tools.py:279-281, 370, 425, 509, 562`）。`find_symbol` と `get_symbols_overview` は「対象ファイルを自分で開くので不要」として呼ばない（`symbol_tools.py:57, 194`）                                                                                                        |
| 編集後の diagnostics   | 実装はあるが `ENABLE_DIAGNOSTICS = False`（「個々の編集は意図的に diagnostics を増やすことが多い」。`tools_base.py:466-519`）                                                                                                                                                                                                                                                                                                    |

## 3. `references` の経路

`request_references`（`ls.py:1713-1726`）は `SymbolLocationRequest.execute()`（`ls.py:1456-1476`）を通る。definition / implementation も同じ経路。

1. `_pre_open_for_cross_file_references()`（`didOpen` の前に indexing 追跡を武装するフック。基底は no-op）
2. 対象ファイルだけを `open_file`。他のファイルは開かない
3. `_wait_for_cross_file_references_if_needed()`: **インスタンスごとに一度だけ固定 sleep**、既定 2 秒（`ls.py:1042-1048, 1628-1633`）。docstring は「finished initializing の信号が信頼できない LS 向け」
4. `includeDeclaration: false` 固定で送る（`ls.py:1664-1671`）
5. 応答の各 location を `convert_location_item`（`ls.py:1500-1537`）で濾す: プロジェクト外は「LS がインストール済みパッケージや標準ライブラリを解析している。バグだが今は捨てる」の警告つきで捨てる。存在しないパスは捨てる。無視パス（`.git` 等の固定集合 + 非ソース拡張子 + gitignore）は捨てる
6. **空でも再試行しない**（`ls.py:1686-1696`）

ツール層（`find_referencing_symbols`、`symbol_tools.py:252-339`）はこれに加えて、参照ごとに参照元ファイルを open して `documentSymbol` を取り、包含シンボルを求める（`ls.py:2401-2422`）。見つからなければ「HORRIBLE HACK … SPECIFIC TO PYTHON」と自称する行テキストの `.` 分割（`ls.py:2424-2446`）、さらにファイル全体の `File` シンボルで代用する。参照が 0 件のときの結果は **`{}`** で、インデックスが冷えている可能性は伝えない。`safe_delete_symbol` は references が 0 件なら削除するので（`symbol_tools.py:698-738`）、削除の安全性は references の完全性に依存する。

## 4. キャッシュと `workspace/symbol`

- `documentSymbol` は 2 段のキャッシュ（生の応答と加工後）を `.serena/cache/<言語>/*.pkl` に持ち、キーは相対パス、有効性は**内容の md5**（`ls.py:555-563, 1845-1903, 1927-2057`）。ツール呼び出しのたびに保存（`tools_base.py:412-417`）
- **空または `None` の応答はキャッシュしない**。「LS がインデックスやビルドを終えていないときに起きる（Lean 4 の `lake build` 前など）。キャッシュすると準備完了後も古いデータを返し続ける」（`ls.py:1893-1898`）
- `find_symbol` はプロジェクト全体ならディレクトリを自分で歩き、ファイルごとに `documentSymbol`（キャッシュ経由）。`workspace/symbol` は使わない（`ls.py:2059-2140`、`src/serena/symbol.py:735-765`）
- `request_workspace_symbol`（`ls.py:3102-3127`）は例外的に `open_file` もキャッシュもフィルタもなし。rust-analyzer には `initializationOptions` で `workspace.symbol.search = {kind: only_types, limit: 128, scope: workspace}` を渡す（`language_servers/rust_analyzer.py:681`）
- `serena project index` は documentSymbol をファイルごとに取るだけ（`src/serena/cli.py:802-843`）

## 5. プロセスの生死と再起動

- 起動はプロジェクト有効化の直後に非同期。言語ごとに 1 サーバーを並列に起動し、1 つでも失敗すれば全部止めて `LanguageServerManagerInitialisationError`（`src/serena/ls_manager.py:99-157`）。最初のツール呼び出しは同じ直列キューで起動の後ろに並ぶ（`tools_base.py:426-427`、`task_executor.py:198-220`）
- 生死判定は **`is_running()` = プロセスの `returncode is None`** だけ（`ls_process.py:515-516`）。stderr は記録のみで検知には使わない（`ls_process.py:631-651`）
- サーバーへのアクセスのたびに `_ensure_functional_ls` が `is_running()` を見て、偽なら回数制限も待ち時間もなく新しいインスタンスを同期的に作る（`ls_manager.py:159-163, 183-201`）
- ツール実行中に `SolidLSPException.is_language_server_terminated()` なら再起動して **1 回だけ**再試行（`tools_base.py:380-397`）。判定は `cause` が `LanguageServerTerminatedException` かどうか（`ls_exceptions.py:22-39`）で、これは stdout 読み取りスレッドがプロセス終了を見たときに未応答のリクエストへ配られる（`ls_process.py:326-335, 625-629`）
- tsserver の終了（`_TSSERVER_EXITED_PATTERN`、PR #1848）は `TypeScriptServerCrashedError` になるが、`is_language_server_terminated()` は偽なので再起動しない。`restart_language_server` ツールは既定で無効（`symbol_tools.py:25-33`）
- 終了は `shutdown` + `exit` を別スレッドで 2 秒だけ待ってから `terminate`（`ls_process.py:253-268, 546-562`）。上流 `24940a15`（#1918、2026-09-03）で子孫プロセスの回収が足された
- 上流 `dc59a893`（2026-09-02）で、起動途中の例外でサブプロセスが残る不具合が直された

## 6. 時間に基づく判定の一覧

| 場所                                                                         | 値                             | 何が起きるか                                                       |
| ---------------------------------------------------------------------------- | ------------------------------ | ------------------------------------------------------------------ |
| 初回の横断リクエスト前の sleep（`ls.py:1628-1633`）                          | 既定 2 秒（言語別に 0〜15 秒） | 一度だけ                                                           |
| `isIncomplete` の補完（`ls.py:1782-1791`）                                   | 30 回まで再取得                | まだ不完全なら `[]`                                                |
| diagnostics の待ち（`ls.py:915-961`）                                        | 2.5 秒                         | 世代カウンタで「新しい publish」を待つ。来なければキャッシュ       |
| hover の予算（`src/serena/symbol.py:632-726`）                               | 10 秒                          | 超えた残りのシンボルは `info = None`。**エージェントには伝えない** |
| typescript-language-server の待ち（`typescript_language_server.py:101-110`） | 10 秒 / 30 秒 / 猶予 5 秒      | 超えたら "proceeding anyway"                                       |
| Dart の初期解析待ち（上流 `e1322a3b`）                                       | 60 秒                          | 同上                                                               |
| ツールの打ち切り（`task_executor.py:143-145`）                               | 240 秒                         | 待つ側は `TimeoutError`、スレッドは走り続ける                      |
| 有効化コマンド（`agent.py:1299-1307`）                                       | 180 秒                         | terminate して続行                                                 |

## 7. clone 以降の上流の変更（2026-08-20 → 09-03、21 コミット）

- `e1322a3b` fix(dart): analysis server の `$/analyzerStatus`（`isAnalyzing: false`）**または `experimental/serverStatus`（`quiescent: true`）** を 60 秒まで待つ。Dart は rust-analyzer の語彙を話す 3 つ目のサーバー（仕様 10 章の対応表の候補）
- `24940a15` fix(process): 言語サーバーの子孫プロセスを回収する（#1918）
- `dc59a893` `LanguageServerManager.start` の例外でサブプロセスが残らないようにする
- `813fd98f` `project index-file` が該当言語のサーバーだけを使うようにする
- 他は設定・プラットフォーム判定・メモリ・Nix など

## 8. lsp-det への含意（候補。決定ではない）

Serena が自前で持つ処理のうち、「無言の嘘」の 3 類型に当たるものと、本プロトコルまたは lsp-det がまだ扱っていないものを突き合わせた。

| 候補                                                               | 根拠となる Serena の処理                                                                                                             | 現状の lsp-det / 仕様                                                                                                                                          | 必要な作業                                                                                                                                                                                                   |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| A. `freshness` の対象に `workspace/didChangeWatchedFiles` を加える | 2 章。ツールごとに mtime を走査して送る。新規ファイルは pyright のために open / close                                                | 仕様 6 章 2 項は受信済みの `didChange` だけ。ディスク上の編集（別ツール、git）は対象外                                                                         | 各サーバーで `didChangeWatchedFiles` 後の再インデックスが同期か非同期か（`indexing` に戻すべきか）を実測。7.3 に第 2 のテスト。ADR                                                                           |
| B. `workspace/symbol` の完全性                                     | 4 章。rust-analyzer に `limit: 128`（rust-analyzer 自身の既定も同値）                                                                | 7.0 の対象に `workspace/symbol` があり、rust-analyzer に `completeness` を宣言している。打ち切りは LSP に伝える語彙がない（`isIncomplete` は completion だけ） | 129 個以上のシンボルを持つ fixture で実測。打ち切られるなら、7.0 から外すか、`completeness` の定義（「後から増えない」）に打ち切りが含まれないことを明記する                                                 |
| C. `initializing` の間は 7.0 以外も保留する                        | 4 章。`documentSymbol` の空応答をキャッシュしない（Lean 4）。`serena project index` は起動直後に全ファイルの `documentSymbol` を取る | 下流側の保留は 7.0 だけ。仕様 3 章の `initializing` は「まだ何も答えられない」                                                                                 | まず写像が `initializing` を正しく使っているかを見る（pyright の列挙中は単一ファイルの要求に答えられるので `indexing` が正しい）。そのうえで実測                                                             |
| D. 回復しない `health: error` の代行                               | 5 章。再起動の引き金はプロセスの生死だけ。tsserver の終了は再起動に繋がらず、lsp-det 経由では以後のツールが理由付きで失敗し続ける    | 下流側は `error` の間 `RequestFailed` を返す。上流は生きたまま                                                                                                 | 写像が「回復しない」と知っている場合（typescript-language-server は tsserver を再起動しない）に、非対応クライアントの代行として上流を止めて接続を閉じ、クライアントの再起動に繋げるかを判断。設計 4.3 の変更 |
| E. 保留中であることを人間向けに知らせる                            | 1 章・6 章。235 秒で打ち切られ、理由のない "timed out" になる。`$/cancelRequest` は来ない                                            | 保留は stderr にしか出ない                                                                                                                                     | 非対応クライアントの代行中に `$/progress`（begin / end、title は保留の理由）を送るか。クライアントが `window.workDoneProgress` を宣言している場合のみ。仕様 6 章 4 項と整合                                  |
| F. diagnostics の phase（仕様 3 章の予約）                         | 6 章。世代カウンタで 2.5 秒待つ。編集後の diagnostics は雑音が多くて無効化                                                           | `phases` は予約のみ                                                                                                                                            | rust-analyzer の flycheck の progress、gopls の診断発行を信号にできるかの実測。長期                                                                                                                          |
| G. Dart の写像                                                     | 7 章。`experimental/serverStatus` を話す                                                                                             | 対応表にない                                                                                                                                                   | rust-analyzer の写像を名乗り `dart` で流用できるかの実測。安価                                                                                                                                               |

Serena 側の不具合として上流に出せるもの（lsp-det の機能ではない）:

- 打ち切りが素の `TimeoutError` で、再起動と再試行の経路（`SolidLSPException` のみ）に乗らない（1 章）
- 書き込み失敗を握りつぶし、読み取りスレッドの状態次第で打ち切りまで待つ（1 章）
- 再 open 時の全文 `didChange` が `version` を増やさない（2 章。LSP は増加を求める）
- hover の予算切れをエージェントに伝えない（6 章）

## 一般化してはならない点

- ソースを読んだ結果で、Serena を動かして測ったのは [serena-integration-measurement.md](serena-integration-measurement.md) の範囲だけ
- 8 章の候補は根拠と必要な作業を並べたもので、採否は決めていない。仕様に触れるもの（A、B、C、F）は ADR が要る
