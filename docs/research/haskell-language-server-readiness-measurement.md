# haskell-language-server の readiness の実測（M15）

ADR 0019 決定 F の M15。コーパス（[readiness-vocabulary-corpus.md](readiness-vocabulary-corpus.md)）は HLS を「複数トークンの並行」型（"Indexing" と "Processing" の 2 本の `$/progress`）に置き、「未完了トークン 0 で `ready` にして 7.2 が通るか」を疑問にしていた。実測とソースで、**トークンは readiness の語彙ではない**。通常モードでは lsp ライブラリの `optProgressStartDelay = 1 秒`（「1 秒たてば人は遅いと感じ始める」というコメント付き）で 1 秒未満のセッションは一切出ず、セッションは kick ごと・索引バッチごとに作り直されるので、200 モジュールを 8 秒かけて索引する間もトークンはほぼ出ない。その間 `references` は **部分的な結果を返し続け、増えていく**。`--test` で遅延を 0 にしても、索引のトークンはバッチごとに開閉し、閉じている隙間に結果は不完全で、最後のトークンが閉じた後も 12 秒索引が続く。readiness の正直な写像は `unknown`（仕様 8.2 の 3）。health は cradle の失敗が診断（`source: "cradle"`）で分かる。

## 方法

- nixpkgs の haskell-language-server 2.13.0.0（GHC 9.10.3、cabal-install 3.16.1.0。`flake.nix` の `servers`）。`haskell-language-server-wrapper --lsp`。2026-09-06
- 被験体: `cabal-version: 2.4` の library（`hie.yaml` は `cradle: cabal:`）。`src/A.hs`（`target :: Int`）、`src/B.hs`（`x = target + 1`）。大きい被験体は、`A.target` を使う関数を 120 個持つモジュール `H001` … `H200` を連鎖 import で足した 202 モジュール（コールドで 8 秒）。300 モジュールの軽い連鎖も使った
- 道具: scratchpad の `lsp_probe.py`。`workspace/configuration` には `{"checkProject": true}` を返す（`null` を返すと "parsing settings failed" の警告が出る。挙動は変わらない）。キャッシュは `~/.cache/ghcide/` の hiedb（`<hash>-<project>-<ghc>-2.hiedb`）と interface ファイル（`<package>-inplace-<hash>/`）で、コールドにするには両方を消す
- 走行: (1) 2 モジュール、(4) 300 モジュールの連鎖、(5)(8) 202 モジュールをコールドで、(6)(13) 同じ被験体を `--test` で、(7) hiedb を残したウォーム、(9) 開いている `B.hs` に `didChange` で使用を足す、(10) 開いていない `B.hs` をディスクで変えて Changed、(11) サーバー停止中に `B.hs` を変えてから起動、(12) `hie.yaml` に存在しない component を書いて cradle を壊す
- 裏付けにソース（`ghcide/src/Development/IDE/Main.hs`、`Core/ProgressReporting.hs`、`Core/Shake.hs`、`Core/OfInterest.hs`、`Core/Compile.hs`、`Types/Options.hs`）を読んだ

## 結果

### 語彙

| 信号                                      | 内容                                                                                                                                                                                                                                                                                                              |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `$/progress`（token は整数、create の後） | title "Processing"（kick = 開いているファイルの型検査。message "1/2" など）、"Indexing"（hiedb への書き込み。バッチごとに開閉）、"Setting up \<project\> (for \<file\>)"（cradle の読み込み）。**セッションが 1 秒続かないと出ない**（`Main.hs` の `LSP.optProgressStartDelay = 1_000_000`）。`--test` では遅延 0 |
| `textDocument/publishDiagnostics`         | 型検査の結果。cradle の読み込みに失敗すると、開いたファイルに `source: "cradle"`、severity 1 の診断（message "Failed to run cabal v2-repl …"）                                                                                                                                                                    |
| `client/registerCapability`               | `workspace/didChangeWatchedFiles` を `**/*.hs`、`**/*.hs-boot`、cabal ファイル等で動的登録する（id "globalFileWatches"）                                                                                                                                                                                          |
| `--test` だけの通知                       | `kick/start` / `kick/done`（対象ファイルの配列）、`ghcide/cradle/loaded`、`ghcide/reference/ready`（ファイルごと。総数は分からない）                                                                                                                                                                              |

`serverInfo` は **null**。`window/logMessage` での名乗りもない（版は stderr にしか出ない）。`InitializeResult.capabilities.executeCommandProvider.commands` は `"<pid>:ghcide-type-lenses:typesignature.add"` のように pid とプラグイン名を前置した形で、`ghcide-` を含む命令が HLS の名乗りに当たる。`capabilities.experimental` はない。

### 通常モードではトークンがほぼ出ず、`references` は増え続ける（走行 5、8）

202 モジュール、コールド、`didOpen src/A.hs`、`checkProject: true`。

| 時刻（秒） | 出来事                                                                                                         |
| ---------- | -------------------------------------------------------------------------------------------------------------- |
| 0.61       | `initialize` 応答（`didOpen` の後、`workspace/configuration` と `client/registerCapability`）                  |
| 1.04       | `references` が **3 件（`A.hs` の中だけ）**                                                                    |
| 1.15〜7.62 | `references` が 249 → 2323 → 4885 → … → 22819 と **13 回とも違う数を返す**（hiedb に索引されたぶんだけ答える） |
| 5.87〜6.61 | "Indexing" のトークンが 1 本だけ（begin の message "12/36"、end まで 0.74 秒）。索引全体の一部しか覆わない     |
| 8.19       | 24405 件（完全）。以後変わらない                                                                               |

ソース: `progressReportingNoTrace` は `ProgressNewStarted` で走っているセッションを **cancel して作り直す**（`updateState`）。kick は開いているファイルが変わるたびに `ProgressNewStarted` を送り、索引は `pending` が空になるたびに `ProgressCompleted` を送るので、セッションが 1 秒続くことは稀で、lsp ライブラリは 1 秒未満のセッションのトークンを作らない。2 モジュール（走行 1）と 300 モジュールの軽い連鎖（走行 4。211 件 → 605 件の部分応答あり）では、トークンは 1 本も出なかった。

**未完了トークンが 0 でも索引は終わっていない。トークンの不在は「済み」でも「未着手」でもない。** 空応答ではなく部分応答なので、`ready` を名乗る観測者はもちろん、`unknown` の観測者も嘘を消せない。

### `--test` でも隙間に不完全（走行 6、13）

`--test`（"Enable additional lsp messages used by the testsuite"）は進捗の遅延を 0 にし、`kick/start` / `kick/done` / `ghcide/cradle/loaded` / `ghcide/reference/ready` を送る。0.1 秒間隔で問うと:

- "Processing" の begin → "Setting up fixture_heavy (for src/A.hs)" の begin → end（0.16 秒）→ "Processing" end → `kick/done`（1.07 秒）。この時点の `references` は 3 件（開いたファイルの中だけ）
- "Indexing" は **ファイルごとに 45 本**、1 本 15 ms、間に 10 ms の隙間。隙間では未完了トークンが 0 で、`references` は 127 → 615 → 1103 → … と不完全
- 最後の "Indexing" の end は 4.07 秒。`ghcide/reference/ready` はその後も 15.9 秒まで来続け（173 回）、`references` は 10.6 秒でまだ 16963 件（完全は 24405）

「未完了トークン 0 で `ready`」は `--test` でも成り立たない。`reference/ready` はファイル単位で、総数を伝える信号がない。

### 開いている文書の `didChange` は同期（走行 9）

開いている `B.hs` に `target` の使用を足す `didChange` の 0.1 秒後、`B.hs` からの `references` は 6 件（5 → 6）。ファイルの型検査は要求と同期する（`useWithStale` ではない経路）。7.3 の 1 は素の HLS で通る。

### 監視対象の変更は取り込まない（走行 10）

開いていない `B.hs` に使用を足して Changed を送っても、30 秒で `references` は 5 件のまま。`**/*.hs` を動的登録しているが、開いていないファイルの索引は更新しない。

### ウォーム起動は古い索引で答える（走行 11）

サーバー停止中に `B.hs` に使用を 2 つ足してから起動すると、最初の `references`（0.75 秒）は **古い 5 件**、次（0.85 秒）から 7 件。hiedb を読み込んだ直後は停止中の変更を知らない。窓は 0.1 秒で、信号はない。

### cradle が壊れると診断で分かる（走行 12）

`hie.yaml` の component を存在しないものにすると、`didOpen` の 0.05 秒後に `A.hs` へ `source: "cradle"`、severity 1、message "Failed to run cabal v2-repl … 'lib:doesnotexist' …" の診断が 1 件出て、以後 `references` は空配列（30 秒）。**壊れたサーバーの成功風応答**そのもので、診断が health の信号になる。回復は同じファイルに cradle の診断のない `publishDiagnostics` が来ること。cradle の読み込みに成功したことを伝える信号は通常モードにはない（`ghcide/cradle/loaded` は `--test` だけ、"Setting up" のトークンは 1 秒未満なら出ない）ので、`ok` は観測できない。

## 写像（設計）

- **識別**: `serverInfo` がなく、`capabilities.executeCommandProvider.commands` に `<数字>:ghcide-` で始まる命令があれば HLS。版は取れない
- **readiness**: `unknown`（仕様 8.2 の 3、8.4 の 1）。トークンは時間で抑制され、不在が「済み」を意味しないので、`initializing` に留めることも、トークンの end で `ready` にすることもできない。トークンが開いている間だけ `indexing` にする案は、end で `unknown` に戻るだけで保留の意味がなく、採らない
- **health**: `unknown` から、`source: "cradle"` の error の診断で `error`（message は診断の 1 行目）。同じ URI に cradle の診断のない `publishDiagnostics` が来たら `unknown` に戻す（`ok` の観測はない）
- **coverage / freshness**: 宣言しない（`{}`）。readiness を観測しないので 7.2 / 7.3 の約束はない
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: `serverInfo`。索引の完了を伝える信号（`--test` の `kick/done` と `reference/ready` に「総数」を足して通常モードでも送る、または本プロトコルを話す）。1 秒の抑制は UI のためのもので readiness には使えない

## コーパスへの反映

「複数トークンの並行」の型は HLS では成り立たない。並行どころか、トークンは時間で抑制されてほぼ出ず、出ても索引の一部しか覆わない。疑問は「時間で抑制された進捗は readiness の語彙ではない」に置き換わる。`--test` は opt-in の信号（決定 G）に見えるが、起動フラグは観測者が注入できるものではなく、注入できても総数がないので `ready` を言えない。
