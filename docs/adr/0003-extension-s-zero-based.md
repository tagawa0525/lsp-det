# ADR 0003: スコープをゼロベースで再導出し、拡張 B を拡張 S（Server State）に再定義する

- 日付: 2026-08-28
- 状態: 一部廃止。決定 2（`dead` の代理送出）は [ADR 0009](0009-success-criterion-and-two-sided-reference.md) が廃止。他は生きている
- 関連: [ADR 0001](0001-tool-first-readiness-gate.md)、[ADR 0002](0002-extension-b-surface-in-v01.md)

## 経緯

旧スコープ（3 拡張のうち B を中核）は、元草案の「要求最小: 3 点のみ」という枠を前提に選定されていた。実装着手前にユーザーの指示でこの枠を外し、LSP への不満の全量棚卸し（本体 issue トラッカー・実装者批判・エージェント側実害の 3 方向、docs/research/lsp-pain-points-*.md）から拡張セットを導き直した。

棚卸しの結論: 3 方向を貫く最頻の failure pattern は「無言の嘘」（プロトコル上正当だが真実ではない応答）であり、(1) インデックス未完了の空応答、(2) 死んだ・壊れたサーバーの成功風応答、(3) 編集を織り込まない応答、の 3 側面を持つ。これらは「サーバー状態をクライアントが機械的に知る手段がない」という単一欠陥の現れで、rust-analyzer の `experimental/serverStatus` が既に `health` + `quiescent` を 1 通知で扱っている。一方、旧 A（範囲。低頻度・高深刻度・パーサ必要）と旧 C（起動。最大コストだがワイヤ外・upstream 明示的スコープ外）は性質が異なり統合できない。

## 決定

1. 旧「拡張 B（Readiness）」を**拡張 S（Server State）**に再定義する。`ServerState` は `health`（`ok | warning | error | dead`）と `readiness`（`initializing | indexing | ready`）の 2 軸。メソッドは `workspace/serverState` / `workspace/serverStateChanged`、capability は `experimental.serverStateProvider`（サーバー側）/ `experimental.serverState`（クライアント側）
2. `dead` はプロキシが子プロセスの消失を観測して代理送出する値とする（サーバーの自己申告では原理的に出せない）
3. 鮮度は v0.1 では契約文言（「`ready` のとき受信済み didChange は織り込み済み」）で保証し、明示的な因果トークンは将来拡張とする。プロキシの観測だけでは厳密に保証できない値を仕様に入れない
4. 診断の完了は拡張 S の前方互換な将来拡張（`phases`）として予約のみ行う。push 型診断の完了はプロキシから観測困難で、v0.1 に入れると精度の低い実装になる
5. 初期草案の `$partial` 注釈・`completeMethods` / `partialMethods` は廃止する（配列応答に付けられず、無改造クライアントに効果がない）
6. 「3 点」という枠は廃止し、問題定義「無言の嘘を消す」に置き換える。旧 A・C は独立の凍結項目として維持
7. **成果物の優先順位**: 問題を解決する枠組み（プロトコル定義、docs/spec/）が第一の成果物であり、実装は可能な範囲でそれを実証する。規範は docs/spec/extension-s-server-state.md に置き、他文書と食い違う場合は仕様を正とする。本決定は [ADR 0001](0001-tool-first-readiness-gate.md) 決定 1 を一部改訂する（優先関係の裁定は [ADR 0006](0006-external-review-fixes.md) 決定 1）

## 影響

- v0.1 の実装増分は小さい（health のプロセス監視は元々 4.7 節で必要だった機構の再利用。ゲートは dead 時に即時エラーの分岐が加わるのみ）
- 上流提案の物語が強くなる: 「serverStatus の後継として health を保ち quiescent を 3 値化した」という移行コスト最小の形になり、tsserver のクラッシュ判別不能・Serena #1814（OOM 死の成功報告）という実害に直接効く
- ゲート方式・argv・テスト戦略・対象 2 サーバー・CC プラグイン統合は変更なし
