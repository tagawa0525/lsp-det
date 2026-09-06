# Sorbet の readiness の実測（M22）

ADR 0020 決定 C の M22。[dart-and-sorbet-readiness-vocabulary.md](dart-and-sorbet-readiness-vocabulary.md) は文書だけで `sorbet/showOperation` の語彙を確かめていた。601 ファイルと 3001 ファイルの被験体で測り、文書のとおり **Sorbet は Idle でないあいだ横断リクエストを待たせる**と分かった。起動直後、開いている文書の編集の直後、（watchman があれば）ディスク上の変更の直後のどれでも、`references` は操作の end まで待たされてから完全で新しい結果で答える。写像は要求に伴わない操作（Indexing、SlowPathBlocking、SlowPathNonBlocking 等）の start で `indexing`、未完了の操作がなくなった end で `ready`。`serverInfo` はなく版は語彙に現れないので、保証は宣言しない。ディスク上の変更は watchman が root を watch しているときだけ拾い、Sorbet 自身は `watch-project` を発行しない。

## 方法

- rubygems の `sorbet-static` 0.6.13485（x86_64-linux の prebuilt。`flake.nix` の `servers` の derivation）。`sorbet --lsp`（`--disable-watchman` を付けた走行と、watchman 2026.07.27.00 を PATH に置いた走行）。`initializationOptions` は `{"supportsOperationNotifications": true}`。2026-09-06
- 被験体: `sorbet/config`（`--dir` と `.`）、`lib/a.rb`（`module Lib; def self.target; end; end`）、`lib/f0.rb`〜（各 30 メソッドと `Lib.target` を 1 回呼ぶメソッドを持つクラス。全部 `# typed: true`）。601 ファイルと 3001 ファイルの 2 種
- 道具: scratchpad の `lsp_probe.py` と、要求の id ごとの順序を見る `sorbet_order.py`
- 走行: (1) 起動して `a.rb` を開き `target` の `references` を送り続ける（3001 ファイル）、(2) `ready` 後に開いている `f0.rb` に `didChange`（全文）でメソッドを足す、(3) 同じくメソッド本体に呼び出しを足すだけ（定義の形を変えない）、(4) `g.rb` を作って `didChangeWatchedFiles` Created を送る（`--disable-watchman`）、(5) 開いていない `f1.rb` をディスクで書き換えて Changed を送る（同）、(6) (4) を通知なしで、(7) (4) を watchman ありで（`.watchmanconfig` を置く）、(8) (7) の前に `watchman watch-project` を手で実行する
- 裏付けに Sorbet の文書（`website/docs/server-status.md`、`lsp.md`）とソース（`main/lsp/watchman/WatchmanProcess.cc`、`WatchmanSubscription.cc`）を読んだ

## 結果

### 語彙

| 信号                          | 内容                                                                                      |                                                                                                                                                                                                                                                                                                                                                          |
| ----------------------------- | ----------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `InitializeResult.serverInfo` | **なし**（`null`）。`capabilities.experimental` もない。版はどこにも現れない              |                                                                                                                                                                                                                                                                                                                                                          |
| `sorbet/showOperation`        | `{operationName, description, status: "start" \                                           | "end"}`。`initializationOptions.supportsOperationNotifications: true` のときだけ。観測した `operationName` は `SlowPathBlocking`（"Typechecking..."）、`Indexing`（"Indexing files..."）、`SlowPathNonBlocking`（"Typechecking in background"）、`References`（"Finding all references..."）。操作は入れ子になる（`SlowPathBlocking` の中に `Indexing`） |
| `client/registerCapability`   | **なし**。`workspace/didChangeWatchedFiles` は登録されず、送っても読まれない（走行 4、5） |                                                                                                                                                                                                                                                                                                                                                          |
| stderr "Pausing" / "Resuming" | 起動時の型検査の前後。プロトコルには出ない                                                |                                                                                                                                                                                                                                                                                                                                                          |

health の信号はない。文書（`server-status.md`）の表は Idle だけが "Responsive to IDE features" で、"Find All References (and features powered by finding all references, like Rename Symbol) are only available from the Idle state" と明記している。

### 起動（走行 1、3001 ファイル）

| 時刻（秒） | 出来事                                                                                    |
| ---------- | ----------------------------------------------------------------------------------------- |
| 0.016      | `initialized`、`didOpen a.rb`。直後に `SlowPathBlocking` start、`Indexing` start          |
| 0.02〜0.17 | `references` を送り続ける。**応答はない**（サーバーが待たせる）                           |
| 0.115      | `Indexing` end                                                                            |
| 0.175      | `SlowPathBlocking` end                                                                    |
| 0.177      | `References` start → end（0.322）。**最初の応答は 3001 件**（`f0`〜`f2999` と宣言。完全） |

`SlowPathBlocking` の end の前に答えられる要求はなく、空応答も部分応答もない。7.1 の前提「`ready` の前の結果は不完全」は Sorbet では起きない。

### 変更の取り込み（走行 2〜8）

| 引き金（`ready` 後）                                                            | 結果                                                                                                                                                                                                                                                                                                                                                              |
| ------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 開いている `f0.rb` に `didChange` でメソッドを足す（走行 2）                    | 1 ms 後に `SlowPathBlocking` start（中に `Indexing`）→ end → `SlowPathNonBlocking` start → end（3001 ファイルで 0.12 秒）。`didChange` の直後、`SlowPathBlocking` の間、`SlowPathNonBlocking` の間に送った要求はどれも **`SlowPathNonBlocking` の end まで待たされ、3002 件**（織り込み済み）。`didChange` の前に送った要求は古い件数で答える（LSP の順序どおり） |
| 同、メソッド本体に呼び出しを足すだけ（走行 3）                                  | 同じ（slow path になる。`FastPath` の操作はこの被験体では観測できなかった）                                                                                                                                                                                                                                                                                       |
| `g.rb` を作って `didChangeWatchedFiles` Created（走行 4、`--disable-watchman`） | 変わらない（601 件のまま 1 秒以上）。登録していない通知は読まれない                                                                                                                                                                                                                                                                                               |
| 開いていない `f1.rb` をディスクで書き換えて Changed（走行 5、同）               | 同じく変わらない                                                                                                                                                                                                                                                                                                                                                  |
| 同じ作成を通知しない（走行 6、同）                                              | 変わらない                                                                                                                                                                                                                                                                                                                                                        |
| watchman を PATH に置き `.watchmanconfig` を置く（走行 7）                      | 変わらない。stderr に watchman の `RootResolveError: ... is not watched`                                                                                                                                                                                                                                                                                          |
| 同、先に `watchman watch-project <root>` を実行しておく（走行 8）               | 作成の 20 ms 後に `SlowPathBlocking` → `SlowPathNonBlocking`、次の要求は end まで待たされて **602 件**                                                                                                                                                                                                                                                            |

ソース: `WatchmanSubscription.cc` は `["subscribe", root, name, {...}]` だけを送り、`watch-project` は発行しない。watchman の `subscribe` は既に watch された root にしか効かないので、他の何か（エディタの拡張など）が `watch-project` していない環境では、`lsp.md` の言う「`.git` か `.watchmanconfig` があればよい」は成り立たない。`didChangeWatchedFiles` を読まないので、クライアント（と lsp-det の代行）が通知しても効かない。

## 写像（設計）

- **識別**: `serverInfo` がない。名乗りは `sorbet/showOperation` の通知そのもの（メソッド名で識別する。`identity_from_notification` の経路）。版は取れない
- **有効化**: `initializationOptions.supportsOperationNotifications: true` を、lsp-det が起動したコマンドの basename が `sorbet` か `srb` のときだけ注入する（ADR 0020 決定 D）
- **readiness**: `initializing` から、要求に伴わない操作（`Indexing`、`SlowPathBlocking`、`SlowPathNonBlocking`、`FastPath`。文書の表で Idle でない状態）の start で `indexing`、未完了の操作がなくなった end で `ready`。操作は入れ子になるので数える。要求に伴う操作（`References`、`SymbolSearch`、`Rename`、`MoveMethod`）は状態にしない（横断リクエストそのものの処理）
- **先読み**: しない。サーバーが要求を待たせるので、`didChange` の直後に `ready` のまま転送しても古い答えは返らない。start は `didChange` の 1 ms 後に来る
- **health**: 信号がなく `unknown`
- **coverage / freshness**: 宣言しない（`{}`）。7.1〜7.3 の 1 は成り立つが、版が語彙に現れず、通した版に限って宣言する（仕様 8.2 の 5）ことができない。7.3 の 2〜4 は watchman が root を watch しているときだけ成り立つ
- **実サーバーの結合テスト**: `--disable-watchman` で起動し、7.3 は 1（`didChange`）だけを当てる。2〜4 は watchman の daemon と `watch-project` を要し、テストの環境に置かない
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: (a) `InitializeResult.serverInfo` を返す。(b) `subscribe` の前に `watch-project` を発行する（または文書に「root が watch されていること」を書く）。(c) `workspace/didChangeWatchedFiles` を登録して読む（watchman がない環境の取り込み）

## コーパスへの反映

「`Indexing` / `SlowPathBlocking` / `SlowPathNonBlocking` の start で `indexing`、未完了の操作がなくなった end で `ready`」はそのとおり。加えて Dart と同じく「要求をサーバー自身が待たせるので、観測者の保留がなくても嘘は出ない」。
