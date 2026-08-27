# LSP 拡張仕様: Server State（拡張 S）

状態: 草案。本文書が拡張 S の規範（normative）であり、他文書の記述と食い違う場合は本文書を正とする。

## 1. 目的

LSP には、サーバーの状態（生きているか・要求に完全に答えられるか・どの編集まで織り込んでいるか）をクライアントが機械的に知る手段がない。その結果、プロトコル上は正当だが真実ではない応答 — インデックス未完了の空配列、死んだサーバーの沈黙、編集を織り込まない stale な結果 — をクライアントが信じてしまう。本拡張はサーバー状態を機械可読な単一の語彙で表し、この「無言の嘘」を消す。

## 2. 型定義

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
   * "ready":        すべての要求に完全な結果を返せる
   * health が "dead" の場合、readiness に意味はない
   */
  readiness: "initializing" | "indexing" | "ready";

  /** 人間向けの補足。機械判定に使ってはならない */
  message?: string;
}
```

将来拡張のため、`ServerState` に未知のフィールドが含まれてもクライアントはエラーにせず無視しなければならない（前方互換）。予約済みの拡張候補: `phases`（診断等のフェーズ別完了状態）、鮮度トークン（織り込み済み変更の識別子）。

## 3. メソッド

### 3.1 状態の問い合わせ

```text
Request:  workspace/serverState
Params:   なし
Response: ServerState
```

### 3.2 状態変化の通知

```text
Notification: workspace/serverStateChanged
Params:       ServerState
```

サーバーは `health` または `readiness` が変わるたびに送る。同一状態の重複送信は許容されるが推奨しない。`indexing` 中の細かい進捗は送らない（進捗表示は既存の `$/progress` の役割）。

## 4. Capability

```typescript
// サーバー → クライアント (InitializeResult)
interface ServerCapabilities {
  experimental?: {
    serverStateProvider?: boolean;
  };
}

// クライアント → サーバー (InitializeParams)
interface ClientCapabilities {
  experimental?: {
    serverState?: boolean;
  };
}
```

サーバーはクライアントが `experimental.serverState: true` を宣言した場合のみ `workspace/serverStateChanged` を送る。`workspace/serverState` リクエストは宣言の有無によらず応答する。

## 5. セマンティクス

1. **完全性の保証**: `readiness` が `"ready"` のとき、以後の要求への応答は完全な結果でなければならない。「完全」とは、ワークスペース全体を把握した上での結果であり、後から（再インデックス等で）同じ問い合わせの結果が増えることがない、という意味である
2. **鮮度の保証**: `readiness` が `"ready"` のとき、それまでにサーバーが受信した `textDocument/didChange` はすべて織り込み済みでなければならない
3. **再インデックス**: ワークスペースの再解析（依存ファイル変更、ブランチ切り替え等）が始まったら、サーバーは `readiness` を `"indexing"` に戻して通知しなければならない
4. **dead**: `"dead"` は終端状態であり、サーバー自身は送れない。プロキシ等の中継層がプロセス消失を観測して代理送出する値である
5. **既存機構との関係**: `$/progress` は人間向けの進捗表示であり本拡張を代替しない。`ServerCancelled` エラーはポーリングを強いるため本拡張を代替しない（LSP issue #1367 の議論を参照）

## 6. 準拠要件（テスト可能な形）

準拠実装は以下をすべて満たす。

1. `initialize` 完了直後の `workspace/serverState` に応答し、その時点で `readiness` は `"ready"` ではない（空のワークスペースを除く）
2. `readiness` が `"ready"` になった後の `textDocument/references` は、事前計算した完全な結果と一致する
3. `readiness` の `"ready"` → `"indexing"` → `"ready"` の遷移が、依存変更の後に観測できる
4. クライアントが capability を宣言した場合のみ `workspace/serverStateChanged` が届く

## 7. 既存実装との対応

| 実装 | 既存の語彙 | 拡張 S への写像 |
| --- | --- | --- |
| rust-analyzer | `experimental/serverStatus` の `health` / `quiescent` | `health` はそのまま、`quiescent: true` → `readiness: "ready"`。最も近い先行実装であり、本拡張は事実上その後継 |
| jdtls | `language/status` の `ServiceReady` / `ProjectStatus` | `ServiceReady` → `readiness: "ready"`、`ProjectStatus: WARNING` → `health: "warning"` |
| gopls | `$/progress`（title "Setting up workspace"）の end | end → `readiness: "ready"`（中継層による合成） |
| pyright | workDoneProgress の end（残り解析ファイル数 0 と同期） | 同上 |
