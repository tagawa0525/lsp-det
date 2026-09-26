# typescript-language-server: solution のない multi-project の workspace で coverage の宣言が破れる（2026-09-23）

Serena の #1937 の再現（[serena-request-path-state-prototype.md](serena-request-path-state-prototype.md)）の副産物として、lsp-det 自身の typescript-language-server の写像が、workspace の形によっては守れない `coverage` を宣言していることが分かった。本報告はそれを lsp-det 経由で測り直し、原因を tsserver のソースで確かめる。扱いは [ADR 0023](../adr/0023-typescript-coverage-by-workspace-layout.md)（2026-09-25 採用。workspace の tsconfig.json / jsconfig.json の配置を読み、完全とみなせる形のときだけ `coverage` を宣言する）。

## 結論

- lsp-det の typescript-language-server の写像は、検証済みの版（TypeScript 5.9.3）なら workspace の形によらず `coverage: {scope: "workspace", incomplete: {}}` を宣言する。**solution（他の project を `references` に持つ tsconfig.json）のない multi-project の workspace では、`ready` の後の `textDocument/references` が不完全で、別のファイルを開いて project が読み込まれると、`indexing` → `ready` を経て同じ問い合わせの結果が増える**。仕様 6 章 1 項（`ready` の間は、範囲のインデックスの進行で後から結果が増えない）と 7.2 の 1（完全な結果と一致する）に反する宣言である
- 原因は tsserver の設計で、不具合ではない。references が探すのは「読み込み済みの project」と「読み込み済みの solution から `references` で辿れる project」だけで、読み込まれるのは開いたファイルの project と、その祖先ディレクトリの tsconfig.json（composite の project から辿る）だけである（下の「tsserver のソース」）
- 準拠テスト 7.2 の fixture は tsconfig.json が 1 つの workspace（`tests/support/mod.rs` の `TSCONFIG` + `a.ts` / `b.ts`）で、この形を試していない。仕様 8.2 の 5 は宣言を版で決め、workspace の形を見ない
- 信号（`$/progress` の begin / end）は正しく、保留も効く。壊れているのは保証の宣言であって、`readiness` ではない
- 実物の pnpm/pnpm（tsconfig.json 354 個、workspace の根にはない）でも同じ振る舞い（247 ファイル中 0 → 別のファイルを開くと 22）。この repo の TypeScript は 6.0.3 で、lsp-det は未検証の版として保証を宣言しない（`serverStateProvider: {}`）ので、今日の lsp-det の宣言の誤りではない。6.0.3 を今の準拠テストで通して `TESTED_VERSIONS` に足すと誤りになる

## 方法

- probe: `scripts/typescript/coverage-probe.py`。サーバーのコマンド（既定は `lsp-det -- typescript-language-server --stdio`）を stdio で起動し、`experimental.serverState` を宣言するクライアントとして振る舞う（宣言したクライアントを lsp-det は保留しないので、probe が自分で `ready` を待つ。仕様 9 章の挙動）。`InitializeResult` の宣言を出し、対象のシンボルのファイルを開いて `ready` を待ち、`textDocument/references`（宣言を含めない）を送る。`--open-after` で 2 つ目のファイルを開き、それが起こす読み込み（`indexing` の後の最初の `ready`）を待って同じ問い合わせを送る。読み込みの起きない open と信号の来ない読み込みは見分けられないので、`--no-load`（呼び出し側が「開くファイルは読み込み済みの project にある」と宣言する）のときだけ `--observe` 秒（既定 120）`indexing` が来ないことを受け入れ、そうでなければ測定は無効として止める。tsserver は project を 1 つずつ読み込み、lsp-det は読み込みの間に `ready` を報告するので、2 回目の数は「開いたことで増えた分」の下限で、問いは増えるかどうかだけ。`health` が `error`（tsserver の終了）になったら測定は無効として止める。使われた TypeScript の版は `window/logMessage` の "Using Typescript version" で出す
- lsp-det: main `26989d0` の release ビルド。typescript-language-server 5.3.0。TypeScript は flake の 5.9.3（user-setting。保証を宣言する組）、pnpm/pnpm だけ workspace の 6.0.3
- workspace:
  - **single**: tsconfig.json 1 つ（`include: ["*.ts"]`）と `a.ts`（`export function target`）、それを使う `b.ts`、`c.ts`。準拠テストの fixture と同じ形
  - **solution**: `scripts/serena/ts-monorepo-fixture.py --packages 8 --files 80`（相対 import、661 ファイル）。共有パッケージ 1 つと、それを使うパッケージ 8 つ（各 80 ファイル、うち 2 ファイルが対象を使う）。各パッケージは自前の tsconfig.json（`composite: true`）を持ち、workspace の根の tsconfig.json が全パッケージを `references` に持つ。完全な答えは 16 ファイル
  - **no-solution**: 同じで `--no-solution`（根の tsconfig.json を置かない）
  - **pnpm/pnpm**: `a8ade49`（`pnpm install --frozen-lockfile --ignore-scripts`）。対象は `pnpm11/core/error/src/index.ts` の `PnpmError`（2 行目 13 列、0 始まり）。2 つ目に開くのは `pnpm11/auth/commands/src/logout.ts`。完全な答えの目安は、`*/src/` の下で識別子 `PnpmError` を含む `.ts` の数（宣言のファイルを除いて 247。各パッケージの tsconfig.json の `include` は `src/**/*.ts` なので、テストのファイルは入らない）。文字列の数え上げで、厳密な答えではない

## 結果

| workspace   | TypeScript | 宣言                                     | 対象を開いて `ready` の後 | 2 つ目のファイルを開いた後（最初の `ready`。下限）                              |
| ----------- | ---------- | ---------------------------------------- | ------------------------- | ------------------------------------------------------------------------------- |
| single      | 5.9.3      | `coverage: {scope: "workspace"}`         | 2 / 2                     | 2 / 2（120 秒 `indexing` なし。今の probe では `--no-load` を付けて再現する）   |
| solution    | 5.9.3      | `coverage: {scope: "workspace"}`         | 16 / 16                   | 16 / 16（120 秒 `indexing` なし。今の probe では `--no-load` を付けて再現する） |
| no-solution | 5.9.3      | `coverage: {scope: "workspace"}`         | **0 / 16**                | **2 / 16**（`indexing` → `ready` の後）                                         |
| pnpm/pnpm   | 6.0.3      | `{}`（未検証の版なので保証を宣言しない） | 0 / 247                   | 22 / 247（この実行全体では `indexing` → `ready` が 19 回続いた）                |

どの行も、問い合わせは `readiness` が `ready` の状態で送っている。no-solution では、宣言が約束する「`ready` の間は後から結果が増えない」が、クライアントがファイルを 1 つ開くだけで破れる。pnpm/pnpm の 1 回目の 2 箇所は宣言のファイル自身の中で、表からは除いている。

## tsserver のソース

TypeScript 5.9.3（flake の `typescript/lib/typescript.js`）と 6.0.3 で、以下の関数と条件が同じであることを確かめた。

- **references が探す project**（`getReferencesWorker`）: 要求のファイルを含む project を探した後、`projectService.loadAncestorProjectTree(searchedProjectKeys)` を呼び、`projectService.forEachEnabledProject` で読み込み済みの project を順に探す。まだ読み込まれていない project は探さない
- **solution から辿る読み込み**（`loadAncestorProjectTree` → `ensureProjectChildren`）: 読み込み済みの configured project それぞれについて、`references` で解決した子のうち、探した project を（推移的に）参照するものを読み込む。`disableReferencedProjectLoad` があれば辿らない
- **solution の見つけ方**（`forEachAncestorProjectLoad`）: ファイルを開いたとき、その project の tsconfig.json の親ディレクトリから上に向かって最も近い tsconfig.json を「solution の候補」として読み込み、さらに上へ続ける。ただし出発点の project が `composite` でなければ探さない（`searchOnlyPotentialSolution && !composite` で戻る）。`disableSolutionSearching` があれば探さない

したがって、ある project の参照が答えに入るのは、その project が読み込み済みであるか、読み込み済みの solution から `references` で辿れるときに限る。solution のない workspace では、開いていないパッケージの project は読み込まれず、その中の参照は答えに現れない。開けば読み込まれて現れる。

## Serena の #1937 との関係

[serena-request-path-state-prototype.md](serena-request-path-state-prototype.md) で再現した #1937 の揺れ（直前の呼び出しから 100 ms 以内の問い合わせが 13 ファイル中 1）は、参照先のファイルを open / close した後に tsserver が project を解放して読み直す間の一過性の窓で、`$/progress` が出るので状態を見れば塞げる。本報告の不完全さは時間が経っても直らない安定したもので、信号がない。同じ solution のない形で両方が起きる。

## 読み

- 宣言の誤りは準拠テストの fixture の形に由来する。7.2 は tsconfig.json が 1 つの workspace で通り、その結果が workspace の形によらず宣言に使われている。8.2 の 5 は「テストを通した版の範囲を超えて宣言してはならない」と版だけを見ていて、workspace の形がテストの範囲の外にあることを想定していない
- lsp-det は写像を選んだ時点で `initialize` の `workspaceFolders` を知っている（`src/tracker.rs` の `adopt` が `learn_workspace_folders` を呼んでから `provider` が宣言を返す）。tsserver と同じ規則で workspace の tsconfig.json の配置を読めば、宣言を workspace ごとに決められる。扱いの選択肢と採った案は ADR 0023
- 保留（`indexing` の間の 7.0 の要求を待たせる）はどの形でも効く。宣言をやめても、#1937 のような一過性の窓は塞げる

## 一般化してはならない点

- workspace は 4 つ。完全だった 2 つ（single、solution）と不完全だった 2 つ（no-solution、pnpm/pnpm）の違いは tsserver のソースから読んだ規則と一致するが、中間のディレクトリにある tsconfig.json、`extends` で継承した `composite`、`disableSolutionSearching` / `disableReferencedProjectLoad`、tsconfig.json のない workspace（inferred project）は測っていない
- 測ったのは `textDocument/references` だけ。7.0 の他のメソッド（`workspace/symbol`、`rename`、call hierarchy 等）も読み込み済みの project だけを探すと読めるが、測っていない
- pnpm/pnpm の完全な答えの数は文字列の数え上げで、型を通した厳密な数ではない

## 実装（2026-09-25）

ADR 0023 の案 A と規則 R1 を写像に入れた（`src/adapter/typescript_layout.rs`）。検証済みの版でも、workspace のフォルダがちょうど 1 つで、その根に設定ファイルが 1 つだけあり、`extends` を持たず全ソースを含むときだけ `coverage` を宣言し、それ以外では `freshness` だけを宣言する。本報告の solution のない workspace を実サーバーで当てると `coverage` を宣言しなくなった（`tests/conformance.rs` の `typescript_language_server_declares_no_coverage_without_a_solution_with_real_server`）。7.2 の fixture（設定ファイル 1 つ）では従来どおり宣言し、7.2 / 7.3 も通る。`coverage` を宣言した後にクライアントが設定ファイルの変化を知らせると、tsserver が読み込みをやり直しても `readiness` は `unknown` のまま戻らない（決定 4）。仕様は 1.1 に上げ、8.2 の 5 と 10 章の typescript-language-server の行を改めた。

フォルダが複数の workspace でも同じ構造の穴があることを測り（フォルダ A と B に全ソースを含む tsconfig を 1 つずつ置き、B が A を呼ぶ配置で、A だけを開くと references が空。tsls 5.3.0）、フォルダがちょうど 1 つのときだけ宣言するよう改めた（ADR 0023 追補 2026-09-26、`tests/conformance.rs` の `typescript_language_server_declares_no_coverage_for_several_folders_with_real_server`）。
