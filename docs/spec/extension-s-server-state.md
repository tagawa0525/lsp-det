# LSP 拡張仕様: Server State（拡張 S）

状態: 草案。本文書が拡張 S の規範（normative）であり、他文書の記述と食い違う場合は本文書を正とする。

## 1. 目的

LSP には、サーバーの状態（生きているか・要求に完全に答えられるか・どの編集まで織り込んでいるか）をクライアントが機械的に知る手段がない。その結果、プロトコル上は正当だが真実ではない応答 — インデックス未完了の空配列、死んだサーバーの沈黙、編集を織り込まない stale な結果 — をクライアントが信じてしまう。本拡張はサーバー状態を機械可読な語彙で表し、この「無言の嘘」を消す。

本拡張は情報を渡すだけであり、クライアントの挙動を強制しない。状態を見て待つか、部分的な結果を承知で進むかは、クライアントが判断する。

## 2. 対象読者と送出主体

本仕様の実装者は 3 者である。

- **サーバー**: 言語サーバー本体
- **クライアント**: エディタ、エージェント、ブリッジ
- **中継層**: サーバーとクライアントの間に立つプロキシ等。サーバーに代わって本拡張を提供でき、サーバープロセスを外から観測できる

値ごとの送出主体は 6 章の表で規定する。

## 3. 型定義

```typescript
interface ServerState {
  /**
   * "ok":      完全に機能している
   * "warning": 部分的に機能している（依存欠落等。結果が不完全になりうる）
   * "error":   機能していない（結果は信頼できない）
   * "dead":    サーバープロセスが存在しない。以後の要求には応答できない
   */
  health: "ok" | "warning" | "error" | "dead";

  /**
   * "initializing": initialize 直後。まだ何も答えられない
   * "indexing":     一部の要求に答えられるが、結果が不完全になりうる
   * "ready":        インデックスが完了している
   * "unknown":      readiness を観測する手段がない。中継層のみが送出する（6.1）
   */
  readiness: "initializing" | "indexing" | "ready" | "unknown";

  /** 人間向けの補足。機械判定に使ってはならない */
  message?: string;
}
```

`health` と `readiness` は独立の 2 軸である。推奨解釈（非規範）: `health` が `error` または `dead` のとき、`readiness` を判断材料に使うべきではない。待機の終了については 6 章 5 項が規範である。

`readiness` が `unknown` のとき、クライアントは基本グレード（5.1）と同じく、応答が不完全でありうることを承知で進むか、自前で待つかを判断する。`readiness` に失敗を表す値はない。インデックスの失敗は `health` で表す（6 章 5 項）。

前方互換のため、`ServerState` に未知のフィールドが含まれてもクライアントはエラーにせず無視しなければならない。予約済みの拡張候補: `phases`（診断等のフェーズ別完了状態）、鮮度トークン（織り込み済み変更の識別子）。

## 4. メソッド

### 4.1 状態の問い合わせ

```text
Request:  experimental/serverState
Params:   なし
Response: ServerState
```

### 4.2 状態変化の通知

```text
Notification: experimental/serverStateChanged
Params:       ServerState
```

`health` または `readiness` が変わるたびに送る。同一状態の重複送信は許容されるが推奨しない。`indexing` 中の細かい進捗は送らない（進捗表示は既存の `$/progress` の役割）。

### 4.3 命名

メソッド名は LSP 本体に取り込まれるまで `experimental/` プレフィックスを用いる（rust-analyzer の `experimental/serverStatus` と同じ慣行）。取り込み時に `workspace/serverState` / `workspace/serverStateChanged` へ改名する。

## 5. Capability と保証グレード

```typescript
// サーバー・中継層 → クライアント (InitializeResult)
interface ServerCapabilities {
  experimental?: {
    serverStateProvider?: boolean | {
      /**
       * readiness が "ready" のとき、7.1 に列挙するワークスペース横断
       * メソッドの応答が完全である（後から同じ問い合わせの結果が
       * 増えることがない）ことを保証する。
       */
      completeness?: boolean;
      /**
       * readiness が "ready" のとき、受信済みの textDocument/didChange を
       * すべて織り込んだ応答を返すことを保証する。
       */
      freshness?: boolean;
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

### 5.1 保証グレード

`serverStateProvider: true`（または両フラグなし）は**基本グレード**であり、状態の通知そのものだけを保証する。`indexing` 中の応答が不完全でありうるという警告として、それだけでも価値を持つ。

`completeness` と `freshness` は**独立**であり、順序関係はない。現実のサーバーは 4 象限すべてに存在する:

| | freshness | freshness なし |
| --- | --- | --- |
| **completeness** | スナップショット方式 + 全インデックス（rust-analyzer） | 全インデックスだが非同期処理（tsserver 系） |
| **completeness なし** | リクエスト毎スナップショットだが全インデックスなし（clangd） | 基本グレード |

実装は自分が守れる保証だけを宣言する。守れない保証の宣言は本仕様への違反である。

### 5.2 クライアント宣言の意味

クライアントの `experimental.serverState: true` は、通知の購読要求であると同時に、**「状態を自分で解釈し、待つか進むかを自分で判断する」という意思表示**である。中継層はこの宣言を見て、非対応クライアント向けの保護動作（要求の保留等）を解除してよい。宣言したクライアントが状態を無視して不完全な結果を得た場合、それはそのクライアントの責任である。

サーバー・中継層は、クライアントがこの宣言をした場合のみ `experimental/serverStateChanged` を送る。`experimental/serverState` リクエストは宣言の有無によらず応答する。

## 6. セマンティクス

1. **完全性**（`completeness` 宣言時）: `readiness` が `"ready"` のとき、7.1 のメソッドへの応答は完全な結果でなければならない
2. **鮮度**（`freshness` 宣言時）: `readiness` が `"ready"` のとき、それまでに受信した `textDocument/didChange` はすべて織り込み済みでなければならない。この保証の実質はクロスファイルの鮮度（変更したファイル以外を起点とする問い合わせに、インデックスが変更を反映していること）である。単一ファイル内の変更→問い合わせの多くは、LSP の既存の処理順序保証（同一接続では後続リクエストが先行通知の後に処理される）だけで満たされる
3. **再インデックス**: ワークスペースの再解析（依存ファイル変更、ブランチ切り替え等）が始まったら、`readiness` を `"indexing"` に戻して通知しなければならない
4. **既存機構との関係**: `$/progress` は人間向けの進捗表示であり本拡張を代替しない。`ServerCancelled` エラーはポーリングを強いるため本拡張を代替しない（LSP issue #1367 の議論を参照）
5. **待機の終了**: `health` が `error` または `dead` のとき、クライアントは `readiness` が `"ready"` になるのを待ってはならない。サーバーはインデックスの失敗を `readiness` ではなく `health` で表す。`readiness` は `"indexing"` に留めても `"ready"` にしてもよく、待つ側は `health` だけを見て抜ける

### 6.1 値ごとの送出主体

| 値 | サーバー | 中継層 |
| --- | --- | --- |
| `health: ok / warning / error` | 送出可 | 送出可（上流の状態の転写または推定） |
| `health: dead` | **送出してはならない**（死んだプロセスは通知を送れない） | 送出可（プロセス消失の観測に基づく） |
| `readiness: initializing / indexing / ready` | 送出可 | 送出可 |
| `readiness: unknown` | **送出してはならない**（サーバーは自分の readiness を必ず知っている） | 送出可（観測手段がないとき） |

`dead` は終端状態である。`dead` と `unknown` はいずれも観測者だけが出せる値であり、主な送出者は中継層である。本拡張が LSP 本体へ提案される際には両者の位置づけ（クライアントライブラリが接続断や観測手段の欠如から合成する値とする等）を再検討する。

## 7. 準拠要件（グレード別・テスト可能な形）

準拠テストの fixture は非自明な規模（インデックスに観測可能な時間を要するファイル数）でなければならない。

### 7.0 対象メソッド（completeness の保証対象）

`textDocument/references`, `textDocument/definition`, `textDocument/typeDefinition`, `textDocument/declaration`, `textDocument/implementation`, `workspace/symbol`, `textDocument/prepareCallHierarchy`, `callHierarchy/incomingCalls`, `callHierarchy/outgoingCalls`, `textDocument/rename`, `textDocument/prepareRename`

### 7.1 基本グレード

1. `initialize` 完了直後の `experimental/serverState` に応答し、その時点で `readiness` は `"ready"` ではない
2. クライアントが capability を宣言した場合のみ `experimental/serverStateChanged` が届く
3. `readiness` を `"unknown"` 以外で報告する実装では、依存変更の後に `"ready"` → `"indexing"` → `"ready"` の遷移が観測できる

### 7.2 completeness 宣言時

1. `"ready"` になった後の `textDocument/references` が、事前計算した完全な結果と一致する

### 7.3 freshness 宣言時

1. ファイル A への `textDocument/didChange`（別ファイル B から参照されるシンボルの追加・削除）送信後、`readiness` が `"ready"` の状態で **B を起点に**発行した横断問い合わせ（`textDocument/references` 等）の応答が、A の変更を反映している

テストは必ずクロスファイル（変更したファイルとは別のファイルを起点とし、インデックス経由でしか到達できない結果）で行わなければならない。単一ファイルの変更→問い合わせは LSP の処理順序保証だけで通ってしまい、freshness を検証しない（6 章 2 項）。

## 8. 既存実装との対応

| 実装 | 既存の語彙 | 拡張 S への写像 | 宣言できるグレード（見込み） |
| --- | --- | --- | --- |
| rust-analyzer | `experimental/serverStatus` の `health` / `quiescent` | `health` はそのまま、`quiescent: true` → `readiness: "ready"`。本拡張は事実上その後継 | completeness + freshness（準拠テスト 7.2 / 7.3 で確認済み） |
| jdtls | `language/status` の `ServiceReady` / `ProjectStatus` | `ServiceReady` → `readiness: "ready"`、`ProjectStatus: WARNING` → `health: "warning"` | completeness |
| gopls | `$/progress`（title "Setting up workspace"）の end | end → `readiness: "ready"`（中継層による合成） | completeness（freshness は要実測） |
| pyright | workDoneProgress の end（残り解析ファイル数 0 と同期） | 同上 | completeness |
| tsserver 系 | `$/progress`（クラッシュ時も end） | 中継層がログ・接続監視と併用して合成 | completeness のみ（非同期処理のため freshness 不可） |
| clangd | なし | 中継層は `readiness: "unknown"` と `health`（プロセス観測による `ok` / `dead`）のみ提供 | 中継層経由: 基本グレード。サーバー自身が実装する場合: freshness のみ（全インデックスを持たない） |

`experimental/serverState` という名前は rust-analyzer の `experimental/serverStatus` と近いが、これは後継であることを示す意図的な命名である。両者はクライアントのログや設定で混同しやすいため、実装・運用時は注意する。上流提案時には後継関係を明示する。

同名の再利用（`experimental/serverStatus` の流用）は採らない。ペイロードが非互換であり（`quiescent: bool` → `readiness` 3 値、`health` に `dead` が追加）、既存のパーサが同名の別 schema を受け取ることになる。さらに中継層は上流の本物の `serverStatus` を原文のまま透過するため、同名では同一接続上に schema も送信者も異なる通知が 2 系統流れて判別できない。別名であれば両者は共存できる（却下の詳細は ADR 0006 決定 4）。
