# サーバー状態プロトコル (Server State)

[English](server-state.md)

状態: 草案。規範（normative）は英語版 [server-state.md](server-state.md) であり、他文書の記述と食い違う場合は英語版を正とする。本文書はその日本語版で、同じコミットで追従させる（ADR 0017）。LSP 本体に取り込まれるまでは `experimental/` 配下の拡張として実装する（4.3）。

## 1. 目的

LSP には、サーバーの状態（要求に完全に答えられるか・どの編集まで織り込んでいるか・壊れていないか）をクライアントが機械的に知る手段がない。その結果、プロトコル上は正当だが真実ではない応答 — インデックス未完了の空配列、壊れたサーバーの成功風の応答、編集を織り込まない stale な結果 — をクライアントが信じてしまう。本プロトコルはサーバー状態を機械可読な語彙で表し、この「無言の嘘」を消す。

本プロトコルは情報を渡すだけであり、クライアントの挙動を強制しない。状態を見て待つか、部分的な結果を承知で進むかは、クライアントが判断する。9 章に推奨する挙動を非規範として示す。

## 2. 文書の構成と読者

本文書は 2 層からなる。

- **サーバーの義務**（3〜7 章）: 言語サーバー本体が実装する部分。LSP 本体への提案の対象はこの層である
- **観測者が合成する値**（8 章）: サーバーの外からプロセスや接続を見る主体（プロキシ、クライアントライブラリ、エディタ本体）が、サーバーの語彙を補うときの規則。8 章を削っても 3〜7 章は成立する

9 章はクライアントの推奨挙動（非規範）とその準拠要件、10 章は既存実装との対応である。

実装者は 3 者である。

- **サーバー**: 言語サーバー本体。3〜7 章に従う
- **クライアント**: エディタ、エージェント、ブリッジ。値を読み、待つか進むかを判断する
- **観測者**: サーバーとクライアントの間に立つ中継層（プロキシ等）や、接続を持つクライアントライブラリ。サーバーに代わって本プロトコルを提供でき、8 章の値を合成できる

## 3. 型定義

```typescript
interface ServerState {
  /**
   * "ok":      完全に機能している
   * "warning": 部分的に機能している（依存欠落等。結果が不完全になりうる）
   * "error":   機能していない（結果は信頼できない）
   */
  health: "ok" | "warning" | "error";

  /**
   * "initializing": initialize 直後。まだ何も答えられない
   * "indexing":     一部の要求に答えられるが、結果が不完全になりうる
   * "ready":        インデックスが完了している
   */
  readiness: "initializing" | "indexing" | "ready";

  /** 人間向けの補足。機械判定に使ってはならない */
  message?: string;
}
```

`health` と `readiness` は独立の 2 軸である。推奨解釈（非規範）: `health` が `error` のとき、`readiness` を判断材料に使うべきではない。保証の適用条件は 6 章が規範である。

`readiness` に失敗を表す値はない。インデックスの失敗は `health` で表す（6 章 5 項）。

前方互換のため、クライアントは次の 2 つを守らなければならない。

- `ServerState` に未知のフィールドが含まれてもエラーにせず無視する
- `health` または `readiness` に本章が定義しない値が来たら、その軸からは何も読み取れないものとして扱う（8 章の `unknown` はこの規則に乗る）

予約済みの拡張候補: `phases`（診断等のフェーズ別完了状態）、鮮度トークン（織り込み済み変更の識別子）。

## 4. メソッド

### 4.1 状態の問い合わせ

```text
Request:  experimental/serverState
Params:   なし
Response: ServerState
```

問い合わせを受けた時点の状態を応答する。応答を遅らせて状態の変化を待ってはならない。応答が返らない場合の解釈（サーバーが停止しているとみなす等）は本プロトコルでは定めず、クライアントに委ねる。

### 4.2 状態変化の通知

```text
Notification: experimental/serverStateChanged
Params:       ServerState
```

`health` または `readiness` が変わるたびに送る。同一状態の重複送信は許容されるが推奨しない。`indexing` 中の細かい進捗は送らない（進捗表示は既存の `$/progress` の役割）。

### 4.3 命名

メソッド名は LSP 本体に取り込まれるまで `experimental/` プレフィックスを用いる（rust-analyzer の `experimental/serverStatus` と同じ慣行）。取り込み時に `workspace/serverState` / `workspace/serverStateChanged` へ改名する。

## 5. Capability と保証

```typescript
// サーバー・観測者 → クライアント (InitializeResult)
interface ServerCapabilities {
  experimental?: {
    /**
     * キーがあれば本プロトコルを話す。{} は状態の通知だけを約束する。
     * coverage / freshness は、readiness が "ready" かつ health が "error"
     * でないときの応答について、それぞれの保証を足す。キーがなければ
     * その保証はしない。
     */
    serverStateProvider?: {
      coverage?: {
        /**
         * 7.0 のメソッドの応答が基づく範囲。
         * "workspace":     ワークスペース全体のインデックス。インデックスの
         *                  進行によって後から同じ問い合わせの結果が増えることがない
         * "openDocuments": クライアントが開いている文書だけ。開いていない
         *                  ファイルの内容は応答に現れない
         */
        scope: "workspace" | "openDocuments";
        /**
         * 件数の上限で結果を切るメソッドと、その上限。上限に達した応答は
         * 完全でないことがある。LSP のメソッド名をキーにする。
         */
        incomplete: { [method: string]: number };
      };
      freshness?: {
        /**
         * 織り込んでいる workspace/didChangeWatchedFiles の変更の種類
         * （FileChangeType の名前）。textDocument/didChange は常に織り込む。
         */
        fileChanges: ("Created" | "Changed" | "Deleted")[];
      };
    };
  };
}

// クライアント → サーバー (InitializeParams)
interface ClientCapabilities {
  experimental?: {
    serverState?: boolean;
  };
}
```

### 5.1 保証

`serverStateProvider: {}`（保証のキーなし）は、状態の通知そのものだけを約束する。`indexing` 中の応答が不完全でありうるという警告として、それだけでも価値を持つ。

`coverage` と `freshness` は LSP の他の capability の options（`renameProvider: { prepareProvider }` 等）と同じくオプションのオブジェクトであり、`ready` が結果について何を意味するかを足す。値は真偽値ではなく、**あるべき姿からの欠けを名指しする**形をとる。ワークスペース全体のインデックスに基づき、打ち切らず、知らされた変更をすべて織り込むサーバーは `coverage: {scope: "workspace", incomplete: {}}`、`freshness: {fileChanges: ["Created", "Changed", "Deleted"]}` と宣言する。そこから欠けているものを、範囲（`scope`）、上限で切るメソッドと件数（`incomplete`）、織り込まない変更の種類（`fileChanges` からの欠落）として書く。クライアントは欠けを読んで、問い合わせを絞る・ファイルを開く・応答の件数を上限と比べる、といった判断ができる。

両者は**独立**で順序関係はない。現実のサーバーは 4 象限すべてに存在する:

|                   | freshness                                                    | freshness なし                              |
| ----------------- | ------------------------------------------------------------ | ------------------------------------------- |
| **coverage**      | スナップショット方式 + 全インデックス（rust-analyzer）       | 全インデックスだが非同期処理（tsserver 系） |
| **coverage なし** | リクエスト毎スナップショットだが全インデックスなし（clangd） | 保証なし                                    |

実装は自分が守れる保証だけを宣言する。守れない保証の宣言は本プロトコルへの違反である。`incomplete` の件数と `fileChanges` の種類も同じで、確かめていない値を書いてはならない。

### 5.2 クライアント宣言の意味

クライアントの `experimental.serverState: true` は、通知の購読要求であると同時に、**「状態を自分で解釈し、待つか進むかを自分で判断する」という意思表示**である。観測者はこの宣言を見て、非対応クライアント向けの代行動作（9 章）を解除してよい。宣言したクライアントが状態を無視して不完全な結果を得た場合、それはそのクライアントの責任である。

サーバー・観測者は、クライアントがこの宣言をした場合のみ `experimental/serverStateChanged` を送る。`experimental/serverState` リクエストは宣言の有無によらず応答する。

## 6. セマンティクス

1. **網羅**（`coverage` 宣言時）: `readiness` が `"ready"` かつ `health` が `"error"` でないとき、7.0 のメソッドへの応答は `scope` の範囲のインデックスに基づかなければならず、その範囲のインデックスの進行によって後から同じ問い合わせの結果が増えてはならない。`scope` が `"workspace"` ならワークスペース全体、`"openDocuments"` ならクライアントが開いている文書の範囲である。`incomplete` に挙げたメソッドは、応答の件数がその上限に達したとき結果が完全でないことがある。挙げていないメソッドの応答は上限で切られてはならない
2. **鮮度**（`freshness` 宣言時）: `readiness` が `"ready"` かつ `health` が `"error"` でないとき、それまでに受信した `textDocument/didChange` と、`workspace/didChangeWatchedFiles` のうち `fileChanges` に挙げた種類の変更は、すべて織り込み済みでなければならない。挙げていない種類の変更は、`"ready"` の後も織り込み途中でありうる（サーバーは取り込みを `readiness` で伝えられない、と自覚して宣言する）。約束するのは知らされた変更までであり、サーバーが自前のファイル監視で拾った変更は対象外である（観測者には見えず、検証できない）。この保証の実質はクロスファイルの鮮度（変更したファイル以外を起点とする問い合わせに、インデックスが変更を反映していること）である。単一ファイル内の変更→問い合わせの多くは、LSP の既存の処理順序保証（同一接続では後続リクエストが先行通知の後に処理される）だけで満たされる
3. **再インデックス**: ワークスペースの再解析（依存ファイル変更、ブランチ切り替え等）が始まったら、`readiness` を `"indexing"` に戻して通知しなければならない
4. **既存機構との関係**: `$/progress` は人間向けの進捗表示であり本プロトコルを代替しない。`ServerCancelled` エラーはポーリングを強いるため本プロトコルを代替しない（LSP issue #1367 の議論を参照）
5. **失敗の表現**: サーバーはインデックスの失敗（ワークスペースをロードできない等）を `readiness` ではなく `health`（`"error"` または `"warning"`）で表さなければならない。`readiness` は `"indexing"` に留めても `"ready"` にしてもよい。`health` が `"error"` のとき、1・2 項の保証は適用されない。推奨解釈（非規範。1 章のとおりクライアントの挙動は強制しない）: このとき `readiness` が `"ready"` になるのを待ち続けることは本プロトコルの意図に反する。待つ側は `health` を見て抜ける
6. **時間の不使用**: 本プロトコルの値は信号（サーバーの状態変化、観測者の観測）だけで決まる。経過時間を根拠に値を変えてはならない。「一定時間信号がないので `ready` とみなす」のような合成は、消すはずの無言の嘘を作る

## 7. 準拠要件（サーバー。テスト可能な形）

準拠テストの fixture は、インデックスに観測可能な時間を要する規模（ファイル数）でなければならない。小規模な fixture では `initialize` 応答の時点で既に `ready` に達し、7.1 の 3 の遷移が観測できない。これはテスト側の前提であり、サーバーが `initialize` 直後に正直に `ready` を返すことを禁じるものではない。

### 7.0 ワークスペース横断メソッド（`coverage` の対象、9 章で `ready` を待つ対象）

`textDocument/references`, `textDocument/definition`, `textDocument/typeDefinition`, `textDocument/declaration`, `textDocument/implementation`, `workspace/symbol`, `textDocument/prepareCallHierarchy`, `callHierarchy/incomingCalls`, `callHierarchy/outgoingCalls`, `textDocument/rename`, `textDocument/prepareRename`

件数の上限で結果を切るメソッド（`workspace/symbol` はエディタのピッカー向けのあいまい検索で、多くのサーバーが上限を持つ）は、上限を `coverage.incomplete` に宣言する。打ち切りを伝える語彙は LSP にないので、クライアントは応答の件数を上限と比べて判断する（10 章）。

### 7.1 保証なしの宣言

1. `initialize` 完了直後の `experimental/serverState` に応答する
2. クライアントが capability を宣言した場合のみ `experimental/serverStateChanged` が届く
3. 依存変更の後に `"ready"` → `"indexing"` → `"ready"` の遷移が観測できる
4. インデックスの失敗（ロードできないワークスペース等）が `health` の `"error"` または `"warning"` として報告される（6 章 5 項）

### 7.2 coverage 宣言時

1. `"ready"` になった後の `textDocument/references` が、事前計算した完全な結果と一致する（`scope` が `"openDocuments"` なら、開いている文書の範囲で）
2. `incomplete` に挙げたメソッドは、上限を超える数の一致がある問い合わせに対し、上限の件数を返す（上限が宣言どおりであること）。挙げていないメソッドは、上限を超える数の一致がある問い合わせに対し、すべてを返す

### 7.3 freshness 宣言時

1. ファイル A への `textDocument/didChange`（別ファイル B から参照されるシンボルの追加・削除）送信後、`readiness` が `"ready"` の状態で **B を起点に**発行した横断問い合わせ（`textDocument/references` 等）の応答が、A の変更を反映している
2. `fileChanges` に `"Changed"` があるとき: ファイル A をディスク上で変更して `workspace/didChangeWatchedFiles`（`Changed`）を送った後、`readiness` が `"ready"` の状態で **B を起点に**発行した横断問い合わせの応答が、A の変更を反映している。A は開かない（開くと `didOpen` の経路になり、ディスク上の変更を検証しない）
3. `fileChanges` に `"Created"` があるとき: 新しいファイル C を作って `workspace/didChangeWatchedFiles`（`Created`）を送った後、同様に C の内容を反映している。C は開かない
4. `fileChanges` に `"Deleted"` があるとき: ファイル C を消して `workspace/didChangeWatchedFiles`（`Deleted`）を送った後、同様に C からの参照が消えている

テストは必ずクロスファイル（変更したファイルとは別のファイルを起点とし、インデックス経由でしか到達できない結果）で行わなければならない。単一ファイルの変更→問い合わせは LSP の処理順序保証だけで通ってしまい、freshness を検証しない（6 章 2 項）。

## 8. 観測者が合成する値

サーバーの外から観測する主体は、サーバーが知っていることを知らない。本章は、観測者がサーバーに代わって本プロトコルを提供するときに、知らないことを知っているように見せないための規則である。本章はサーバーの義務（3〜7 章）に何も加えない。

### 8.1 `unknown`

観測者は `health` と `readiness` のそれぞれに値 `"unknown"` を用いてよい。

- `health: "unknown"`: health を観測する手段がない、またはまだ観測していない
- `readiness: "unknown"`: readiness を観測する手段がない

`unknown` のとき、その軸からは何も読み取れない。クライアントは応答が不完全でありうることを承知で進むか、自前で待つかを判断する（3 章の前方互換規則により、`unknown` を知らないクライアントも同じ扱いになる）。

サーバーは `unknown` を送出してはならない。サーバーは自分の状態を必ず知っている。

### 8.2 観測者の規則

1. **観測なしに `ok` や `ready` を名乗らない**。`ok` は「完全に機能している」、`ready` は「インデックスが完了している」の意味であり、観測できていなければ `unknown` を報告する
2. **最初の信号まで `health` は `unknown`**。サーバーの語彙（rust-analyzer の `experimental/serverStatus` 等）を写す観測者も、最初の信号が届くまでは `health` を知らない。`readiness` は「initialize 直後」に対応する `initializing` から始めてよい
3. **信号のないサーバーは両軸 `unknown`**。readiness を伝える語彙を持たないサーバー（clangd 等）を代行する観測者は、両軸 `unknown` を報告する。これは正直な準拠であり、`initializing` に留め置く（「まだ何も答えられない」という嘘）ことも `ready` を名乗ることもしてはならない
4. **6 章 3 項と 7.1 の 3 は `readiness` を追跡している観測者にのみ適用する**。両軸 `unknown` の観測者は再インデックスを観測できない
5. **保証の宣言は観測に基づく**。観測者がサーバーに代わって `coverage` / `freshness` を宣言できるのは、そのサーバーの当該版に 7.2 / 7.3 のうち宣言する内容に対応する要件を当てて通った場合に限る（`incomplete` の件数、`fileChanges` の各種類も同じ）。観測者はサーバーの内部を保証できず、テストを通した版の範囲を超えて宣言してはならない。サーバーは `InitializeResult.serverInfo` で名前と版を名乗るので、観測者はそれで範囲を判定できる
6. **サーバーが自ら宣言していれば観測者は加えない**。上流の `InitializeResult` に `serverStateProvider` があれば、中継層はサーバーの宣言をそのまま通し、`experimental/serverState` を上流へ転送し、自前の通知を送らない。同一接続に送信者の異なる 2 系統の状態が流れることを避けるためである。中継層の宣言でサーバーの宣言を隠すことは 5.1 の趣旨に反する
7. **プロセスの消失は本プロトコルの値ではない**。サーバープロセスの終了は接続の終了（stdio の EOF 等）として伝わり、既存のクライアントはそれで再起動を判断する。中継層は上流の消失を観測したら、未応答のリクエストにエラーを応答したうえで自分の接続も閉じる。「サーバーが死んだ」を表す値を本プロトコルに設けない理由は、死んだサーバーは通知を送れず、生き残った中継層が成功風の応答を返す状態は `health: "error"` で表せるからである

### 8.3 送出主体

| 値                                           | サーバー                                                              | 観測者                                                 |
| -------------------------------------------- | --------------------------------------------------------------------- | ------------------------------------------------------ |
| `health: ok / warning / error`               | 送出可                                                                | 送出可（上流の状態の転写または推定）                   |
| `health: unknown`                            | **送出してはならない**（サーバーは自分の状態を必ず知っている）        | 送出可（観測手段がないとき、または最初の信号が届く前） |
| `readiness: initializing / indexing / ready` | 送出可                                                                | 送出可                                                 |
| `readiness: unknown`                         | **送出してはならない**（サーバーは自分の readiness を必ず知っている） | 送出可（観測手段がないとき）                           |

### 8.4 準拠要件（観測者）

観測者は 7 章の要件を、8.2 の 4 の除外を適用したうえで満たす。加えて:

1. 信号のないサーバーを代行するとき、`initialize` 完了直後の `experimental/serverState` が両軸 `"unknown"` である
2. 上流が `serverStateProvider` を宣言しているとき、`initialize` 応答の宣言が上流のものと一致し、`experimental/serverState` の応答が上流の応答と一致する

## 9. クライアントの推奨挙動（非規範）

本章はクライアントの挙動を強制しない（1 章）。本プロトコルを使うクライアントが「無言の嘘」を消すために取る挙動を示す。中継層が非対応クライアントを代行するときの参照でもある。

1. `readiness` が `"ready"` でなく、かつ `health` が `"error"` でないとき、7.0 のメソッドの要求を出す前に `"ready"` を待つ。待つことに時間の上限を置く必要はない。応答が返らない状況の扱いはクライアント自身のタイムアウトの責任であり、本プロトコルの値とは無関係である
2. `health` が `"error"` のとき、待たずに失敗として扱う。壊れたサーバーを待ち続けることは本プロトコルの意図に反する（6 章 5 項）
3. `readiness` が `"unknown"` のとき、待たずに進む。待つべき信号が存在しない
4. 7.0 以外のメソッド（`textDocument/hover`、`textDocument/completion`、`textDocument/documentSymbol` 等）は `"indexing"` 中も待たない。これらは `coverage` の保証対象ではなく、待っても網羅は得られない
5. `health` が `"warning"` のとき、`"ok"` と同じく進む。待っても改善しない
6. 中継層として非対応クライアントを代行する場合、代行の都合で応答を返さない要求を作らない。クライアントが `$/cancelRequest` を送ったら該当する保留中の要求にキャンセルのエラーを応答し、`shutdown` を受けたら保留中の要求すべてにエラーを応答してから `shutdown` を上流へ流す

### 9.1 準拠要件（クライアント。テスト可能な形）

本章に従うクライアントは、本プロトコルに準拠したサーバー（偽のものでよい）を相手に、次を満たす。

1. サーバーが `readiness: "indexing"` を報告している間、7.0 のメソッドの要求がサーバーに届かず、`"ready"` の通知後に届く
2. サーバーが `health: "error"` を報告したとき、7.0 のメソッドの要求が待たされず、失敗として扱われる
3. サーバーが `readiness: "unknown"` を報告しているとき、7.0 のメソッドの要求が待たされない
4. サーバーが `readiness: "indexing"` を報告している間も、7.0 以外のメソッドの要求がサーバーに届く
5. 中継層として代行する場合、保留中に `$/cancelRequest` または `shutdown` を受けたとき、保留中の要求すべてに応答が返る

## 10. 既存実装との対応

| 実装                       | 既存の語彙                                                                                                                                                                                                                       | 本プロトコルへの写像                                                                                                                                                                                                                                                                                                       | 宣言できる保証（見込み）                                                                                                                                                                                                                                                                                                               |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| rust-analyzer              | `experimental/serverStatus` の `health` / `quiescent`                                                                                                                                                                            | `health` はそのまま、`quiescent: true` → `readiness: "ready"`。本プロトコルは事実上その後継                                                                                                                                                                                                                                | `coverage: {scope: "workspace", incomplete: {"workspace/symbol": 128}}`（上限は `initializationOptions.workspace.symbol.search.limit`、なければ 128）、`freshness: {fileChanges: ["Created", "Changed", "Deleted"]}`（Created / Deleted の通知で観測者が `indexing` を先読みし `quiescent: true` で戻す。ADR 0014 追補）               |
| jdtls                      | `language/status` の `ServiceReady` / `ProjectStatus`                                                                                                                                                                            | `ServiceReady` → `readiness: "ready"`、`ProjectStatus: WARNING` → `health: "warning"`                                                                                                                                                                                                                                      | `coverage: {scope: "workspace", incomplete: {}}`（見込み。未測定）                                                                                                                                                                                                                                                                     |
| gopls                      | `$/progress`（title "Setting up workspace"）の end                                                                                                                                                                               | end → `readiness: "ready"`（観測者による合成）。"Error loading workspace" の progress → `health`                                                                                                                                                                                                                           | `coverage: {scope: "workspace", incomplete: {"workspace/symbol": 100}}`（上限は固定）、`freshness: {fileChanges: ["Created", "Changed", "Deleted"]}`（v0.23.0 で確認済み。同期的に取り込む）                                                                                                                                           |
| pyright                    | `window/logMessage` のファイル列挙完了（"Found N source files" / "No source files found."）。`$/progress` は開いたファイルの解析の進行で、横断リクエストの完全性とは別                                                           | 列挙完了 → `readiness: "ready"`（観測者による合成。ワークスペースフォルダの数だけ待つ）。health の信号はなく `unknown`                                                                                                                                                                                                     | `coverage: {scope: "workspace", incomplete: {}}`、`freshness: {fileChanges: ["Changed"]}`（pyright 1.1.412 と basedpyright 1.39.8 で確認済み。Created / Deleted の後の再列挙の信号 "Found N source files" は直後の問い合わせより後に来て、除外されたファイルでは来ない）                                                               |
| typescript-language-server | `$/progress`（title "Initializing JS/TS language features…"）の begin / end。tsserver の終了は `window/logMessage`（error）"[tsserver] Exited. Code:"（言語サーバーは生き残り、空配列を成功として返す）                          | begin → `indexing`、end → `ready` かつ `health: "ok"`。終了ログ → `health: "error"`（再起動はないので戻らない）。プロジェクトはファイルを開くまでロードされない                                                                                                                                                            | `coverage: {scope: "workspace", incomplete: {}}`、`freshness: {fileChanges: ["Changed"]}`（TypeScript 5.9.3 + typescript-language-server 5.3.0 で確認済み。名乗りに出るのは TypeScript の版だけ。Created / Deleted の後に信号がなく、TypeScript の再帰ディレクトリ監視は Linux では 1 秒のタイマーで動く）                             |
| Metals                     | ビルドの取り込みの title を持つ `$/progress`（"… bspConfig"、"Importing build"、"Indexing"、"Compiling …"）。`statusType: "module"` の `metals/status`（`level` は info / warn / error。`initializationOptions` に関係なく届く） | begin → `indexing`。未完了トークンがなく、初回の取り込みが "Indexing" の end で完了していれば `ready`（その後は "Compiling" の end でも `ready`）。監視対象の変更で `indexing` を先読み（ソースは次の compile の end まで、ソースの Created / Deleted と build ファイルは次の "Indexing" の end まで）。`level` → `health` | `coverage: {scope: "workspace", incomplete: {}}`（1.6.8 で 7.2 を通過。`workspace/symbol` に上限なし）、`freshness: {fileChanges: []}`（`didChange` は presentation compiler が織り込む。ディスク上で変わったファイルの索引は "Compiling" の end の後に信号なしで作り直され、その間の約 0.1 秒はそのファイルの references が空になる） |
| Dart analysis server       | クライアントが `window.workDoneProgress` を宣言しないときは `$/analyzerStatus`（`{isAnalyzing: boolean}`。非推奨と記されている）、宣言すると token `ANALYZING`（title "Analyzing…"）の `$/progress`                              | `isAnalyzing: true` / begin → `readiness: "indexing"`、`isAnalyzing: false` / end → `"ready"`（解析のたびに対で繰り返す。rust-analyzer の `quiescent` と同型）。health の信号はなく `unknown`                                                                                                                              | `coverage: {scope: "workspace", incomplete: {}}`（見込み。語彙は 3.13.0 で実測、保証は未測定）                                                                                                                                                                                                                                         |
| Sorbet                     | `sorbet/showOperation`（`{operationName, description, status: "start" or "end"}`。クライアントが`initializationOptions.supportsOperationNotifications: true` を渡したときだけ送る）                                              | `Indexing` / `SlowPathBlocking` / `SlowPathNonBlocking` の start → `readiness: "indexing"`、最後の 1 つの end → `"ready"`（操作は重なるので数える）。Sorbet の文書は Find All References を Idle でしか答えないと明記                                                                                                      | `coverage: {scope: "workspace", incomplete: {}}`（見込み。未測定）                                                                                                                                                                                                                                                                     |
| clangd                     | なし                                                                                                                                                                                                                             | 観測者は両軸 `"unknown"` を報告する                                                                                                                                                                                                                                                                                        | 観測者経由: 保証なし。サーバー自身が実装する場合: `coverage: {scope: "openDocuments", …}` と `freshness`（全インデックスを持たない。見込み）                                                                                                                                                                                           |

`workspace/symbol` の件数の上限（2026-09-04 の実測、[research/workspace-symbol-truncation-measurement.md](../research/workspace-symbol-truncation-measurement.md)）: rust-analyzer は 128（`workspace.symbol.search.limit` で変更可。観測者はクライアントの `initializationOptions` の値を読んで宣言し、起動後の `workspace/didChangeConfiguration` による変更は宣言に反映されない）、gopls は 100（固定）。どちらも打ち切りを伝えない。pyright と typescript-language-server は打ち切らない。上限を知ったクライアントは、応答の件数が上限に達したら問い合わせを絞る。ワークスペースのシンボルを列挙したいクライアントは `textDocument/documentSymbol` をファイルごとに取る。

`experimental/serverState` という名前は rust-analyzer の `experimental/serverStatus` と近いが、これは後継であることを示す意図的な命名である。両者はクライアントのログや設定で混同しやすいため、実装・運用時は注意する。上流提案時には後継関係を明示する。

同名の再利用（`experimental/serverStatus` の流用）は採らない。ペイロードが非互換であり（`quiescent: bool` → `readiness` 3 値）、既存のパーサが同名の別 schema を受け取ることになる。さらに中継層は上流の本物の `serverStatus` を原文のまま透過するため、同名では同一接続上に schema も送信者も異なる通知が 2 系統流れて判別できない。別名であれば両者は共存できる（却下の詳細は ADR 0006 決定 4）。
