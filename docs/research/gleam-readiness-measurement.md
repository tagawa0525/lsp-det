# Gleam 言語サーバーの readiness の実測（M19）

ADR 0019 決定 F の M19。コーパス（[readiness-vocabulary-corpus.md](readiness-vocabulary-corpus.md)）は Gleam を「信号の不在の曖昧さ」型に置き、「依存ダウンロードがないときの `ready` を信号なしで言えるか」を疑問にしていた。実測とソースで、**信号は不在にならない**。`$/progress` "Downloading Gleam dependencies"（token `"downloading-dependencies"`）は、ダウンロードするものがなくても `initialized` の直後に begin → end する（12 ms）。要求は end の後に順に処理され、コンパイルは要求の中で同期に走るので、最初の要求から完全。Serena の「10 秒待って来なければ済みとみなす」は、来ないことがないので要らない。一方、`gleam.toml` の変更（`didChangeWatchedFiles`）の後は、エンジンが作り直されて同じトークンがもう一度 begin → end した後も **`references` が空のまま**になる（1.18.1 の不具合と見られる。原因はソースで追い切れていない）。

## 方法

- nixpkgs の gleam 1.18.1（Erlang/OTP 28。`flake.nix` の `servers`）。`gleam lsp`。2026-09-06
- 被験体: `gleam.toml`（依存なし、`target = "erlang"`）、`src/a.gleam`（`pub fn target()`）、`src/b.gleam`（`a.target()` を呼ぶ）。依存ありの被験体は `gleam_stdlib` を足したもの
- 道具: scratchpad の `lsp_probe.py`
- 走行: (1) 依存なし、(2) 依存あり・ウォーム、(3) 依存あり・コールド（hex からダウンロード）、(4) `ready` 後に開いていない `b.gleam` をディスクで書き換えて Changed、(5) 開いている `a.gleam` に `didChange` で同一モジュール内の呼び出しを足す、(6) `b.gleam` も開いて `didChange` で呼び出しを足す、(7) ディスクの書き換えを通知しない、(8)(9)(10) `gleam.toml` を touch して Changed（後に `didChange` を送る、問い合わせを疎にする、の変種）
- 裏付けにソース（`language-server/src/server.rs`、`engine.rs`、`progress.rs`、`router.rs`）を読んだ

## 結果

### 語彙

| 信号                                                            | 内容                                                                                                                                                                                                                                                                                             |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `$/progress`（token `"downloading-dependencies"`、create の後） | title "Downloading Gleam dependencies"。`initialized` の直後に必ず begin → end（依存なし 12 ms、依存ありウォーム 12 ms、コールド 0.35 秒）。エンジンが作り直されたとき（`gleam.toml` の Changed の後の最初の要求）にも begin → end。`progress.rs` の `dependency_downloading_started / finished` |
| `client/registerCapability`                                     | `workspace/didChangeWatchedFiles` を `**/gleam.toml` だけで登録する（id "watch-gleam-toml"）                                                                                                                                                                                                     |
| `textDocument/publishDiagnostics`                               | 要求のたびにコンパイルした結果（`respond` の `Compilation::Yes`）                                                                                                                                                                                                                                |

`serverInfo` は **null**。`window/logMessage` での名乗りもない。名乗りに当たるのはこのトークンの title だけ。版はどこにも現れない。コンパイルの開始と終了は `ProgressReporter` に `compilation_started / finished` の口があるが "Do nothing. This is only used for tests currently"（`progress.rs`）で、外には出ない。health の信号はない。

### 起動（走行 1〜3）

| 条件                          | "Downloading Gleam dependencies" | 最初の `references`                                              |
| ----------------------------- | -------------------------------- | ---------------------------------------------------------------- |
| 依存なし                      | 0.001 → 0.013 秒                 | 0.016 秒に 2 件（`b.gleam` を含む。完全）                        |
| `gleam_stdlib` あり、ウォーム | 0.002 → 0.013 秒                 | 0.017 秒に 2 件                                                  |
| 同、コールド                  | 0.002 → 0.355 秒                 | 0.380 秒に 2 件（0.001 秒に送った要求が end まで待たされて完全） |

サーバーは単一スレッドで順に処理し、要求の中でコンパイルする（`engine.rs` の `respond` / `compile`）。トークンの前に答えられる要求はなく、end の後の要求は完全。「信号の不在」は起きない。

### 変更の取り込み（走行 4〜7）

| 引き金（`ready` 後）                                              | 結果                                                                                               |
| ----------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| 開いている `b.gleam` に呼び出しを足す `didChange`（走行 6）       | 0.2 秒後の `references` が 3 件（2 → 3）。同期                                                     |
| 開いている `a.gleam` に同一モジュール内の呼び出しを足す（走行 5） | 2 件のまま。同一モジュール内の非修飾の呼び出しは `references` に数えない（鮮度ではなく意味の問題） |
| 開いていない `b.gleam` をディスクで書き換え、通知しない（走行 7） | 2 件のまま。開いていないファイルはディスクから読み直さない                                         |
| 同、`didChangeWatchedFiles` Changed を送る（走行 4）              | トークンが begin → end した後、**`references` が 0 件**（6 秒間）                                  |
| `gleam.toml` を touch して Changed（走行 8〜10）                  | 同じく 0 件。後に `didChange` を送っても、問い合わせを 3 秒間隔に疎にしても 0 件のまま             |

ソース: `didChangeWatchedFiles` はパスによらず `ConfigFileChanged` として `delete_engine_for_path` を呼び、次の要求でエンジンを作り直す（このとき依存のダウンロードのトークンが出る）。作り直した後の `references` が空になる理由はソースで追い切れていない（診断も出ない）。`freshness.fileChanges` は空にするしかなく、`gleam.toml` の Changed は取り込むどころか壊す。

## 写像（設計）

- **識別**: `serverInfo` がなく、`$/progress` の begin の title が "Downloading Gleam dependencies" なら Gleam（`identity_from_notification` に `$/progress` の経路を足す。既存は `window/logMessage` と `$/typescriptVersion`）。版は取れない
- **readiness**: `initializing` から、"Downloading Gleam dependencies" の begin で `initializing` のまま（初回）、end で `ready`。`ready` 後の begin（エンジンの作り直し）で `indexing`、end で `ready`。`didChange` は要求が同期で織り込むので先読みしない。`didChangeWatchedFiles` は取り込みの完了信号（作り直しのトークン）が次の要求まで来ず、来ても答えが壊れるので先読みしない
- **health**: 信号がなく `unknown`
- **coverage / freshness**: 宣言しない（`{}`）。要求ごとの全体コンパイルで 7.2 と 7.3 の 1 は通るが、版が語彙に現れず、通した版に限って宣言する（仕様 8.2 の 5）ことができない
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: `serverInfo`。`gleam.toml` の変更の後に `references` が空になる不具合。`compilation_started / finished` を `$/progress` に出す

## コーパスへの反映

「信号の不在の曖昧さ」型は Gleam では成り立たない。トークンはダウンロードするものがなくても出るので、不在は起きない。疑問は「`gleam.toml` の変更でエンジンを作り直した後に答えが壊れる」に置き換わる。
