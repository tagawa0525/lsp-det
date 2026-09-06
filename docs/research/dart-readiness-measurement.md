# Dart analysis server の readiness の実測（M21）

ADR 0020 決定 C の M21。[dart-and-sorbet-readiness-vocabulary.md](dart-and-sorbet-readiness-vocabulary.md) は 2 ファイルの被験体で語彙（`$/progress` token `ANALYZING`）を確かめたが、規模が小さく保証は測れていなかった。401 ファイルの被験体で測り、**Dart は要求を自分で解析の完了まで待たせる**と分かった。起動直後の `references` も、編集やファイル作成の直後の `references` も、解析が終わってから完全な結果で答え、空応答も古い応答も返さない。変更の瞬間に処理中だった要求は `-32801 ContentModified` のエラーで断る。写像は `ANALYZING` の begin で `indexing`、end で `ready` で、先読みは要らない。`serverInfo` に版があるので、通した版に保証を宣言する。

## 方法

- nixpkgs の Dart SDK 3.13.0（`flake.nix` の `servers`）。`dart language-server`。2026-09-06
- 被験体: `pubspec.yaml` と `lib/a.dart`（`void target() {}`）、`lib/f0.dart`〜`lib/f399.dart`（各 30 個の関数と `target()` を 1 回呼ぶ関数。`import 'a.dart'`）。`dart pub get` はしない（相対 import だけなので要らない）
- 道具: scratchpad の `lsp_probe.py`。クライアントは `window.workDoneProgress` を宣言する（lsp-det が注入するのと同じ）。`workspace/configuration` には `null` で答える
- 走行: (1) 起動して `a.dart` を開き `target` の `references` を 0.2 秒間隔で送る、(2) `ready` 後に開いている `f0.dart` に `didChange`（全文）で呼び出しを足す、(3) `ready` 後に `lib/g.dart` を作って `didChangeWatchedFiles` Created を送る、(4) 同じ作成を通知しない、(5) (2) と (3) を 0.05 秒間隔で、(6) 解析するものがないワークスペース（`pubspec.yaml` だけ、または空のディレクトリ）
- 裏付けに SDK の `pkg/analysis_server/tool/lsp_spec/README.md` と `lib/src/lsp/handlers/handlers.dart`、`handler_references.dart` を読んだ

## 結果

### 語彙

| 信号                                                                         | 内容                                                                                                                                                                                         |
| ---------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `InitializeResult.serverInfo`                                                | `{"name": "Dart SDK LSP Analysis Server", "version": "3.13.0"}`。版が語彙に現れる                                                                                                            |
| `window/workDoneProgress/create`（token `"ANALYZING"`）                      | 解析の一巡ごとに毎回送られる（同じ token を作り直す）                                                                                                                                        |
| `$/progress`（token `"ANALYZING"`）                                          | begin（title "Analyzing…"）→ end。起動直後の解析、`didChange` の後、ディスク上の変更の後、そのたびに対で出る。解析するものがなくても `initialized` の直後に begin → end する（2 ms。走行 6） |
| `$/analyzerStatus`                                                           | `window.workDoneProgress` を宣言しないクライアントにだけ。README は非推奨（"may be removed in a future Dart SDK release"）としている。lsp-det は宣言を注入するので `$/progress` の経路になる |
| `workspace/configuration`                                                    | `initialized` の直後と `didOpen` のたびに section "dart" を要求する                                                                                                                          |
| `window/showMessage` type 1 "Unknown method workspace/didChangeWatchedFiles" | `workspace/didChangeWatchedFiles` を送ると返る。README に "unused, server does own watching" とあり、サーバーは自分でファイルを監視し、この通知を扱わない                                    |
| `-32801 ContentModified` "Document was modified before operation completed"  | 要求の処理中にファイルが変わったときのエラー応答（`handlers.dart` の `fileModifiedError`）                                                                                                   |

health の信号はない。

### 起動（走行 1）

| 時刻（秒）   | 出来事                                                                         |
| ------------ | ------------------------------------------------------------------------------ |
| 0.016        | `initialize` 応答。`didOpen a.dart`                                            |
| 0.372        | `ANALYZING` begin                                                              |
| 0.02〜1.5    | `references` を 8 回送る。**応答はない**（サーバーが待たせる）                 |
| 1.606〜1.668 | 8 回分の応答がまとめて来る。**すべて 400 件**（`f0`〜`f399` の呼び出し。完全） |
| 1.665        | `ANALYZING` end                                                                |

サーバーは要求を解析の完了まで待たせる（`handler_references.dart` の `requireResolvedUnit` → `handlers.dart` の `server.getResolvedUnit`。解析待ちの unit は解析してから返る）。応答は end の少し前に出る（end はサーバー全体の解析状況が idle になったときの通知で、要求の答えは対象の解析が終われば出る）。7.1 の前提「`ready` の前の結果は不完全」は Dart では起きない。空応答も部分応答もない。2 回目以降の起動は解析のキャッシュが効いて 0.2〜0.7 秒で end になる。

### 変更の取り込み（走行 2〜5）

| 引き金（`ready` 後）                                              | 結果                                                                                                                                                                                                                                        |
| ----------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 開いている `f0.dart` に `didChange` で呼び出しを足す（走行 2、5） | 0.1 秒後に `ANALYZING` begin → end（数 ms）。`didChange` の後に送った `references` は end の直後に **401 件**（織り込み済み）。`didChange` の時点で処理中だった要求は古い 400 件で答える（LSP の順序どおり）                                |
| `g.dart` を作って `didChangeWatchedFiles` Created（走行 3、5）    | `window/showMessage` type 1 "Unknown method workspace/didChangeWatchedFiles"。作成の時点で処理中だった要求は **`-32801 ContentModified`**（作成の 16 ms 後）。0.13 秒後に begin → end。作成の後に送った要求は end まで待たされて **401 件** |
| 同じ作成を通知しない（走行 4）                                    | 同じ。サーバー自身の監視が拾う（処理中の要求は `-32801`、その後 begin → end、次の要求は 401 件）                                                                                                                                            |

`didChangeWatchedFiles` の通知はサーバーに読まれない（README のとおり）。取り込みはサーバー自身の監視で、上の走行では通知の後の問い合わせがどれも新しい答えだったが、監視は通知と因果関係がなく非同期なので、問い合わせが監視より先に届く窓がある。実サーバーの結合テスト 66 件を直列で回したとき、7.3 の 3（Created）で新しいファイルの参照が返らずに 1 度落ちた（単独では 5 回とも通る）。その窓に信号はない（`ANALYZING` の begin は監視が拾った後にしか来ない）。したがって 7.3 の 2〜4 は保証できない。通知はエラーの `showMessage` を生むだけで、取り込みには効かない。

`-32801` は LSP が「内容が変わったのでやり直せ」と定めたエラーで、無言の嘘ではない。lsp-det はそのまま転送する。

## 写像（設計）

- **識別**: `serverInfo.name` "Dart SDK LSP Analysis Server"。版は `serverInfo.version`
- **readiness**: `initializing` から、`ANALYZING` の begin で `indexing`、end で `ready`。以後の begin（`didChange`、ディスク上の変更、`didOpen`）で `indexing`、end で `ready`。rust-analyzer の `quiescent` と同じ形。解析するものがなくても対が来るので、`initializing` に留まることはない
- **先読み**: しない。サーバーが要求を待たせるので、`didChange` の直後に `ready` のまま転送しても古い答えは返らない（ADR 0014 追補の決定 D の条件は満たすが、要らない）
- **health**: 信号がなく `unknown`
- **coverage / freshness**: 7.1、7.2、7.3 の 1 を通した版（3.13.0）に `coverage: {scope: "workspace", incomplete: {}}` と `freshness: {fileChanges: []}` を宣言する。`didChange` はメッセージの順に適用されてから後続の要求が処理されるので成り立つ。ディスク上の変更（7.3 の 2〜4）は上のとおり窓があり宣言しない。実サーバーの結合テストは 7.1、7.2、7.3 の 1 を当てる
- **`didChangeWatchedFiles` の代行（ADR 0015）との関係**: lsp-det が代行で送る通知にも "Unknown method" の `showMessage`（type 1）が返る。取り込みには効かず、クライアントにエラーの通知が見えるだけ。代行を「上流が登録したときだけ」に絞るかは ADR 0015 の見直しで、本 M では変えない
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: `workspace/didChangeWatchedFiles` を黙って無視する（クライアントが送るのは LSP では登録の後だが、登録しないサーバーに送るクライアントは珍しくなく、type 1 の `showMessage` はエラーとして見える）

## コーパスへの反映

コーパスの「begin → `indexing`、end → `ready`。再解析も同じ対」はそのとおり。加えて「要求をサーバー自身が待たせるので、観測者の保留がなくても嘘は出ない」を記す。Sorbet の文書と同じく、サーバーが自分で待たせる型。
