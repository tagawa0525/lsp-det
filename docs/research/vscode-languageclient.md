# vscode-languageclient 読解調査 — 保留（遅い応答）への耐性

調査対象: `reference/vscode-languageserver-node`（commit `3e4039c`, 2026-08-27 時点の main）。
`vscode-languageclient` 10.1.0 / `vscode-jsonrpc` 9.0.1。
以下のパス:行番号はすべてこの clone からの相対パス。

## 要約

- **リクエストにタイムアウトは存在しない**。jsonrpc 層の `sendRequest` は応答が来るまで無期限に待つ。唯一の時限処理は shutdown 時の 2 秒と、受信バイト列が途中で止まったときの情報イベント（10 秒、打ち切りなし）のみ。
- **キャンセルの起点はほぼ VS Code 本体**。エディタが provider に渡す `CancellationToken` が cancel されると jsonrpc が `$/cancelRequest` を送る。クライアント自身が能動的に cancel するのは pull 診断の置き換え等ごく一部。`RequestCancelled` (-32800) を受けたクライアントは、自分の token が cancel 済みなら黙って既定値（≒空結果）、そうでなければ `CancellationError` を throw する（エラー UI は出ない）。
- **送信側にキューはない**。リクエストは発行された順に即書き込まれ、in-flight で並行する。直列化されるのは「リクエスト送信前に保留中の `didOpen`/full-sync `didChange` を先に流す」順序保証のみ。
- **initialize 完了前のリクエストは Promise で待たされる**（バッファでもエラーでもなく、`start()` 完了を await）。そもそもエディタ由来のリクエストは initialize 応答後に provider が登録されるまで発生しない。
- **progress はすべて表示専用**。`workDoneProgress` をリクエストの発行制御・ゲートに使う箇所は皆無。
- **range の補正・検証は一切ない**。`DocumentSymbol` 等はサーバーの値をそのまま `vscode.Range` に詰め替えるだけ。
- **結論: lsp-det による数十秒の保留はプロトコル上安全**。ただし `$/cancelRequest` の転送と即時の `RequestCancelled` 応答、および initialize・shutdown を保留対象から外すことが必須条件。

## 1. リクエストのタイムアウト: 存在しない

jsonrpc 層の `sendRequest` は、送信した Promise を `responsePromises` マップに登録して返すだけで、タイマーを一切仕掛けない。

- `jsonrpc/src/common/connection.ts:1442-1469` — Promise を生成し `responsePromises.set(id, ...)` して `messageWriter.write()`。resolve/reject の経路はここには書き込み失敗しかない
- `jsonrpc/src/common/connection.ts:903-939` — `handleResponse` が id で対応する Promise を引いて resolve/reject する。応答が来ない限り Promise は永遠に pending
- `jsonrpc/src/common/connection.ts:1539-1549` — 唯一の強制的な決着は `dispose()` で、全 pending を `ErrorCodes.PendingResponseRejected` で reject する

`ResponsePromise` が持つ `timerStart`（`connection.ts:587`, `1454`）はタイムアウト用ではなく、トレースログの所要時間表示にのみ使われる（`connection.ts:1158`）。

jsonrpc 共通層に存在するタイマーは 2 つだけで、どちらもリクエスト打ち切りではない。

| 箇所 | 値 | 役割 |
| --- | --- | --- |
| `jsonrpc/src/common/messageReader.ts:187,270-282` | 10000ms（設定可） | メッセージ本文が途中で止まったとき `partialMessage` 情報イベントを発火するだけ。読み取りは継続 |
| `jsonrpc/src/common/cancellation.ts:50` | 0ms | token コールバックの非同期スケジューリング |

クライアント層で唯一のタイムアウトは停止時のもの: `client/src/common/client.ts:1584-1640` — `stop(timeout = 2000)` が `shutdown` → `exit` と 2 秒タイマーを `Promise.race` し、負けると「Stopping the server timed out」でエラー扱いにする。通常のリクエストには適用されない。

**確認結果: 応答が来るまで無期限に待つ。ユーザー設定でリクエストタイムアウトを与える手段もない。**

## 2. キャンセル

### 送信契機

1. **エディタ（VS Code 本体）の token cancel**。各 feature は provider に渡された token をそのまま `sendRequest` へ渡し（例: `client/src/common/reference.ts:52`, `workspaceSymbol.ts:67`, `documentSymbol.ts:98`）、jsonrpc が `token.onCancellationRequested` で `$/cancelRequest` 通知を送る（`jsonrpc/src/common/connection.ts:1419-1427`, 既定戦略 `connection.ts:641`, 送信実体 `connection.ts:447-452` の `CancellationSenderStrategy.Message`）。「いつ cancel されるか」は VS Code 本体の裁量（再入力で古い補完が不要になった等）であり、クライアント側に独自のトリガーはない
2. **同一対象への新リクエストによる置き換え（pull 診断のみ）**。`client/src/common/diagnostic.ts:436-445` — 同一文書の pull 実行中に新しい pull 要求が来ると `tokenSource.cancel()` して reschedule。ワークスペース診断も refresh 時に前回を cancel する（`diagnostic.ts:516-521`, `668-674`）
3. **送信前に既に cancel 済みの token** は送信せずローカルで `RequestCancelled` reject（`client/src/common/client.ts:946-947`）
4. **dispose/stop は個別 cancel しない**。接続 `dispose()` が全 pending を一括 reject するだけ（`connection.ts:1545-1548`）

なお progress UI のキャンセルボタンは `$/cancelRequest` ではなく `window/workDoneProgress/cancel` 通知を送る（`client/src/common/progressPart.ts:70-72`）。

### `RequestCancelled` (-32800) 受信時の扱い

全 feature は失敗を `handleFailedRequest` に集約する（`client/src/common/client.ts:2298-2326`）。

| 受信エラー | 条件 | クライアントの挙動 |
| --- | --- | --- |
| `RequestCancelled` / `ServerCancelled` | 自分の token が cancel 済み | 既定値（言語機能では `null` = 結果なし）を静かに返す (`client.ts:2306-2308`) |
| `RequestCancelled` / `ServerCancelled` | token は生きている | `CancellationError`（data 付きなら `LSPCancellationError`）を throw (`client.ts:2309-2315`)。VS Code はこれを「キャンセル」として扱いエラー表示しない |
| `ContentModified` (-32801) | semanticTokens 系・resolve 系 (`client.ts:2284-2296`) | `CancellationError` を throw |
| `ContentModified` | それ以外（references 等） | 既定値 `null` を静かに返す (`client.ts:2316-2321`) |
| `PendingResponseRejected` / `ConnectionInactive` | 接続消失 | 既定値を返す (`client.ts:2303-2305`) |
| その他 | — | ログ出力して rethrow (`client.ts:2324-2325`) |

pull 診断だけは `ServerCancelled` の `data.retriggerRequest`（既定 true）を見て再試行を判断する（`client/src/common/diagnostic.ts:404-408`）。

## 3. キューイング / 直列化: 送信は投げっぱなし並行

- **送信側にキューはない**。`client.sendRequest` → `connection.sendRequest` は即 `messageWriter.write()` する（`connection.ts:1455-1457`）。書き込みの直列化は transport 排他の `Semaphore(1)` のみ（`jsonrpc/src/common/messageWriter.ts:122-136`）。複数リクエストが同時に in-flight になり、応答は id で相関されるため**順不同の応答を問題なく処理できる**
- **順序保証は通知が先行することのみ**。`client.sendRequest` は毎回、保留中の `didOpen` と full-sync の `didChange` を先に flush してからリクエストを書く（`client/src/common/client.ts:920-930`, `textSynchronization.ts:171-183`）
- **受信側には順序キューがある**。受信メッセージは `messageQueue` に積まれ `setImmediate` で 1 件ずつディスパッチされる（`connection.ts:700-759`）。`ConnectionOptions.maxParallelism`（`connection.ts:491-495`, 既定 -1 = 無制限）は主に受信リクエスト処理の並列度制御で、言語クライアントは既定のまま使う
- 未ディスパッチの受信リクエストに `$/cancelRequest` が来た場合、`connectionStrategy.cancelUndispatched` でキューから外せる仕組みがある（`connection.ts:773-789`、既定は何もしない）
- 例外的にクライアント側で自前の直列化を持つのは pull 診断だけ（`diagnostic.ts:384-461`、文書ごとに 1 in-flight + reschedule）

## 4. サーバー起動〜initialize 完了前のリクエスト

- 拡張コードが `client.sendRequest` を呼ぶと、まず `await this.$start()` する（`client/src/common/client.ts:921`, `1817-1827`）。`$start` → `start()` は `_onStart` Promise を返し、この Promise は接続確立 → `initialize` 要求 → `initialized` 通知送出まで完了しないと resolve されない（`client.ts:1302-1306`, `1418-1420`, `1479-1545`）。つまり **initialize 完了まで Promise 上で待たされる（エラーにならない）**。start 失敗時のみ reject
- 停止中・失敗後の状態では `ConnectionInactive` で即 reject（`client.ts:916-918`）
- エディタ由来のリクエストはそもそも発生しない: `languages.register*Provider` は initialize 応答受領後の `initializeFeatures`（`client.ts:1543`）→ 各 feature の `registerLanguageProvider` で初めて登録される
- `initialize` リクエスト自体にもタイムアウトはない（`doInitialize` は素の await、`client.ts:1481`）。失敗時は `initializationFailedHandler` か retry ダイアログ（`client.ts:1546-1568`）
- 接続断時は `DefaultErrorHandler` が再起動を判断する: 3 分以内に `maxRestartCount`（既定 4）+1 回 close したら再起動を止める（`client.ts:448-477`, `1170-1175`, `1843-1876`）

## 5. progress / workDoneProgress: 表示専用

- `window/workDoneProgress/create` を受けると `ProgressPart` を作り、`Window.withProgress`（ステータスバー表示）に流すだけ（`client/src/common/progress.ts:40-45`, `progressPart.ts:39-79`）
- progress の begin/report/end を**リクエストの発行可否・保留・再試行の判断に使う箇所はゼロ**。ゲートとしての利用はない
- 双方向性は「cancellable な progress をユーザーが止めたら `window/workDoneProgress/cancel` を送る」のみ（`progressPart.ts:70-72`）
- partialResult を扱うのはワークスペース診断だけ（`diagnostic.ts:611-632`）

したがって「サーバーがインデックス中」であることをクライアントが認識してリクエストを控える仕組みは存在しない。readiness 管理はプロキシ側（lsp-det）が担っても、クライアントの挙動と競合しない。

## 6. range の補正・検証: なし

- `protocol2CodeConverter.asDocumentSymbol` はサーバーの `range`/`selectionRange` を `vscode.Range` にそのまま詰め替えるだけで、包含関係の検証・clamp・行数補正は一切行わない（`client/src/common/protocolConverter.ts:968-985`, `asRange` は `client/src/common/protocolConverter.ts:406-419` の単純変換）
- 補完・codeAction 等の他の変換も同様に素通しで、正規化は `deprecated` → `tags` の吸収（`protocolConverter.ts:987-999`）程度
- 注意（本 clone の範囲外、記憶ベース）: VS Code 本体の `vscode.DocumentSymbol` コンストラクタは `range ⊇ selectionRange` を検証して違反時に throw するため、違反シンボルはクライアントではなくエディタ側で落ちる。**languageclient 層に補正がない**ことは本調査で確定

vision.md の懸念どおり、range 契約の緩さはクライアント側では吸収されていない。lsp-det が補正を担う設計と整合する。

## 7. 結論: lsp-det の readiness ゲートは安全か

**安全である。** 根拠と条件:

1. **無期限待機**: クライアント / jsonrpc にリクエストタイムアウトがないため、references 等を数十秒保留しても打ち切り・エラー・再送は発生しない（§1）。保留中の応答が順不同で返っても id 相関で正しく処理される（§3）
2. **保留中のキャンセルに追従すること（必須）**: 保留中に VS Code が token を cancel すると `$/cancelRequest` が飛んでくる。lsp-det はゲート内の保留リクエストに対してこれを受理し、**即座に `RequestCancelled` (-32800) を返す**べき。クライアントは token cancel 済みならこれを黙って空結果扱いにするので UI ノイズは出ない（§2）。放置しても壊れはしないが、pending が積み上がる
3. **通知・他リクエストを塞がないこと（必須）**: クライアントはリクエスト発行時に `didOpen`/`didChange` の flush を await する（§3）。lsp-det が transport ごと詰まらせる実装（保留中に読み取り停止等）にすると全機能が連鎖停止する。保留は「該当リクエストの応答のみ遅らせる」透過実装であること
4. **ゲート対象から外すべきメッセージ**: `initialize`（start 全体が待たされ、初期化失敗ダイアログ経路に入り得る。§4）、`shutdown`/`exit`（2 秒でタイムアウト扱い。§1）、`willSaveWaitUntil` 系（クライアント層は無期限だが VS Code 本体の保存参加者タイムアウトで edits が捨てられる。`textSynchronization.ts:574-585` にタイムアウトはない）
5. **接続を切らないこと**: サーバー準備中に接続を close すると DefaultErrorHandler の再起動カウント（3 分に 5 回で打ち切り）を消費する（§4）
6. **代替案との比較**: 保留の代わりに `ContentModified` を即答すると references 等では「静かに空結果」となり、エージェントの誤認（vision.md の問題そのもの）を招く。`ServerCancelled` 即答は `CancellationError` になり結果は出ないが再試行はされない。**ready まで応答を遅らせる方式が、クライアント挙動に照らして最も正しい**
7. **残る UX 上の注意**: 保留中もユーザーにはスピナーが見えるだけで理由が伝わらない。progress は表示専用（§5）なので、lsp-det が自前で `window/workDoneProgress/create` + progress を合成してインデックス中であることを見せるのは、クライアント挙動と干渉せず有効
