# crystalline（Crystal）の readiness の実測（M17）

ADR 0019 決定 F の M17。コーパス（[readiness-vocabulary-corpus.md](readiness-vocabulary-corpus.md)）は crystalline を「per-request / per-file の進捗」型に置き、「起動系と per-request 系のトークンを区別できるか」を疑問にしていた。実測とソースで、**起動系のトークンはなく、`$/progress` はすべてリクエスト単位のコンパイル**（title "Building project"、message は entry point のパス）。要求はそのコンパイルを同期で待ってから答えるので、起動直後の要求も完全で、per-request のトークンを readiness に写す必要はない。readiness の語彙は `initialized` の直後の `window/logMessage` "LSP server is ready." で、その前に要求が答えられる窓はない。開いていないファイルのディスク上の変更は結果キャッシュのせいで織り込まれない（監視もしない）。Serena の 10 秒の sleep（`_MIN_COMPILATION_DELAY`）は根拠がない。

## 方法

- nixpkgs の crystalline 0.18.0（Crystal 1.19.1。`flake.nix` の `servers`）。`crystalline`（引数なしで stdio）。2026-09-06
- 被験体: `shard.yml`（target `fixture` の main が `src/fixture.cr`）、`src/a.cr`（`def target`）、`src/b.cr`（`def x` が `target` を呼ぶ）、`src/fixture.cr`（`require` 2 つと `puts x`）
- crystalline は `textDocument/references` も `workspace/symbol` も持たない（definition / hover / completion / documentSymbol / formatting だけ）ので、横断の要求は `textDocument/definition`（`b.cr` の `target` → `a.cr`、`fixture.cr` の `x` → `b.cr`）で測った。道具は scratchpad の `lsp_probe.py` に `--query-method definition` を足したもの
- 走行: (1) `b.cr` を開いて `target` の definition を 0.5 秒間隔、(2) `ready` 後に開いていない `b.cr` をディスクで書き換えて（`def x` を 2 行目から 5 行目へ）Changed、(3) `b.cr` も開いて同じ書き換えを `didChange` で、(4) `a.cr` に構文エラーを入れる
- 裏付けにソース（`src/crystalline/workspace.cr` の `compile` / `update_document` / `save_document`、`progress.cr`、`controller.cr`、`result_cache.cr`）を読んだ

## 結果

### 語彙

| 信号                                                        | 内容                                                                                                                                                                                                                                                                        |
| ----------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `window/logMessage`（type 3）                               | `initialized` の直後（0.02 秒）に "\"[workspace] Found projects:\n\<path\>"（先頭の引用符はソースのヒアドキュメントのまま。`shard.yml` のプロジェクトが見つかったときだけ）、**"LSP server is ready."**、"Flags for project …: []"、コンパイルのたびに "compiler_flags: []" |
| `$/progress`（token は "workspace/compile/N"、create の後） | **要求ごとのコンパイル**。title "Building project"（entry point がある）または "Building"（ない）、message は対象のパス、end の message は "Completed successfully." または "Completed with errors."。`progress.cr` の汎用ラッパーを `compile` が使う                       |
| 応答                                                        | definition は `compile(in_memory: true)` を**同期で待って**から答える（初回 0.65 秒、キャッシュが効けば 4 ms）                                                                                                                                                              |

`serverInfo` は **null**。`InitializeResult.capabilities` は `textDocumentSync: 2` と 5 つの provider だけで、名乗りに使えるものは `"[workspace] Found projects:"` のログしかない。版はどこにも現れない。監視対象の登録（`client/registerCapability`）はない。health の信号はない。

### 起動直後から完全（走行 1）

| 時刻（秒）   | 出来事                                                                                                                                                 |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 0.019        | `initialize` 応答（`serverInfo` なし）。`initialized`、`didOpen src/b.cr`                                                                              |
| 0.020        | "[workspace] Found projects:"、**"LSP server is ready."**、"Flags for project …" のログ                                                                |
| 0.334〜0.664 | 最初の definition が "Building project" のトークンを begin → end（"Completed successfully."）させ、**その後に** `a.cr` の 1 件を返す（0.648 秒待った） |
| 0.670〜      | 2 回目以降は 4 ms で同じ 1 件（結果キャッシュ）                                                                                                        |

要求がコンパイルを待つので、起動直後の空応答も部分応答もない。トークンは要求の副作用で、readiness ではない（8 章の規則「per-request の進捗は readiness にしない」のとおり）。

### 開いている文書の `didChange` は同期（走行 3）

`b.cr` を開いて `def x` を 5 行目へ動かす全文の `didChange` を送ると、次の definition（`fixture.cr` の `x`）は "Building project" を begin → end させてから `b.cr` の 5 行目を返す（0.5 秒）。`update_document` が結果キャッシュを無効化し（entry point と当該ファイル）、次のコンパイルが開いている文書の内容（`file_overrides`）で走る。7.3 の 1 は通る。

### 開いていないファイルのディスク上の変更は織り込まない（走行 2）

開いていない `b.cr` をディスクで書き換えて（`def x` を 5 行目へ）Changed を送っても、definition は **8 秒間 2 行目のまま**（結果キャッシュ。`didChangeWatchedFiles` を登録も処理もしない）。無効化は `didChange` / `didSave` / `didClose` だけ。`freshness.fileChanges` は空になる。

### コンパイルが失敗すると definition は空（走行 4）

`a.cr` に構文エラーを入れると、要求ごとに "Building project" が "Completed with errors." で end し、definition は **空配列**（`fixture.cr` の `x` は壊れていない `b.cr` にあるのに）。ユーザーのコードの誤りで、サーバーの health ではない（診断が出る）が、エージェントには「見つからない」と「コンパイルできない」の区別がつかない。上流に、コンパイルが失敗した要求はエラー応答にすることを求める候補。

## 写像（設計）

- **識別**: `serverInfo` がなく、`window/logMessage` が `"[workspace] Found projects:` で始まれば crystalline（既存の `identity_from_notification` の経路。pyright / tsls と同じ）。`shard.yml` のプロジェクトがない root では出ず、両軸 `unknown` のまま。版は取れない
- **readiness**: `initializing` から、`window/logMessage` "LSP server is ready." で `ready`。`$/progress` は要求単位なので写さない（開いている間 `indexing` にすると、要求が自分のコンパイルを待つだけなのに他の要求を止める）。先読みはしない（`didChange` は次の要求が同期で織り込む。監視対象の変更は織り込まれず、完了の信号もない）
- **health**: 信号がなく `unknown`。"Completed with errors." はユーザーのコードの診断
- **coverage / freshness**: 宣言しない（`{}`）。要求ごとの全体コンパイルなので `coverage` は満たし、`didChange` も同期だが、版が語彙に現れず、通した版に限って宣言する（仕様 8.2 の 5）ことができない。準拠テスト 7.2 / 7.3 は `references` で測るので、definition だけのサーバーには definition で測る
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: `serverInfo`。コンパイルが失敗した要求のエラー応答。`didChangeWatchedFiles` の登録と結果キャッシュの無効化

## コーパスへの反映

「per-request / per-file の進捗」型のとおりで、疑問「起動系と per-request 系のトークンを区別できるか」の答えは「起動系のトークンはなく、区別は要らない。readiness は起動ログにある」。Serena の 10 秒の sleep は、要求がコンパイルを同期で待つ以上、何も埋めていない。
