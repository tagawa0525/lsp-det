# Claude Code 経由のドッグフーディング観測（2026-09-03）

`dogfood/README.md` の手順で Claude Code（CC）に lsp-det を挟み、CC が言語サーバーをどう使うかを観測する。設計 8 章の観測項目（CC がサーバーをいつ起動しいつ最初の横断リクエストを投げるか、リクエストタイムアウトとエラーの見せ方、未知の通知の扱い）を埋めるのが目的。本文は観測のたびに追記する。

## 観測環境

- CC: `claude --plugin-dir dogfood/claude-plugin`（`dogfood/README.md` と同じ。セッションの再開 `-c` は付けても挙動は変わらない）。第 1 回は `--debug` なし、第 2 回は `--debug` あり。作業ディレクトリは本リポジトリで、direnv が `target/release` を PATH に置く
- lsp-det: main `f9b8237` の release ビルド
- 上流: rust-analyzer 2026-08-03（flake.nix で固定した nixpkgs のビルド）
- 確認手段: CC の LSP ツール（hover / findReferences）と、`ps` / `/proc` でのプロセス系譜の確認。第 3 回は被験者を入れ子の非対話 CC（`claude -p "<findReferences を 1 回投げて結果をそのまま書け>" --plugin-dir <本リポジトリ>/dogfood/claude-plugin --debug --output-format json --allowedTools LSP`）にし、対象ディレクトリを cwd にして起動した。`--output-format json` の `session_id` から debug ログの場所が分かる

## 第 1 回（2026-09-03）: 経路の成立と起動タイミング

### 結論

| 項目                                 | 観測                                                                                                                                                                                                                                 |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 経路の成立                           | **成立**。プロセス系譜は `claude` → `lsp-det -- rust-analyzer` → `rust-analyzer-unwrapped-2026-08-03`。lsp-det の実体は `target/release/lsp-det`、cwd はリポジトリ                                                                   |
| CC がサーバーを起動する時点          | **起動時ではなく、最初の LSP ツール呼び出し時**。セッション開始後しばらく lsp-det のプロセスはなく、hover を投げた直後に現れた                                                                                                       |
| 起動直後の横断でないリクエスト       | 最初の hover は「hover 情報なし」で返った。hover は仕様 7.0 の横断リクエストではないので下流側は保留せず転送し、インデックス中の rust-analyzer が null を返したもの。CC はこれを「シンボル上でないか、インデックス未完了」と表示する |
| インデックス完了後                   | 同じ位置の hover が型とドキュメントを返し、`findReferences` が 2 ファイル 6 箇所を返した。応答は lsp-det 経由                                                                                                                        |
| 同一拡張子を扱う別プラグインとの競合 | 設定で有効な公式の `rust-analyzer-lsp` プラグインはキャッシュに README しかなく LSP 定義を持たないため、競合は起きていない。CC の子プロセスは lsp-det 1 つだけ                                                                       |
| 写像の選択ログ                       | lsp-det の stderr は CC のソケットに繋がっている。`--debug` なしでは残らないので、宣言内容の確認は `claude --debug` で起動し直して行う（次回）                                                                                       |

### 設計への含意

- 横断リクエスト以外（hover・documentSymbol 等）で起きる「インデックス未完了の空応答」は、設計 4.3 の判定表の対象外なのでそのまま見える。これは設計どおりであり、対象を広げるかは仕様 7.0 の横断リクエストの定義の問題であって lsp-det 単独では決めない
- CC がサーバーを遅延起動するため、「起動直後に横断リクエストが来る」状況は CC で普通に起きる。下流側の保留（`indexing` の間 `references` を保留し `ready` で流す）は CC にとって意味がある

## 第 2 回（2026-09-03）: 起動直後の横断リクエストと `--debug` ログ

`claude --debug` で開き直し、言語サーバーが起動していない状態で最初の操作として `findReferences` を投げた。CC は `--debug` のとき言語サーバーの stderr と LSP メッセージの送受信を `~/.claude/debug/<セッション ID>.txt` に残す。

### 結論

| 項目                                  | 観測                                                                                                                                                                                                            |
| ------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 写像の選択と宣言                      | stderr に `upstream is "rust-analyzer" version "2026-08-03"; using its mapping, declaring {"completeness":true,"freshness":true}` が出た。テスト済みの版の一覧に載っている版なので保証が宣言されている          |
| CC が最初の横断リクエストを投げる時点 | `initialize` の応答から **6ms 後**。`initialized` → `didOpen`（対象ファイル 1 つ）→ `textDocument/references` の順で、インデックスの完了を待たない                                                              |
| 下流側の保留                          | `references` が届いた時点の状態は `{ok, indexing}`。lsp-det が保留し、6.7 秒後の `ready` で流した。CC には **完全な結果（2 ファイル 6 箇所）** が返った。保留がなければ第 1 回の hover と同じく空応答だった局面 |
| CC のリクエストタイムアウト           | 応答まで 8.4 秒（保留 6.7 秒 + rust-analyzer の処理 1.7 秒）でタイムアウトしていない。上限の値はこの観測では分からない（8.4 秒より長い、としか言えない）                                                        |
| CC が宣言していない通知の扱い         | `$/progress` が 223 件 CC に届いたが、エラーもログの警告もない。`experimental/serverStateChanged` は写像ありのとき CC に流さない設計なので届いていない（設計 4.2）                                              |
| サーバー → クライアントのリクエスト   | rust-analyzer の `workspace/diagnostic/refresh` を CC が 0ms で応答した。lsp-det が代行するのは注入に由来する `window/workDoneProgress/create` だけで、これは素通し                                             |

時系列（`--debug` ログの時刻、`initialize` 送信を 0 とする）:

| 時刻   | 出来事                                                                      |
| ------ | --------------------------------------------------------------------------- |
| 0.000s | CC が `initialize` を送る。lsp-det の初期状態は `{unknown, unknown}`        |
| 0.005s | `initialize` 応答。写像を選び `{unknown, initializing}` へ                  |
| 0.006s | CC が `initialized`・`didOpen`・`references (1)` を送る                     |
| 0.008s | 最初の `experimental/serverStatus` で `{ok, indexing}`。`references` は保留 |
| 6.715s | `{ok, ready}`。保留を解放                                                   |
| 8.376s | `references (1)` の応答が CC に届く                                         |

### 設計への含意

- 「CC は起動直後に横断リクエストを投げる」が事実として確定した。下流側の保留は CC のこの使い方に直接効いている
- CC のリクエストタイムアウトは今回の保留（6.7 秒）では掛からなかった。大規模ワークスペースで保留が長引いたときに CC がどう見せるかは未観測のまま。仕様は時間による打ち切りを禁じている（6 章 6 項）ので、掛かった場合の対処は CC 側の `$/cancelRequest` を受けて保留を取り消す経路（設計 4.3）になる
- 未知の通知（`$/progress`）を CC は黙って受け取る。設計 4.2 の「問題が出たら握りつぶしに変更する」条件は今のところ満たしていない

## 第 3 回（2026-09-03）: 長い保留・gopls 経路・`error` の拒否

第 2 回の未観測 3 件を `reference/` の先行事例リポジトリで埋めた。被験者はいずれも入れ子の非対話 CC で、起動直後に `findReferences` を 1 回投げる（観測環境の節）。

### 結論

| 被験者                                                             | 上流                     | `references` 到着時の状態 | 下流側の判定                                 | 応答まで    | CC の見せ方                                                                                                                                                                          |
| ------------------------------------------------------------------ | ------------------------ | ------------------------- | -------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `reference/zed`（Rust 1935 ファイル、依存 1815 crate）の `fn main` | rust-analyzer 2026-08-03 | `{ok, indexing}`          | 保留 → 80.6 秒後の `{warning, ready}` で転送 | **82.4 秒** | 完全な結果（1 箇所）。**タイムアウトも `$/cancelRequest` もなし**                                                                                                                    |
| `reference/rust-analyzer`（Rust 1481 ファイル）の `AnalysisHost`   | rust-analyzer 2026-08-03 | `{ok, indexing}`          | 保留 → 2.7 秒後の `{warning, ready}` で転送  | 3.6 秒      | 完全な結果（11 ファイル 45 箇所。grep でも 11 ファイル）                                                                                                                             |
| `reference/golang-tools`（Go 1935 ファイル）の `packages.Load`     | gopls v0.23.0            | `{unknown, indexing}`     | 保留 → 0.67 秒後の `{ok, ready}` で転送      | 0.9 秒      | 完全な結果（45 ファイル 147 箇所）                                                                                                                                                   |
| Cargo.toml のないディレクトリの `lib.rs`                           | rust-analyzer 2026-08-03 | `{error, ready}`          | 拒否（RequestFailed）                        | 3ms         | `Error performing findReferences: LSP request 'textDocument/references' failed for server '…': lsp-det: the language server reports health: error (Failed to discover workspace. …)` |

- **CC のリクエストタイムアウトは 82 秒の保留でも掛からない**。上限値は依然として分からないが、実用上の大規模ワークスペース（zed）で保留が問題にならないことは確認できた
- **`warning` は転送する**（設計 4.3 の判定表どおり）。zed と rust-analyzer リポジトリでは nixpkgs の rustc でビルドスクリプトが失敗して health が `warning`（"Failed to run build scripts of some packages."）になったが、`references` は完全な結果を返した。この場合の「完全」はビルドスクリプト由来のコードを除いた完全さで、rust-analyzer の語彙ではそれ以上区別できない
- **rust-analyzer の `ready` はファイル数に比例しない**。rust-analyzer リポジトリ（1481 ファイル）は 2.7 秒、本リポジトリ（数十ファイル）は 6.7 秒、zed は 80.6 秒。`quiescent` の実体は VFS のロードとキャッシュのプライミング（ADR 0007）で、時間を支配するのはビルドスクリプト・proc macro の実行である
- **`error` の拒否は CC にそのまま見える**。lsp-det が付けた理由文（rust-analyzer の `message` を含む）を CC はエラー本文として表示し、次の手（`linkedProjects` の設定）まで読み取れる

### 副産物: CC の `shutdown` は `params: {}` を持ち、rust-analyzer が拒否する

CC は終了時の `shutdown` リクエストに `params: {}` を付ける。rust-analyzer（lsp-server crate）は `shutdown` の params を `()` として読むので `invalid type: map, expected unit` の InvalidParams（-32602）を返し、CC は "Failed to stop LSP server" をエラーログに出して `exit` を送らずに切断する。gopls は `params: {}` を受理する。lsp-det はボディをそのまま流すので関与しておらず、直接接続でも同じ応答になることを確認した（`params` を省けば両者とも `result: null`）。CC が切断したあとは lsp-det が stdin の EOF で上流を道連れにし（設計 4.5）、4 回の観測で孤児プロセスは残っていない。これは CC 側（または rust-analyzer 側の厳格さ）の問題で、公式の rust-analyzer プラグインでも同じことが起きるはずである。

### 設計への含意

- 下流側の保留は、CC が実際に投げる「起動直後の横断リクエスト」に対して、小規模（7 秒）から大規模（80 秒）まで一貫して効いた。仕様が禁じる打ち切りタイマー（6 章 6 項）がなくても CC 側で困っていない
- `$/cancelRequest` を CC が送る場面は 4 回の観測で一度もなかった。設計 4.3 のキャンセル経路は準拠テストでしか確かめられていない

### 未観測（次回以降）

- CC のリクエストタイムアウトの実値（82 秒より長い、としか言えない）
- CC が `$/cancelRequest` を送る条件
- gopls で health が `error` になる場合（"Error loading workspace"）の CC の見せ方

## 第 4 回（2026-09-04）: CC が送る通知の全数と `initialize` の capability

`--debug` のログ 29 本（LSP 通信のある 6 本）を全数で確認し、さらに `tee` で stdin を記録するラッパーを言語サーバーの位置に置いて、入れ子の非対話 CC（2.1.259）の `initialize` を原文で取った。実測の本体は [disk-edit-propagation-measurement.md](disk-edit-propagation-measurement.md)。

### 結論

| 項目                       | 観測                                                                                                                                                                                                                  |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| CC が送る通知              | `initialized`・`textDocument/didOpen`・`exit` の 3 種だけ。`didChange`・`didSave`・`didClose`・`workspace/didChangeWatchedFiles`・`$/cancelRequest` は 29 本のログに一度も出ない                                      |
| Write の後                 | 書き込み（一時ファイルを書いて rename）の 1ms 後に、同じファイルへ `didOpen` を送り直す（新しい本文、閉じない）。サーバーのある言語だけ。引き金は diagnostics の経路で、LSP ツールの呼び出しではない                  |
| Bash の編集の後            | 何も送らない（377 回の Bash 呼び出しに LSP の行が伴わない）                                                                                                                                                           |
| `initialize` の capability | `workspace` は `{configuration: false, workspaceFolders: false}` だけで **`didChangeWatchedFiles` がない**。`textDocument.synchronization` は `didSave: true` を宣言するが送らない。`window` と `experimental` はない |
| サーバーからのリクエスト   | `workspace/diagnostic/refresh` に `-32601 Unhandled method` で答える                                                                                                                                                  |

### 設計への含意

- gopls と pyright は自前でファイルを監視しないので、CC の Bash 編集はセッションの間ずっと見えない。rust-analyzer は監視の宣言がないときの自前の notify で拾う。tsls は 2 度目の `didOpen` を拒み、古いバッファが残る（disk-edit-propagation-measurement.md）。ADR 0015 の代行 2 つの根拠
- CC への報告の材料（`docs/upstream-submissions.md`）

## 第 5 回（2026-09-06）: 下流側の代行 2 つを CC 2.1.261 で確かめる

ADR 0015 の代行 2 つ（`workspace/didChangeWatchedFiles` の代行、重複した `didOpen` の `didChange` への書き換え）が実物の CC で効くかを、**lsp-det あり／なし**で比べた。CC への報告に「lsp-det を挟むと消える」と書くための実測。

### 方法

- 被験者: 入れ子の非対話 CC 2.1.261（`claude -p "<手順>" --plugin-dir <プラグイン> --debug-file <ログ> --output-format json --allowedTools "LSP,Bash" --model sonnet`。手順は固定で、ファイルは読ませない）。モデルは Sonnet（被験者は CC の LSP クライアントであり、モデルは無関係）
- プラグイン 2 種: **直接**（`command` を tee ラッパーにし、サーバーの stdin を記録）と **lsp-det 経由**（`lsp-det -- <tee ラッパー>`。lsp-det が上流に書いたものを記録）
- lsp-det: main `1234bdd`（0.3.0）の release ビルド。上流: gopls 0.23.0、typescript-language-server 5.3.0（TypeScript 5.9.3）
- 被験体: `git init` だけしたディレクトリ（代行は `git ls-files --others` で未追跡も拾う）。Go は `a.go`（`func Target()` と `main` での呼び出し）と `b.go`（呼び出し 1 つ）。TS は `a.ts`（`export function target()`）と `b.ts`（import と呼び出し 1 つ）
- 手順（Go）: `findReferences`（`Target`）→ Bash で `c.go`（呼び出し 1 つ）を作る → `findReferences`。手順（TS）: `findReferences`（`target`）→ Write で `b.ts` を上書き（呼び出し 2 つ）→ `findReferences` → Write で再度上書き（呼び出し 3 つ）→ `findReferences`

### 結果

| 被験体                                 | 直接                                                                                                                                                            | lsp-det 経由                                                                                                           |
| -------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| gopls: Bash で `c.go` を足した後の件数 | 3 → **3**（`c.go` は見えない。上流に届いたのは `didOpen a.go` と references だけ）                                                                              | 3 → **4**（`c.go` を含む。2 回目の references の前に `workspace/didChangeWatchedFiles` `Created c.go` が上流に届いた） |
| tsls: 起動直後の 1 回目の件数          | **1**（`a.ts` の宣言だけ。プロジェクトは `didOpen` の後にロードされ、その前の応答）                                                                             | **3**（`indexing` の間 0.175 秒保留し、`ready` で流した完全な結果）                                                    |
| tsls: Write 1 回目 → 2 回目の件数      | 4 → 5                                                                                                                                                           | 4 → 5                                                                                                                  |
| 2 回目の Write の後に CC が送るもの    | `textDocument/didChange`（全文、version 2）と `textDocument/didSave`                                                                                            | 同じ（書き換えは起きていない）                                                                                         |
| `initialize` の capability             | 第 4 回と同じ。`workspace` は `{configuration: false, workspaceFolders: false}` で **`didChangeWatchedFiles` がない**。`didChangeWatchedFiles` の通知も送らない | 同じ                                                                                                                   |
| `shutdown`                             | `params: {}`（第 4 回と同じ）                                                                                                                                   | 同じ                                                                                                                   |

### 結論

- **`didChangeWatchedFiles` の代行は実物の CC で効く**。CC 2.1.261 は監視を宣言も送信もせず、gopls は自前で監視しないので、直接では Bash で作ったファイルがセッションの間見えない。lsp-det を挟むと、次の横断リクエストの前に Created が届き、結果に含まれる
- **重複した `didOpen` は CC 2.1.261 では起きない**。2.1.259（第 4 回）は Write のたびに `didOpen` を送り直したが、2.1.261 は開いている文書への 2 回目の Write で `didChange`（全文）と `didSave` を送る。ADR 0015 決定 B の書き換えは CC では発動せず、典型的な誘因は CC 側で消えた。書き換え自体は LSP 違反への一般的な対処として残す
- **tsls でも起動直後の無言の嘘が CC で見える**。直接では最初の `findReferences` が宣言 1 件だけを返し、CC はそれを結果として受け取る。lsp-det 経由では `ready` まで保留して 3 件
- CC への報告（`docs/upstream-submissions.md`）は 4 件に減る: 起動直後の横断リクエスト（rust-analyzer に加え tsls でも）、`shutdown` の `params: {}`、`didChangeWatchedFiles` の欠落、`didClose` を送らないこと。`didChange` / `didSave` は送るようになった
- lsp-det 経由では、CC の Write の後に代行が `Changed b.ts` も送っている（開いている文書のディスク変更。サーバーは開いているバッファを優先するので無害）
- `exit`: CC は `exit` を送った同じミリ秒にクライアントを止める。lsp-det 経由では上流の stdin に `exit` が現れなかったが、上流は終了し残留プロセスはない（EOF かプロセス寿命の経路。原因は特定していない）
- 費用: 1 走行 5〜7 ターン、Sonnet で 0.1 ドル前後

## 一般化してはならない点

- 「最初の LSP ツール呼び出しで起動」「`initialize` の直後に横断リクエストを投げる」は CC のこの版での観測。CC の版が変われば変わり得る（実際、Write の再 `didOpen` は 2.1.259 で観測し 2.1.261 で消えた）
- 起動直後の hover が null を返す時間幅と、保留の 6.7 秒は本リポジトリの規模での話で、他のワークスペースには一般化できない
- 「82 秒でタイムアウトしなかった」は上限の下界にすぎず、CC のタイムアウト値そのものは分かっていない
- zed の 80.6 秒は、他の被験者と同時に走らせた（CPU を分け合った）値で、単独ならもっと短い。順序（小規模 < 大規模）だけを読む
