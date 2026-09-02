# ADR 0002: 拡張 B の surface を v0.1 に含め、ゲートは互換モードと位置づける

- 日付: 2026-08-28
- 状態: 一部廃止。決定 1・2 の名称と `completeMethods` は [ADR 0003](0003-extension-s-zero-based.md) が改名・廃止。決定 4 の位置づけは [ADR 0009](0009-success-criterion-and-two-sided-reference.md) 決定 D-7 が置き換え。決定 3・5 は生きている
- 関連: [ADR 0001](0001-tool-first-readiness-gate.md)

## 経緯

ADR 0001 のツール先行の決定を検討する過程で、v0.1 のクライアント向け surface を「ゼロ（素の LSP + ゲートによる意味論の強化のみ）」とする案が出た。呼ぶクライアントが存在しない API は検証できない、という理由による。

しかしこの案は本プロジェクトの出発点と矛盾する。目的は「準備完了を区別する語彙がプロトコルにない」という LSP の欠陥に対して独自拡張（拡張 B: `workspace/readiness`）を定義・実証し、最終的に LSP 本体へ提案することであり、vision.md 4.1 は参照プロキシを「クライアントからは準拠サーバーに見える」ものと定義している。surface ゼロのゲート専用プロキシは、Serena 等が内部に持つ shim と同類の応急処置に留まり、補正を構造的に消すという目的に到達しない。

## 決定

1. v0.1 のプロキシは拡張 B の surface を実装し、**この拡張の最初の準拠実装**となる: `InitializeResult.capabilities.readinessProvider` の宣言、`workspace/readiness` リクエストへの応答、`workspace/readinessChanged` 通知の送出
2. v0.1 の `ReadinessState` は `state`（3 値）のみ必須。`completeMethods` 等はアダプタが判定できる場合のみ
3. クライアントが `ClientCapabilities.experimental.readiness: true` を宣言した場合はゲートを無効化する（自分で判断できるクライアントを妨げない）
4. ゲートは拡張を呼ばないクライアント（現在の Claude Code / Serena）のための**互換モード**であり、既定動作として維持する
5. 「呼ぶ側不在」の懸念は、readiness を呼ぶ偽クライアントをテストに含めることで解消する。これは vision.md 2.4 の準拠テストの雛形を兼ねる

## 影響

- lsp-det は「shim の一種」ではなく「拡張 B の仕様 + 参照実装 + 準拠テスト + 実測データ」の一体物になり、上流（rust-analyzer / gopls / LSP 本体 issue #511）への提案物がそのまま揃う
- 実装増分は薄い（内部状態機械は元々 `ReadinessState` 相当を計算しており、外に出す層のみ）
- 将来 Serena に readiness 対応の PR を出す際、呼び先が既に存在する状態から始められる
