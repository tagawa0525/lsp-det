# typescript-language-server の readiness・health・名乗りの実測（2026-09-03）

M6（typescript-language-server の写像、ADR 0010）の前提を実サーバーで確かめた。[research/server-readiness.md](server-readiness.md) 2 章のソースの読みを実測で裏付け、ADR 0010 決定 B の M6 と ADR 0011 決定 A（`serverInfo` のないサーバーの名乗り）をこのサーバーに当てはめる根拠にする。

## 結論

| 項目                     | ソースの読み                                                                                                                                           | 実測                                                                                                                                                                                                                                 |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `serverInfo`             | `initialize` 応答に `serverInfo` を書くコードがない（`lsp-server.ts`）                                                                                 | **なし**                                                                                                                                                                                                                             |
| 名乗りに相当する信号     | 起動時に `window/logMessage`（info）"Using Typescript version (user-setting) 5.9.3 from path …" と、独自通知 `$/typescriptVersion` `{version, source}` | 両方が `initialize` 応答の直後に届く。どちらも tsserver（TypeScript）の版であり、typescript-language-server 自身の版（5.3.0）はワイヤに出ない                                                                                        |
| 起動直後の横断リクエスト | tsserver の `projectLoadingStart` から `projectLoadingFinish` までプロジェクトのロードが進む                                                           | 2 ファイルでも、`didOpen` 直後の references は **0 件**（期待 2 ファイル分）。progress end の後は全件（4 箇所 = import と呼び出し）                                                                                                  |
| readiness の信号         | `$/progress`（title "Initializing JS/TS language features…"）の begin / end。トークンは `window/workDoneProgress/create` で作られる                    | 一致。begin 0.17 秒、end 0.34 秒（2 ファイル）                                                                                                                                                                                       |
| プロジェクトのロード契機 | tsserver はファイルが開かれたときにそのプロジェクトをロードする                                                                                        | **ファイルを開くまで progress は一度も出ない**。`initialize` だけでは何もロードしない                                                                                                                                                |
| 再ロード                 | tsconfig の変更で `projectLoadingStart` が再発火                                                                                                       | 一致。tsconfig.json をディスクで変えて `didChangeWatchedFiles` を送ると 0.25 秒後に begin、0.09 秒後に end                                                                                                                           |
| 複数プロジェクト         | ロードは逐次で、新しい `projectLoadingStart` は前の progress を `reset()`（end）してから begin する                                                    | 一致。2 プロジェクトを続けて開くと、1 つ目の end → 2 つ目の begin → end の順。同時に 2 つの progress が開くことはない                                                                                                                |
| tsserver のクラッシュ    | `onExit` で `window/logMessage`（error）"[tsserver] Exited. Code: N. Signal: S"。exit code が非 0 なら言語サーバー自身も落ちる                         | SIGKILL（code null）では**言語サーバーは生き残り**、直後の references に **空配列を成功として返す**（壊れたサーバーの成功風応答）。ログは type 1（Error）で、"[lspserver] [tsclient] [tsserver] Exited. Code: null. Signal: SIGKILL" |
| クラッシュ時の progress  | `serviceExited()` が indicator を reset する                                                                                                           | ロード中でなければ progress は出ない（今回の実測）。ロード中のクラッシュは測っていない                                                                                                                                               |
| tsserver の再起動        | なし（`serverState = None` のまま。`restart` / `respawn` のコードがない）                                                                              | 6 秒後も言語サーバーは生きていて、tsserver は戻らない                                                                                                                                                                                |

## 測定環境

- typescript-language-server 5.3.0、typescript 5.9.3（tsserver）、node 24.19.0（すべて nixpkgs、flake.nix で固定）。起動は `typescript-language-server --stdio`
- 経路: 偽クライアント（Python スクリプト、スレッド化した読み手）→ 言語サーバー直結。lsp-det は挟んでいない
- `initialize` は `rootUri` と `workspaceFolders` を渡し、`window.workDoneProgress: true` を宣言
- fixture: `tsconfig.json`（`include: ["**/*.ts"]`）、`a.ts` に `export function target()`、`m0000.ts`… が `import { target }` と `target()` の呼び出しを持つ

## 時系列（2 ファイル、`initialize` 送信を 0 とする）

| 時刻   | 出来事                                                                                                                  |
| ------ | ----------------------------------------------------------------------------------------------------------------------- |
| 0.049s | `window/logMessage` "Using Typescript version (user-setting) 5.9.3 from path …"、`initialize` 応答（`serverInfo` なし） |
| 0.050s | `initialized`、`didOpen a.ts`、references #1 を送る                                                                     |
| 0.051s | `$/typescriptVersion` `{"version":"5.9.3","source":"user-setting"}`                                                     |
| 0.165s | `window/workDoneProgress/create`、`$/progress` begin "Initializing JS/TS language features…"                            |
| 0.310s | references #1 の応答: **0 件**                                                                                          |
| 0.343s | `$/progress` end。references #2 を送る                                                                                  |
| 0.362s | references #2 の応答: 4 件                                                                                              |

クラッシュ（別の実行、`initialize` 送信を 0 とする）:

| 時刻   | 出来事                                                                                                         |
| ------ | -------------------------------------------------------------------------------------------------------------- |
| 4.524s | 言語サーバーの子プロセス 2 つ（tsserver の semantic / syntax サーバー）に SIGKILL。直後に references #5 を送る |
| 4.536s | `window/logMessage`（type 1）"[lspserver] [tsclient] [tsserver] Exited. Code: null. Signal: SIGKILL"           |
| 4.536s | references #5 の応答: **0 件、error なし**                                                                     |
| 10.55s | 言語サーバーはまだ生きている（exit code なし）                                                                 |

## 写像への含意（ADR 0010 決定 B の M6 に当てはめる）

- **名乗り**: `$/typescriptVersion` は typescript-language-server 固有の通知で、これを名乗りとして写像を選ぶ。版はワイヤに出る tsserver（TypeScript）の版で突き合わせる。typescript-language-server 自身の版は出ないので、テスト済みの版の一覧は「TypeScript の版」で持ち、その注意を一覧に書く。`serverInfo` を足す上流 PR の候補（pyright と同じ）
- **readiness**: `initializing` から始め、"Initializing JS/TS language features…" の begin で `indexing`、そのトークンの end で `ready`。逐次ロードなので同時に複数のトークンは開かないが、gopls と同じく begin で覚えたトークンがすべて end したら `ready` にする
- **health**: 最初の end で `ok`（ロードの成功を観測した）。"[tsserver] Exited. Code:" のログ（error）で `error`。再起動はないので `error` は戻らない。クラッシュ後の references は空配列を成功として返すので、下流側の拒否（RequestFailed）が「壊れたサーバーの成功風応答」を消す
- **限界**: ファイルを開くまでプロジェクトをロードしないので、`didOpen` を送らずに横断リクエストだけを送るクライアントでは `initializing` のまま保留が解けない（保留がロードの契機を奪う）。Claude Code は `didOpen` の後に横断リクエストを送る（[research/claude-code-dogfooding.md](claude-code-dogfooding.md)）。Serena は M7 で観測する。根本の解決は typescript-language-server が本プロトコルを話すこと

## 一般化してはならない点

- 0 件になる窓（約 0.3 秒）は 2 ファイルの fixture と本機の速さでの値。大規模プロジェクトでは長くなる
- クラッシュはロード完了後に測った。ロード中のクラッシュで progress の end とログのどちらが先に届くかは測っていない（写像はどちらの順でも `error` に落ち着く。end で `ok` にしてもその後の "Exited." で `error` になる）
- exit code が非 0 のクラッシュでは言語サーバー自身が落ちて接続が閉じる（ソースの読み。実測は SIGKILL のみ）。その場合は EOF で伝わる（ADR 0009 決定 C-3）
- 7.2 / 7.3 の通過は本文書では測っていない。M6 の準拠テスト（`tests/conformance.rs` の `typescript_language_server_*` ignored）で測り、通った版だけ一覧に載せる。仕様 10 章の見込みは「completeness のみ（非同期処理のため freshness 不可）」
