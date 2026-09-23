# ADR 0023: typescript-language-server の coverage を workspace の形で宣言する

- 日付: 2026-09-23
- 状態: 提案（ユーザーの判断待ち）
- 関連: [research/typescript-language-server-no-solution-coverage.md](../research/typescript-language-server-no-solution-coverage.md)、[ADR 0009](0009-success-criterion-and-two-sided-reference.md) 決定 D-5（保証は測った版に）、[ADR 0020](0020-v0.5-dart-sorbet-jdtls-clangd.md) と [ADR 0021](0021-v0.6-nixd-nil-and-daily-dogfooding.md) の追補（2026-09-09。観測者がサーバー自身の探索をなぞって workspace を読む先例）

## 経緯

lsp-det の typescript-language-server の写像は、検証済みの版（TypeScript 5.9.3）なら workspace の形によらず `coverage: {scope: "workspace", incomplete: {}}` を宣言する。solution（他の project を `references` に持つ tsconfig.json）のない multi-project の workspace では、tsserver は開いたファイルの project と、読み込み済みの solution から辿れる project しか探さない。lsp-det 経由で測ると、宣言したまま `ready` の後の `textDocument/references` が 16 ファイル中 0 で、別のファイルを 1 つ開くと `indexing` → `ready` を経て 2 に増えた（仕様 6 章 1 項と 7.2 の 1 に反する宣言）。tsconfig.json が 1 つの workspace と、根の solution が全パッケージを `references` に持つ workspace では完全だった。

準拠テスト 7.2 の fixture は tsconfig.json が 1 つの workspace で、その結果が workspace の形によらず宣言に使われている。仕様 8.2 の 5 は宣言を版で決め、workspace の形を見ない。

信号（`$/progress`）は正しく、保留も効いている。壊れているのは保証の宣言だけである。

## 決めること

1. lsp-det が、solution のない multi-project の workspace で何をするか（下の案 A〜F）
2. 案 A を採るなら、どの形を「完全」とみなすか（規則 R1 / R2）
3. 仕様 8.2 の 5 に「workspace の形」を書き足すか

## 案

| 案                                                                      | 内容                                                                                                                                                                                                                                                                                                                                                                      | 評価                                                                                                                                                                                                                                                                                                                                                                 |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **A. workspace の形で宣言を決める**（推奨）                             | 写像を選んだ時点で `initialize` の `workspaceFolders` を読み（`src/tracker.rs` の `adopt` が `learn_workspace_folders` を呼んだ後に宣言を返すので、今の経路でできる）、tsserver と同じ規則で tsconfig.json の配置を見る。完全とみなせる形なら今までどおり `coverage` を宣言し、そうでなければ `coverage` を宣言しない（`freshness` は残す）。`readiness` と保留は変えない | 守れない保証を名乗らなくなる。保留は残るので #1937 のような一過性の窓は引き続き塞げる。clangd と nil の追補と同じく、観測者がサーバー自身の探索をなぞる。誤って「完全でない」と判定しても失うのは約束だけで、lsp-det を挟まない状態より悪くはならない。宣言は `InitializeResult` で一度決まるので、会話の途中の `workspace/didChangeWorkspaceFolders` には追従しない |
| B. `readiness` を `unknown` にする                                      | clangd（データベースなし）と nil（nixpkgs の入力なし）の追補と同じ扱い                                                                                                                                                                                                                                                                                                    | 却下を推す。先例は「信号が来ない」から `unknown` にした。ここでは信号は来ていて正しい。保留も失い、#1937 の一過性の窓が lsp-det 経由で戻る                                                                                                                                                                                                                           |
| C. 仕様に新しい `scope` の値を足す（例: 読み込み済みの project の範囲） | 宣言は保ち、欠けを名指しする値を足す（仕様 1.1）                                                                                                                                                                                                                                                                                                                          | 保留を推す。範囲がクライアントの操作（どのファイルを開いたか）で動くので、クライアントが欠けを読んで判断に使いにくく、観測者にも確かめにくい。再検討の条件: LSP 本体への提案の議論で、サーバーが自分の範囲を名乗る形が求められたとき                                                                                                                                 |
| D. 現状のまま、仕様 10 章の tsls の行に欠けとして書く                   | 宣言は変えない                                                                                                                                                                                                                                                                                                                                                            | 却下を推す。仕様 5 章「守れない保証の宣言は本プロトコルへの違反」                                                                                                                                                                                                                                                                                                    |
| E. lsp-det がファイルを開いて project を読み込ませる                    | 全パッケージの project を tsserver に読み込ませ、宣言を真にする                                                                                                                                                                                                                                                                                                           | 却下を推す。観測者がサーバーの状態を変える。大きな workspace では記憶と時間を食い、tsserver は開いていない project を後で解放する（#1937 の揺れの原因）                                                                                                                                                                                                              |
| F. tsls では常に `coverage` を宣言しない                                | workspace の形を読まない                                                                                                                                                                                                                                                                                                                                                  | A の予備。単純だが、測って完全だった形（tsconfig.json 1 つ、根の solution）の約束まで捨てる                                                                                                                                                                                                                                                                          |

## 案 A の規則（決めること 2）

どちらも、workspace の各フォルダの下（`node_modules` と `.git` を除く）の `tsconfig.json` / `jsconfig.json` の配置だけで決め、tsserver を問い合わせない。フォルダが複数なら全部が満たすときだけ宣言する。tsconfig.json が 1 つもない workspace（inferred project）は完全とみなさない。

- **R1（厳しい）**: 設定ファイルがちょうど 1 つで、それがフォルダの根にあり、かつその設定がフォルダの中の `.ts` / `.tsx` をすべて含むときだけ完全とみなす。単一の設定でも `files` / `include` / `exclude` で範囲が狭まれば、外れたファイルは開くまで答えに現れない（inferred project）。そこで R1 は根の設定の `files` / `include` / `exclude` を読み（コメントと末尾のカンマを除く小さな読み取り。依存は足さない）、既定の `include`（`**/*`）か、フォルダの中の全ファイルを含む `include` のときだけ完全とみなす。`extends` があって `files` / `include` を自分で書いていない設定は、継承元を解かずに不完全とみなす。準拠テスト 7.2 の fixture（`include: ["**/*.ts"]`）と同じ形で、今の宣言の根拠をそのまま使える。solution で完全な monorepo でも約束を捨てる
- **R2（solution を読む）**: R1 に加えて、フォルダの根の tsconfig.json から `references` を推移的に辿って他のすべての設定ファイルに届き、かつ各設定ファイルとフォルダの根の間に別の tsconfig.json がないとき（tsserver の祖先探索が根の solution に届く）も完全とみなす。tsconfig.json はコメントと末尾のカンマを許すので、それを除く小さな読み取りを書く（依存は足さない）。宣言を広げる形なので、仕様 8.2 の 5 に従い solution の形の fixture を準拠テスト 7.2 に足して通してから宣言する（今回の測定では 16 / 16 で完全）。`disableSolutionSearching` / `disableReferencedProjectLoad` はどの設定にあっても祖先の solution の探索や子の読み込みを止めるので、どれかの設定がそれを書いていれば不完全とみなす。`extends` を持つ設定は継承元を解かないので、継承でこれらが入りうる以上、R2 では `extends` を持つ設定が 1 つでもあれば不完全とみなす（実際の monorepo の多くが `extends` を使うので、R2 の範囲はかなり狭い）。各設定の `include` / `exclude` は R1 と同じく読む

推奨は、R1 で宣言の誤りを先に止め、R2 は solution の fixture の準拠テストと一緒に続けて入れる、の 2 段。

## 仕様 8.2 の 5 への書き足し（決めること 3）

「観測者は、テストを通した版であっても、workspace の形がテストの範囲の外にあると判定できるときは宣言してはならない」の旨を 8.2 の 5 に足す（仕様 1.1。11 章に記す）。typescript-language-server だけの事情ではなく、宣言の根拠（準拠テストの fixture）の外を観測者が見分けられるときの一般則になる。書き足さない場合、案 A は写像の実装の判断として残り、仕様は版だけを条件にしたままになる。

## 影響（案 A + R1 + 書き足しを採る場合）

- `src/adapter/typescript_language_server.rs`（`learn_workspace_folders` で設定ファイルの配置を読み、`guarantees` で `coverage` を外す）
- `tests/conformance.rs`（実サーバーの no-solution の workspace で `coverage` を宣言しないこと。今は RED）、偽上流のテスト
- 仕様 8.2 の 5 と 10 章の typescript-language-server の行（英日）、11 章
- `docs/research/typescript-language-server-readiness-measurement.md` の末尾、`README.md` / `README.ja.md` の該当箇所、`CHANGELOG.md`、`CLAUDE.md` の現在地
