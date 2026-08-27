# LSP 不満点の統合マップとスコープ再導出

3 本の棚卸し調査（[upstream](lsp-pain-points-upstream.md) / [implementers](lsp-pain-points-implementers.md) / [agents](lsp-pain-points-agents.md)）を統合し、旧「3 拡張」の枠を外してゼロベースで拡張候補を導き直す。

## 1. 統合マップ

評価軸: 証拠の強さ（3 ソースでの独立出現）、頻度（issue 件数）、深刻度（エージェントへの実害）、プロキシで実装可能か、上流化の見込み。

| 不満領域 | 証拠 | 頻度 | 深刻度 | プロキシ実装 | 上流化 | 判定 |
| --- | --- | --- | --- | --- | --- | --- |
| インデックス未完了の空応答（readiness） | 3 ソース全て。LSP #511(8年)、Serena #1937 他多数、全ブリッジが近似実装 | 最高 | 高（誤った編集判断） | **可**（serverStatus / progress / ヒューリスティック） | 中（#1367 の close 理由 = push 型拒否に注意） | 中核 |
| サーバー死活・部分故障の無言化（health） | tsserver OOM 死後の空応答成功報告（Serena #1814）、Helix #11730、boostvolt #14、LSP #558/#646 | 高 | 高（死を成功と誤認） | **可**（プロキシは子プロセスの死・exit code・stderr を直接観測できる。サーバー自身より確実） | 中（serverStatus の health が先行） | 中核 |
| 応答の鮮度（この応答はどの編集まで織り込んだか） | matklad・michaelpj が中核批判と明言。LSP #2060 | 中 | 高（stale 応答） | **部分**（順序保証の中継 + quiescent 復帰で近似。真の因果トークンはサーバー協力が必要） | 低〜中（新概念） | 中核の隣接 |
| 診断の完了通知 | LSP #54/#737/#50024、cclsp #42、CC #80267 他 | 高 | 中〜高（stale 診断） | **部分**（push 型診断の「完了」はプロキシから観測困難。pull 型 + readiness 連動なら可） | 中 | 将来候補 |
| シンボル範囲の実装依存（旧 A） | LSP #327/#1778、SCIP の設計転換、Serena #1529/#1484（コード破損） | 低 | 最高（唯一の直接破損） | **困難**（合成にパーサ必要） | 中（新規提案可を確認済み） | 凍結継続 |
| 起動・環境の仕様外（旧 C） | 3 ソース全て。実測最大のコスト（solidlsp 78 フック、マーケットプレース issue の主成分、CC の独自スキーマ解釈バグ） | 最高 | 中（動かないだけで嘘はつかない） | 対象外（ワイヤの外） | 低（10 年放置、upstream 明示的スコープ外） | 凍結継続 |
| 無視ディレクトリの解釈差 | Serena #1806/#1729、CC #72594、solidlsp 56 クラス | 高 | 中 | 対象外（サーバー内部の挙動） | 低 | 記録のみ |
| position encoding / URI | LSP #376（歴代最多、3.17 で解決済み）、boostvolt #29 | 中 | 中 | 一部（変換は可能だが 3.17 で解決済み） | 済 | 対象外 |
| クライアント側のプロトコル準拠バグ | CC #1359/#52693、cclsp #52 | 高 | 高 | **副次的に可**（プロキシが server→client 要求を肩代わり応答） | — | ゲートの副産物 |
| 合成クエリ・出力形式・トークン効率 | LSAP / LSAI の主戦場 | 高 | 中 | 対象外 | — | 上位層（非目的） |
| 状態同期モデルの根本批判（RPC vs 購読） | matklad「最大の構造的欠陥」 | — | — | 不可（プロトコル再設計） | 極低 | 対象外 |

## 2. 導出: 統合概念「サーバー状態の真実性」

3 ソースを貫く最頻の failure pattern は**「無言の嘘」**である: 応答はプロトコル上正当だが真実ではない。

- インデックス未完了の空応答（readiness の嘘）
- 死んだ・壊れたサーバーの成功風応答（health の嘘）
- 編集を織り込んでいない応答（鮮度の嘘）

この 3 つは従来別の問題として扱われてきたが、**「サーバーの状態をクライアントが機械的に知る手段がない」という単一の欠陥の 3 側面**である。rust-analyzer の `experimental/serverStatus` が既に `health` + `quiescent` を 1 通知に載せている事実は、この統合が実装者の直観とも一致することを示す。

よって旧「拡張 B（readiness）」を、次の**拡張 S（Server State）**に再定義する:

```typescript
interface ServerState {
  /** ok: 完全に機能 / warning: 部分的 / error: 機能不全 / dead: プロセス消失 */
  health: "ok" | "warning" | "error" | "dead";
  /** initializing / indexing / ready */
  readiness: "initializing" | "indexing" | "ready";
  /**
   * 鮮度の近似: readiness が ready のとき、これまでに送信された
   * didChange をすべて織り込んだ状態であることを保証する。
   * (完全な因果トークンは将来拡張)
   */
  message?: string;
}
```

- `dead` はプロキシだからこそ正確に出せる。プロキシは子プロセスの exit・シグナル・stderr を直接観測しており、サーバー自身の自己申告より信頼できる
- 鮮度は v0.1 では「`ready` = 送信済み didChange 全反映」という契約文言で近似し、変更カウンタ等の明示的トークンは将来拡張とする
- 診断の完了は同型の問題だが通知駆動でモデルが異なるため、S の語彙に将来席を用意する（例: `phases` フィールド）に留める

## 3. 再導出後のスコープ案

| 層 | 内容 |
| --- | --- |
| v0.1 中核 | **拡張 S（Server State）**: `workspace/serverState` リクエスト + `workspace/serverStateChanged` 通知 + capability 宣言。ゲートは非対応クライアント向け互換モード（従来設計のまま、health=dead 時は保留せず即エラー化） |
| v0.1 副産物 | クライアント準拠の肩代わり（`workspace/configuration` 等への自前応答）— ゲート実装に必要な機構の再利用 |
| 将来候補 | 診断完了（S の `phases`）、鮮度トークン、範囲の契約（旧 A）、起動の宣言（旧 C） |
| 非目的 | 合成クエリ・出力形式（上位層）、状態同期モデルの再設計、多重化 |

旧 3 拡張との差分: B は S に拡大（health・鮮度契約を吸収）。A・C は「凍結」のまま位置づけ不変。ただし枠は「3 点」ではなく「無言の嘘を消す」という問題定義に置き換わり、将来候補は S の語彙の拡張として増やせる。

## 4. 旧決定への影響

- ゲート方式・argv・テスト戦略・対象 2 サーバー・CC プラグイン統合: 変更なし
- readiness の状態モデル: `ReadinessState` → `ServerState`（health 追加、dead の明示、鮮度の契約文言）
- アダプタ trait: `observe()` の出力が readiness 遷移から ServerState 遷移に広がる。gopls アダプタは progress + プロセス監視、rust-analyzer アダプタは serverStatus をほぼそのまま写像
- vision.md の拡張 B 節は S として書き直す（上流提案時は serverStatus の後継として提示、移行コスト最小の論法は不変）
