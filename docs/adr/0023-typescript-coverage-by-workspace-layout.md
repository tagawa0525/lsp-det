# ADR 0023: typescript-language-server の coverage を workspace の形で宣言する

- 日付: 2026-09-23
- 状態: 採用（2026-09-25。ユーザーの決定。推奨どおり）
- 関連: [research/typescript-language-server-no-solution-coverage.md](../research/typescript-language-server-no-solution-coverage.md)、[ADR 0009](0009-success-criterion-and-two-sided-reference.md) 決定 D-5（保証は測った版に）、[ADR 0020](0020-v0.5-dart-sorbet-jdtls-clangd.md) と [ADR 0021](0021-v0.6-nixd-nil-and-daily-dogfooding.md) の追補（2026-09-09。観測者がサーバー自身の探索をなぞって workspace を読む先例）

## 経緯

lsp-det の typescript-language-server の写像は、検証済みの版（TypeScript 5.9.3）なら workspace の形によらず `coverage: {scope: "workspace", incomplete: {}}` を宣言する。solution（他の project を `references` に持つ tsconfig.json）のない multi-project の workspace では、tsserver は開いたファイルの project と、読み込み済みの solution から辿れる project しか探さない。lsp-det 経由で測ると、宣言したまま `ready` の後の `textDocument/references` が 16 ファイル中 0 で、別のファイルを 1 つ開くと `indexing` → `ready` を経て 2 に増えた（仕様 6 章 1 項と 7.2 の 1 に反する宣言）。tsconfig.json が 1 つの workspace と、根の solution が全パッケージを `references` に持つ workspace では完全だった。

準拠テスト 7.2 の fixture は tsconfig.json が 1 つの workspace で、その結果が workspace の形によらず宣言に使われている。仕様 8.2 の 5 は宣言を版で決め、workspace の形を見ない。

信号（`$/progress`）は正しく、保留も効いている。壊れているのは保証の宣言だけである。

## 決めること

1. lsp-det が、solution のない multi-project の workspace で何をするか（下の案 A〜F）
2. 案 A を採るなら、どの形を「完全」とみなすか（規則 R1 / R2）
3. 仕様 8.2 の 5 に「workspace の形」を書き足すか
4. 案 A を採るなら、宣言の後に配置が変わったときどうするか（下の「会話の途中で配置が変わるとき」。推奨は `readiness` を `unknown` にする）

## 決定

2026-09-25 にユーザーが推奨どおりに決めた。

1. **案 A を採る**。写像は `initialize` の workspace の根で tsconfig.json / jsconfig.json の配置を読み、完全とみなせる形のときだけ `coverage` を宣言する。そうでなければ `coverage` を宣言せず、`freshness` は残す。`readiness` と保留は変えない。B・D・E は却下、C は保留（再検討の条件は下の表）、F は採らない
2. **規則は R1 を先に入れ、R2 は後に続ける**。R1 で宣言の誤りを止める。R2 は宣言を広げるので、solution の形の fixture を準拠テスト 7.2 に足して通すのと同じ変更で入れる
3. **仕様 8.2 の 5 に書き足す**。「観測者は、テストを通した版であっても、workspace の形がテストの範囲の外にあると判定できるときは宣言してはならない」の旨。仕様の版を上げて 11 章に記す（ADR 0022 決定 A）
4. **会話の途中で配置が変わったら `readiness` を `unknown` にする**（下の「会話の途中で配置が変わるとき」のとおり。保留もやめる）。lsp-det に届かない変化は仕様 10 章の typescript-language-server の行に欠けとして書く

## 案

| 案                                                                      | 内容                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | 評価                                                                                                                                                                                                                                                                                                                                                                  |
| ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **A. workspace の形で宣言を決める**（推奨）                             | 写像を選んだ時点で `initialize` の workspace の根（`workspaceFolders`、なければ `rootUri`。`src/initialize.rs` の `workspace_roots` と同じ読み方）を読み（`src/tracker.rs` の `adopt` は写像に folder を渡してから宣言を受け取るので順序は今の経路のままでよいが、今渡しているのは `workspace_folders`（`rootUri` を読まない）なので、`workspace_roots` を渡す経路を足す。影響の欄）、tsserver と同じ規則で tsconfig.json の配置を見る。完全とみなせる形なら今までどおり `coverage` を宣言し、そうでなければ `coverage` を宣言しない（`freshness` は残す）。`readiness` と保留は変えない | 守れない保証を名乗らなくなる。保留は残るので #1937 のような一過性の窓は引き続き塞げる。clangd と nil の追補と同じく、観測者がサーバー自身の探索をなぞる。誤って「完全でない」と判定しても失うのは約束だけで、lsp-det を挟まない状態より悪くはならない。宣言は `InitializeResult` で一度決まるので、会話の途中の配置の変化は下の「会話の途中で配置が変わるとき」で扱う |
| B. `readiness` を `unknown` にする                                      | clangd（データベースなし）と nil（nixpkgs の入力なし）の追補と同じ扱い                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | 却下を推す。先例は「信号が来ない」から `unknown` にした。ここでは信号は来ていて正しい。保留も失い、#1937 の一過性の窓が lsp-det 経由で戻る                                                                                                                                                                                                                            |
| C. 仕様に新しい `scope` の値を足す（例: 読み込み済みの project の範囲） | 宣言は保ち、欠けを名指しする値を足す（仕様 1.1）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         | 保留を推す。範囲がクライアントの操作（どのファイルを開いたか）で動くので、クライアントが欠けを読んで判断に使いにくく、観測者にも確かめにくい。再検討の条件: LSP 本体への提案の議論で、サーバーが自分の範囲を名乗る形が求められたとき                                                                                                                                  |
| D. 現状のまま、仕様 10 章の tsls の行に欠けとして書く                   | 宣言は変えない                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | 却下を推す。仕様 5 章「守れない保証の宣言は本プロトコルへの違反」                                                                                                                                                                                                                                                                                                     |
| E. lsp-det がファイルを開いて project を読み込ませる                    | 全パッケージの project を tsserver に読み込ませ、宣言を真にする                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | 却下を推す。観測者がサーバーの状態を変える。大きな workspace では記憶と時間を食い、tsserver は開いていない project を後で解放する（#1937 の揺れの原因）                                                                                                                                                                                                               |
| F. tsls では常に `coverage` を宣言しない                                | workspace の形を読まない                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | A の予備。単純だが、測って完全だった形（tsconfig.json 1 つ、根の solution）の約束まで捨てる                                                                                                                                                                                                                                                                           |

## 案 A の規則（決めること 2）

どちらも、workspace の各フォルダの下（`node_modules` と `.git` を除く）の `tsconfig.json` / `jsconfig.json` の配置だけで決め、tsserver を問い合わせない。フォルダが複数なら全部が満たすときだけ宣言する。根が 1 つも分からない `initialize` では宣言しない。tsconfig.json が 1 つもない workspace（inferred project）は完全とみなさない。

- **R1（厳しい）**: 設定ファイルがちょうど 1 つで、それがフォルダの根にあり、かつその設定がフォルダの中のソースをすべて含むときだけ完全とみなす。ソースは `.ts` / `.tsx` / `.mts` / `.cts` と、JavaScript の `.js` / `.jsx` / `.mjs` / `.cjs` である。JavaScript のファイルが 1 つでもあれば、設定が `allowJs` を実効的に真にしていない限り（`jsconfig.json` は `compilerOptions.allowJs: false` を自分で書いていないとき、`tsconfig.json` は `compilerOptions.allowJs: true` を自分で書いているとき）、それらは project に入らず開くまで答えに現れないので、不完全とみなす（継承の `allowJs` は解かない）。単一の設定でも `files` / `include` / `exclude` で範囲が狭まれば、外れたファイルは開くまで答えに現れない（inferred project）。そこで R1 は根の設定の `files` / `include` / `exclude` を読み（コメントと末尾のカンマを除く小さな読み取り。依存は足さない）、既定の `include`（`**/*`）か、フォルダの中の全ファイルを含む `include` のときだけ完全とみなす。`extends` を持つ設定は、継承元を解かずに不完全とみなす（子が `files` / `include` を書いても、継承の `exclude` や `allowJs` が project の範囲を変えうる）。準拠テスト 7.2 の fixture（`include: ["**/*.ts"]`）と同じ形で、今の宣言の根拠をそのまま使える。solution で完全な monorepo でも約束を捨てる
- **R2（solution を読む）**: R1 の形とは別の形として、フォルダの根の tsconfig.json から `references` を推移的に辿って他のすべての設定ファイルに届き、かつ各設定ファイルとフォルダの根の間に別の tsconfig.json がないとき（tsserver の祖先探索が根の solution に届く）、かつ根の solution を除く各設定が `compilerOptions.composite: true` を自分で書いているとき（tsserver は出発点の project が composite でなければ祖先の solution を探さない）も完全とみなす。tsconfig.json はコメントと末尾のカンマを許すので、それを除く小さな読み取りを書く（依存は足さない）。宣言を広げる形なので、仕様 8.2 の 5 に従い solution の形の fixture を準拠テスト 7.2 に足して通してから宣言する（今回の測定では 16 / 16 で完全）。`disableSolutionSearching` / `disableReferencedProjectLoad` はどの設定にあっても祖先の solution の探索や子の読み込みを止めるので、どれかの設定がそれを書いていれば不完全とみなす。`extends` を持つ設定は継承元を解かないので、継承でこれらが入りうる以上、R2 では `extends` を持つ設定が 1 つでもあれば不完全とみなす（実際の monorepo の多くが `extends` を使うので、R2 の範囲はかなり狭い）。根の solution を除く各設定には、R1 の範囲の検査（`files` / `include` / `exclude`、JavaScript と `allowJs`）をそれぞれに当て、フォルダの中のソースがどれかの設定に含まれることを求める（根の solution は `files: []` の形も許す）

推奨は、R1 で宣言の誤りを先に止め、R2 は solution の fixture の準拠テストと一緒に続けて入れる、の 2 段。

## 会話の途中で配置が変わるとき（案 A）

宣言は `InitializeResult` で一度決まり、仕様には取り消す手段がない。一方で `coverage` の約束は `readiness` が `ready` の間に限る（仕様 6 章 1 項）。そこで、`coverage` を宣言した後に、判定の前提を崩しうる変化を lsp-det が受け取ったら、以後その接続の `readiness` を `unknown` にする（仕様 8.2 の 3。保留もやめる）。対象は次のとおり。

- `workspace/didChangeWatchedFiles` で、`tsconfig.json` / `jsconfig.json` の Created / Changed / Deleted
- 同じく、`allowJs` のない設定の下での JavaScript のファイルの Created
- `workspace/didChangeWorkspaceFolders` によるフォルダの追加

lsp-det に届かない変化（クライアントが通知しないディスク上の変更）は見えないので、この扱いの外に残る。仕様 6 章 2 項が、サーバー自身の監視で拾った変更を `freshness` の約束の外に置くのと同じ線引きである。この欠けは仕様 10 章の typescript-language-server の行に書く。

## 仕様 8.2 の 5 への書き足し（決めること 3）

「観測者は、テストを通した版であっても、workspace の形がテストの範囲の外にあると判定できるときは宣言してはならない」の旨を 8.2 の 5 に足す（仕様 1.1。11 章に記す）。typescript-language-server だけの事情ではなく、宣言の根拠（準拠テストの fixture）の外を観測者が見分けられるときの一般則になる。書き足さない場合、案 A は写像の実装の判断として残り、仕様は版だけを条件にしたままになる。

## 影響（案 A + R1 + 書き足しを採る場合）

- `src/tracker.rs` と写像の trait（`workspace_roots` を写像に渡す経路。今の `learn_workspace_folders` は `rootUri` を読まない `workspace_folders` を渡し、それに頼る Nextflow の写像があるので置き換えない）
- `src/adapter/typescript_language_server.rs`（`learn_workspace_folders` で設定ファイルの配置を読み、`guarantees` で `coverage` を外す）
- `tests/conformance.rs`（実サーバーの no-solution の workspace で `coverage` を宣言しないこと。今は RED）、偽上流のテスト（途中の設定ファイルの変化で `readiness` が `unknown` になること）
- 仕様 8.2 の 5 と 10 章の typescript-language-server の行（英日）、11 章
- `docs/research/typescript-language-server-readiness-measurement.md` の末尾、`README.md` / `README.ja.md` の該当箇所、`CHANGELOG.md`、`CLAUDE.md` の現在地

## 追補（2026-09-26）: 途中の変化の対象を新規のソース一般に広げる

実装（PR #119）のレビューで、決定 4 の対象の 2 つ目「`allowJs` のない設定の下での JavaScript のファイルの Created」では足りないことが分かった。規則 R1 は設定の `files` / `include` / `exclude` が全ソースを含むことも求めるので、たとえば `include: ["src"]` の設定の下で `src` の外に `.ts` が新しくできても、判定は不完全に変わる。`allowJs` が真の設定の下で `src` の外にできた `.js` も同じである。

2026-09-26 にユーザーが承認し、対象の 2 つ目を「`workspace/didChangeWatchedFiles` で、設定の project に入らないソースの Created」に改めた。入るかどうかは判定と同じ照合（`files` / `include` / `exclude`、JavaScript なら `allowJs`）で決める。`allowJs` の外の JavaScript はその一つの場合になる。1 つ目（設定ファイルの変化）と 3 つ目（フォルダの追加）は変えない。仕様 10 章の typescript-language-server の行は同じ PR で合わせてある。

## 追補（2026-09-26）: 設定ファイルの変化では配置を読み直す

決定 4 のままでは、配置が完全なままの設定の変更（`strict` を切り替える等）でも `readiness` が以後ずっと `unknown` になり、tsserver が読み込み直すあいだの横断リクエストも保留されない。設定ファイル 1 つの最も素直な workspace で、lsp-det が塞ぐはずの窓が開く。

2026-09-26 にユーザーが決め、設定ファイル（`tsconfig.json` / `jsconfig.json`）の Created / Changed / Deleted を受け取ったら、そのフォルダの配置をその時点のディスクで規則 R1 により読み直すことにした。

- まだ完全なら追跡を続ける。tsserver の読み込み直しの `$/progress` で `indexing` を経て `ready` に戻り、保留も効く
- 完全でなくなったら、今までどおり以後 `readiness` を `unknown` にする（一度 `unknown` にしたら戻さない）
- 同じ通知に含まれる新規のソースは、読み直した後の配置で判定する
- フォルダの追加は今までどおり `unknown` にする

通知はクライアントが保存した後に届くので、lsp-det が読むディスクは tsserver が読み直すものと同じである。フォルダを歩き直すのは設定ファイルの変化のときだけで、新規のソースはそのファイル 1 つを設定の範囲と照らすだけで済む。
