# ディスク上の編集が言語サーバーに届く経路の実測（2026-09-04）

ADR 0014 と ADR 0015 の根拠。コーディングエージェントはファイルをディスク上で書き換える（Claude Code の Write と Bash、Serena の編集ツール）。その変更が言語サーバーの応答に織り込まれるまでの経路を、4 サーバー × 4 場面で測った。あわせて Claude Code が送る `initialize` の capability を lsp-det の手前で記録した。

## 結論

| 経路                                                        | rust-analyzer                                           | gopls            | pyright                                                        | typescript-language-server                                                                    |
| ----------------------------------------------------------- | ------------------------------------------------------- | ---------------- | -------------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| S1 `workspace/didChangeWatchedFiles`（Changed）             | 即時（送信中の要求は -32801 で拒否、次の要求は正）      | 即時             | 即時                                                           | 即時（監視を登録していなくても受け付ける）                                                    |
| S2 通知なし、クライアントが監視を宣言している               | 10 秒待っても古いまま                                   | 古いまま         | 古いまま                                                       | 即時（自前で監視）                                                                            |
| S2' 通知なし、Claude Code と同じ宣言（監視を宣言しない）    | 5ms で反映（自前の notify に切り替わる）                | **古いまま**     | **古いまま**                                                   | 即時                                                                                          |
| S3 既に開いている uri への 2 度目の `didOpen`（新しい本文） | 反映（stderr に `ERROR duplicate DidOpenTextDocument`） | 反映（ログなし） | 反映（"Received redundant open text document command" のログ） | **反映しない**。"Can't open already open document" で拒み、古いバッファがディスクを覆い続ける |
| S4 新規ファイル + `didChangeWatchedFiles`（Created）        | 0.18 秒後（`quiescent: false` → `true` を通知）         | 即時             | 0.15 秒後（"Found 3 source files" を再発行）                   | 即時                                                                                          |

読み取れること:

- 4 サーバーとも、`didChangeWatchedFiles` を受ければ次の応答から織り込む。非同期に再インデックスする rust-analyzer（新規ファイル）と pyright（再列挙）は、lsp-det が既に読んでいる信号で `indexing` に戻る。**「知らされたら織り込む」は 4 サーバーで成り立つ**（ADR 0014）
- gopls と pyright は自前で監視しない。クライアントが通知を送らなければ、ディスク上の編集はセッションの間ずっと見えない。rust-analyzer はクライアントが `workspace.didChangeWatchedFiles.dynamicRegistration` を宣言しないときだけ自前の notify で監視する。tsls は常に自前で監視する
- 同じ uri への 2 度目の `didOpen`（LSP 違反）は 3 サーバーが黙認するが、tsls は拒み、以後の応答が古いバッファに支配される

## Claude Code の `initialize`（lsp-det の手前で記録）

`tee` で stdin を記録するラッパーを言語サーバーの位置に置き、入れ子の非対話 CC（`claude -p … --plugin-dir … --allowedTools LSP`）から 1 回 `findReferences` を投げさせた。CC 2.1.259。

- `clientInfo`: `{"name": "Claude Code", "version": "2.1.259"}`。`initializationOptions: {}`
- `capabilities.workspace`: `{"configuration": false, "workspaceFolders": false}` だけ。**`didChangeWatchedFiles` はない**。`window` と `experimental` もない
- `capabilities.textDocument.synchronization`: `{"dynamicRegistration": false, "willSave": false, "willSaveWaitUntil": false, "didSave": true}`。他に `publishDiagnostics`、`hover`、`definition`、`references`、`documentSymbol`、`callHierarchy`。`general.positionEncodings: ["utf-16"]`
- 送った通知は `initialized`、`didOpen`（version 1、全文）、`references`、`shutdown`（`params: {}`）の順。`didChange`・`didSave`・`didClose`・`didChangeWatchedFiles`・`exit` はない。サーバーからの `workspace/diagnostic/refresh` には `-32601 Unhandled method` で答える
- `--debug` のログ 29 本の全数確認（[claude-code-dogfooding.md](claude-code-dogfooding.md) 第 4 回）: CC が送る通知は `initialized`・`didOpen`・`exit` の 3 種だけ。Write のたびに同じファイルへ `didOpen` を送り直し（書き込みの 1ms 後、新しい本文）、Bash の編集には何も送らない

したがって CC の利用者に起きていることは次の 3 つで、すべて実測に基づく。

1. Go と Python では、Bash で編集した開いていないファイルはセッションの間ずっと古いまま（S2'）
2. TypeScript では、Write のたびに古いバッファが残る（S3）
3. Rust は rust-analyzer の自前の監視に救われている（S2'）

## 測定方法

- fixture: A が `target` を定義し、B が 1 回呼ぶ 2 ファイル（Rust は `src/lib.rs` と `mod`、Go は package main、Python、TypeScript は `tsconfig.json` 付き）。references は A の定義位置から `includeDeclaration: false`。基準の件数は Rust / Python / TS が 2（import 行が数えられる）、Go が 1
- 各場面で新しいサーバープロセスを使う。readiness の信号（rust-analyzer は `quiescent: true`、gopls は "Setting up workspace" の end、pyright は "Found N source files"、tsls は `didOpen` 後の "Initializing JS/TS language features" の end）を待ってから始める。CC と同じ宣言の場面では信号が来ないので、基準の件数になるまで 100ms ごとに問い合わせて待つ
- S1: B をディスク上で書き換え（呼び出しを 1 つ追加）、開かずに Changed を送り、直後に references。期待の件数になるまで 100ms ごとに 10 秒まで問い合わせる。S2 / S2': 通知を送らない。S3: B を開いてから書き換え、同じ uri へ `didOpen`（version 1）をもう一度送る。S4: C を新規に作って Created を送る（Rust は `src/lib.rs` の `mod c;` の追加を Changed で同時に送る。新しいファイルは `mod` で名指しされるまで crate に入らないため）
- capability の宣言: 監視ありの場面は `workspace.didChangeWatchedFiles.dynamicRegistration: true`、`window.workDoneProgress: true`、`experimental.serverStatusNotification: true`。CC と同じ宣言の場面は上記の CC の capability をそのまま使う
- `client/registerCapability`（`workspace/didChangeWatchedFiles`）の登録: rust-analyzer は `<root>/**/*.rs` と `Cargo.{toml,lock}` 等、gopls は `**/*.{go,mod,sum,work}`、pyright は `**`（2 回）、tsls は登録しない。CC と同じ宣言では 4 サーバーとも登録を試みない
- 版: rust-analyzer 2026-08-03、gopls 0.23.0、pyright 1.1.412、typescript-language-server 5.3.0（TypeScript 5.9.3）

## 一般化してはならない点

- 2 ファイルの fixture で、再インデックスは 0.2 秒以内に終わる。大規模ワークスペースでの非同期の窓の長さは測っていない（lsp-det の `indexing` への戻りで覆う設計だが、pyright の再列挙の信号が大規模でも同じ形かは要観測）
- S2 の rust-analyzer は監視の宣言の有無で挙動が変わる。他のクライアントの宣言（Serena の各言語の capability）では別途測る必要がある
- CC の capability は 2.1.259 の非対話モードで記録した。対話モードで同じかは確かめていない
- tsls の自前監視は tsserver の機能で、監視の範囲（プロジェクト外のファイル等）は測っていない

## 追記（2026-09-06）: 通知の後の完了の信号は必ず来るか

ADR 0014 の第 2 のテストを実サーバーに当てると、Changed は 4 つとも通り、Created は pyright と tsls が通らなかった。通知の直後の問い合わせが古い答えを返し、再インデックスの信号はその後に届く。先読み（通知を見て `indexing` にする）が安全かを決めるため、サーバーごとに信号が必ず来るかを測った。

| サーバー      | 変種                                                                    | 通知後 3 秒以内のサーバーからのメッセージ                            | 直後の問い合わせ → 期待の件数まで |
| ------------- | ----------------------------------------------------------------------- | -------------------------------------------------------------------- | --------------------------------- |
| pyright       | Created `c.py`                                                          | "Found 3 source files"（+0.041 秒）                                  | 古い → +0.143 秒                  |
| pyright       | Created `.venv/lib/x.py`、`notes.txt`、`skip/d.py`（config の exclude） | **なし**                                                             | 古いまま                          |
| pyright       | Deleted `b.py`                                                          | "Found 1 source file"（+0.049 秒）                                   | +0.049 秒                         |
| pyright       | Changed `b.py`                                                          | なし                                                                 | 即時に正                          |
| rust-analyzer | Created `src/c.rs`（lib.rs は触らない、crate に入らない）               | `quiescent: false`（+0.013）→ Fetching → Indexing → `true`（+0.303） | -32801 で拒否 → 次は正            |
| rust-analyzer | Created + lib.rs Changed、Deleted `src/b.rs`                            | 同じ往復（0.3 秒）。health は `ok` のまま                            | +0.19 / +0.29 秒                  |
| rust-analyzer | Changed `src/b.rs`                                                      | なし                                                                 | -32801 で拒否 → +0.106 秒で正     |
| rust-analyzer | 監視を宣言しないクライアント（自前の notify）                           | 書き込みの 6ms 後に同じ往復                                          | 同上                              |
| tsls          | Created `c.ts`（`include: ["**/*.ts"]`）                                | **なし**（33 回すべて）                                              | 古い → **+1.03 秒**               |
| tsls          | Created `c.ts`（`include: ["*.ts"]`、非再帰）                           | なし                                                                 | 即時に正                          |

- "Searching for source files" は pyright のログに一度も出ない（写像が再開の信号として読んでいた文字列は、この版では出ない）
- tsls の 1.03 秒は TypeScript 自身の監視の実装で、Linux では再帰の glob を `setTimeout(…, 1000)` で更新する（`typescript.js` の `createDirectoryWatcherSupportingRecursive`）。tsls は `useClientFileWatcher` を `initializationOptions` で指定し、かつクライアントが `dynamicRegistration` と `relativePatternSupport` を宣言し TypeScript 5.4.4 以上のときだけ、クライアントの通知を tsserver の `watchChange` に流す。それ以外では `didChangeWatchedFiles` は何もしない（`src/lsp-server.ts:184-201, 498-500`）
- 測定の道具は scratchpad の `diskedit/probe2.py`（`run2_all.sh`、`summarize2.py`）
