# nil の readiness の実測（M26）

ADR 0021 決定 C の M26。nil（oxalica/nil）は flake.lock を読んで入力の store path を解決し、`nixpkgs` の入力があれば NixOS のオプションを `nix` で評価する。ソースは読み込みと評価に `$/progress` を持つが、flake.lock を読む段には信号がない。被験体の flake で測り、**flake.lock の読み込みが済むまでの約 100 ms は入力への `definition` が空で、その窓の直後に来るオプションの評価の begin が最初の信号**と確かめた。flake.nix も flake.lock も `nixpkgs` の入力もない workspace では信号が 1 つも来ない（clangd の compile_commands.json のない場合と同型）。

7.0 の横断要求のうち索引（flake の情報）に依るのは `inputs.<名前>` への `definition` だけで、`references` は要求のあった文書の中に閉じる（ADR 0021 決定 D）。要求は snapshot に対して答え、待たない。health は `window/showMessage` の type 1 / 2 に写せる。

## 方法

- nixpkgs の nil 2026-07-23（`serverInfo` は `{"name": "nil", "version": "2026-07-23"}`。`flake.nix` の `servers`）。`nix` はシステムのもの。2026-09-08
- 被験体: `flake.nix`（`inputs.nixpkgs.url` を固定中の rev に固定、`outputs = { self, nixpkgs }:` で `pkgs.hello`）と `nix flake lock` で作った `flake.lock`（store path は開発環境が既に持つ）、`module.nix`（NixOS のモジュール）。変種として、入力のない flake、`flake.lock` のない flake、`flake.lock` の `narHash` を壊して store path が存在しない flake
- 道具: scratchpad の `lsp_probe.py`（横断の判定を URI の完全一致に直した）。クライアントは `window.workDoneProgress` と `workspace.didChangeWatchedFiles.dynamicRegistration` を宣言し、`window/workDoneProgress/create`、`workspace/configuration`（既定は `{}`）、`client/registerCapability` に答える。走行 7 だけ `window.showMessage.messageActionItem` も宣言し、`showMessageRequest` に最初の項目（Fetch）で答える
- 走行: (1) 起動して `flake.nix` を開き `inputs` の `nixpkgs` の `definition` を 50 ms ごとに送る（10 ms でも）、(2) 入力のない flake、(3) `flake.lock` のない flake、(4) store path のない入力（`showMessageRequest` に答えないクライアント）、(5) `nix.binary` を存在しないパスにする、(6) `references`（`pkgs` の束縛）を `module.nix` も開いた状態で、(7) (4) を `showMessageRequest` に答えるクライアントで、(8) `ready` の後に `flake.lock` を書き換えて `didChangeWatchedFiles` Changed
- 裏付けに `reference/nil` を読んだ（`crates/nil/src/server.rs`、`capabilities.rs`、`config.rs`、`crates/ide/src/ide/{goto_definition,references}.rs`、`crates/ide/src/ty/convert.rs`）

## 結果

### 語彙

| 信号                                   | 内容                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `InitializeResult.serverInfo`          | `{"name": "nil", "version": "2026-07-23"}`。版が語彙に現れる（nixpkgs のビルドは `CFG_RELEASE` を入れる）                                                                                                                                                                                                                                                                                                                                                           |
| `client/registerCapability`            | `initialized` の直後に `workspace/didChangeWatchedFiles` を `flake.nix` と `flake.lock` の 2 つの glob で登録する（クライアントが `dynamicRegistration` を宣言したとき）                                                                                                                                                                                                                                                                                            |
| `workspace/configuration`              | `initialized` の直後に `{"section": "nil"}` で 1 度                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `$/progress`                           | token は固定の文字列 3 つ。`nil/loadNixosOptionsProgress`（title "Loading NixOS options from 'nixpkgs'"。`nixpkgs` の入力の store path があるとき）、`nil/flakeArchiveProgress`（title "Fetching flake with inputs"。`showMessageRequest` に Fetch と答えたとき）、`nil/loadInputFlakeProgress`（title "Evaluating input flakes"。設定 `nix.flake.autoEvalInputs` が true のとき。既定は false で観測していない）。begin → end。同じ token の begin の前に `create` |
| `window/showMessage`                   | type 1: "Failed to load flake workspace: …"（走行 5）、"Failed to archiving flake: …"（走行 7）ほか。type 2: "Some flake inputs are not available, please run `nix flake archive` …"（走行 4）ほか                                                                                                                                                                                                                                                                  |
| `window/showMessageRequest`            | type 3（INFO）"Some flake inputs are not available. Fetch them now?"、actions Fetch / Ignore missing ones。クライアントが `window.showMessage.messageActionItem` を宣言したときだけ。答えが来るまで読み込みは進まない                                                                                                                                                                                                                                               |
| エラー応答 `-32800` "Client cancelled" | `didOpen` の直後の最初の要求に返る（走行 1、6、7 で毎回）。salsa の書き込みが走っている間の問い合わせは取り消され、クライアントが `$/cancelRequest` を送っていなくてもこのコードになる                                                                                                                                                                                                                                                                              |

### 起動と索引に依る要求（走行 1）

| 時刻   | 出来事                                                                                                                                            |
| ------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0.001s | `initialize` の応答。`initialized` の直後に `workspace/configuration` と `client/registerCapability`。`didOpen` の直後の `definition` は `-32800` |
| 0.104s | `definition` → 0 件（flake.lock はまだ読まれていない。信号なし）                                                                                  |
| 0.116s | `definition` → 1 件（`/nix/store/<hash>-source/flake.nix`）。直後に `create` + begin "Loading NixOS options from 'nixpkgs'"（10 ms 間隔の走行）   |
| 0.795s | end                                                                                                                                               |

flake.lock の読み込み（`initialized` の 100 ms の debounce の後）が済むまで、入力への `definition` は空で、信号はない。読み込みが済むと `SetFlakeInfoEvent` が主ループに入り、その後にオプションの評価の begin が出る。10 ms 間隔で見ても begin の後に空の答えはなく、**begin は「flake の情報が入った」の信号として使える**。オプションの評価（0.7 秒前後）は completion と hover に効き、7.0 の要求には効かない。

### 信号が出ない条件（走行 2、3、4）

| 条件                                     | 見えるもの                                                                                                          |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| 入力のない flake（`outputs = { self }`） | 何も来ない。`definition` は 0 件のまま                                                                              |
| `flake.lock` のない flake                | 何も来ない                                                                                                          |
| 入力の store path がない                 | `window/showMessage` type 2（`nix flake archive` を促す）だけ。オプションの評価は走らず、`definition` は 0 件のまま |

flake.nix のない普通のディレクトリでも同じで、信号は 1 つも来ない。clangd の compile_commands.json のない場合（ADR 0020 決定 (a)）と同型で、begin が来なければ `initializing` のまま。

### 失敗の見え方（走行 5、7）

- `nix.binary` が存在しないと、flake.lock の読み込み自体が失敗し（入力の store path の解決に `nix` を使う）、`window/showMessage` type 1 "Failed to load flake workspace: Failed to resolve flake inputs from lock file: Failed to spawn …" が 0.104s に来る。progress は出ない
- store path のない入力で `showMessageRequest` に Fetch と答えると、`nil/flakeArchiveProgress` の begin → end（0.3 秒）の後に type 1 "Failed to archiving flake: `nix flake archive …` failed with exit status: 1 …"。オプションの評価は走らない

### 再読み込み（走行 8）

`ready` の後に `flake.lock` を書き換えて `didChangeWatchedFiles` Changed を送ると、走っていたオプションの評価の future が中断されて **end が即座に出て**、100 ms の debounce の後に begin、0.75 秒後に end。`ready` → `indexing` → `ready` は 7.1 の 3 のとおり観測できる。中断の end から次の begin までの 115 ms は古い flake の情報のままで、その間の問い合わせは古い lock に対しては完全である。`flake.nix` の `didOpen` / `didChange` でも同じ再読み込みが走る（`server.rs:275-300, 326-443`）。

### 単一文書に閉じる `references`（走行 6）

`pkgs` の束縛の `references` は `flake.nix` の 1 箇所だけで、`module.nix` を開いていても変わらない（`references.rs:12-60`。名前解決は文書ごと）。

### 変更の取り込み

`didChange` は VFS を主ループで更新してから snapshot を取るので、後続の要求は新しい木を見る。7.3 は横断を要し、上のとおり構成できない。

## 写像（設計）と未決の点

- **識別**: `serverInfo.name` "nil"。版は `serverInfo.version`
- **readiness**: `initializing` から、token が `nil/loadNixosOptionsProgress`、`nil/loadInputFlakeProgress`、`nil/flakeArchiveProgress` のいずれかの begin で `indexing`、未完了の token が 0 になった end で `ready`。他の token は読まない。begin が来ない workspace（flake がない、`flake.lock` がない、`nixpkgs` の入力がない、入力の store path がない）では `initializing` のまま。**ユーザーの決定が要る**: (a) clangd と同じく `initializing` のまま（7.0 の要求は保留され続ける。flake のない Nix のディレクトリでは文書内で完結する `references` も返らない）か、(b) 最初の begin まで `unknown`（保留されず、flake.lock の読み込みの前の約 100 ms は入力への `definition` が空で通る）か。推奨は (a): 作者の Nix の workspace はすべて `nixpkgs` を入力に持つ flake で、時間で判定しない以上、信号の来ない窓を `unknown` で通すより保留の方が仕様の趣旨に合う
- **先読み**: しない。`flake.lock` の `didChangeWatchedFiles` は nil 自身が読み（登録する）、再読み込みの begin → end で状態が動く
- **health**: `window/showMessage` の type 1 で `error`、type 2 で `warning`。上の 3 つの token の begin で `ok`（flake.lock の読み込みが済み入力が揃った、または取得を始めたことを示す）。type 3 以下は読まない
- **クライアントの宣言との関係**: flake.lock の読み込みの前の窓（約 100 ms。信号なし）に届いた入力への `definition` は空で返る。`experimental/serverState` を宣言しないクライアントには lsp-det が `initializing` の間その要求を保留し（9 章の代行）、最初の begin と end の後に転送するので完全な答えになる（実サーバーテストで確認）。宣言するクライアントには 9 章のとおり転送し、待つのはクライアントの責務。Claude Code は宣言しないので前者
- **coverage / freshness**: 決定 E の答え（(b)）により、通した版（2026-07-23）に `coverage: {scope: "document", incomplete: {}}` を宣言する。理由は nixd と同じ。`freshness` は宣言しない: 7.3 は横断を要し、文書に閉じるサーバーでは構成できない
- **実サーバーの結合テスト**: 7.1（識別、`initializing` から begin で `indexing`、end で `ready`、begin の後に送った `inputs` の `nixpkgs` への `definition` が `/nix/store/…-source/flake.nix` を指して返る、宣言しないクライアントが `didOpen` の直後に送った同じ `definition` が保留され `ready` の後に完全に返る、`flake.lock` の `didChangeWatchedFiles` で `indexing` → `ready`、`nix.binary` を壊した設定で health `error`）と 7.2 の 1（`ready` の後の `references` が文書内の結果に一致すること。`includeDeclaration: true` でも `pkgs` 束縛自身の宣言位置は返らず、実際の使用箇所だけが返る）。fixture の `flake.lock` は lsp-det 自身の `flake.lock` の `nixpkgs` の node を写す（store path が開発環境にある）。`nix` が PATH に要る
- **Claude Code との関係（M27 で確かめる）**: `window/showMessageRequest` を CC が答えるか。答えなければ store path のない入力で読み込みが止まる（lsp-det は代行しない。観測者の注入の範囲外）
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: (1) flake.lock の読み込み（`load_flake_info`）にも `$/progress` を出す。今は入力への `definition` が空で通る窓に信号がない、(2) flake がない workspace でその旨を `window/logMessage` か `$/progress` で示す。今は信号の不在が「読み込み中」と区別できない、(3) 中断した評価の end を出す前に、次の begin を出す（今は end → 115 ms → begin で、その間だけ `ready` に見える）

## コーパスへの反映

`readiness-vocabulary-corpus.md` に nil の行を足す（Serena の一覧にない）。信号は `$/progress`（固定 token 3 つ）、flake.lock の読み込みには信号なし、health は `window/showMessage` type 1 / 2、`references` は単一文書。
