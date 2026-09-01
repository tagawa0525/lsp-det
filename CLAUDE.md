# lsp-det

LSP 拡張「Server State（拡張 S）」の参照実装となる透過プロキシ（Rust）。言語サーバーの「無言の嘘」（インデックス未完了の空応答・死んだサーバーの成功風応答・編集を織り込まない応答）を消す。最終目標は拡張の LSP 本体への提案。

## 文書の読む順序と優先度

1. `docs/spec/extension-s-server-state.md` — 拡張 S の**規範**。食い違いはすべてここが正
2. `docs/v0.1-design.md` — 実装スコープ（ゲート・アダプタ・実行モデル・マイルストーン）
3. `docs/adr/` — 決定の経緯と却下案。**採用しなかったもの**（tokio 等）の理由は ADR 0005
4. `docs/vision.md` — 長期構想（拡張 A / C は凍結中）
5. `docs/research/` — 調査報告 13 本。実装中の疑問はまず ここを検索（先行プロキシの落とし穴、各サーバーの readiness 挙動、Serena / CC の統合仕様が実測済み）

## 絶対の制約

- **仕様・設計・ADR を実装の都合で書き換えない**。実装中に仕様の矛盾・実装不能を見つけたら、勝手に直さず**報告して止まる**。仕様変更はユーザーの承認と ADR 追記が必須
- 依存の追加禁止。許可済み: `serde` / `serde_json` / `thiserror` / `libc`（ADR 0005。tokio / rayon / tracing は理由付きで不採用）
- テストの失敗を回避策で隠さない（tolerance 緩和・skip・期待値の曖昧化は禁止）
- メッセージのボディは原文バイトのまま転送する。完全パース + 再シリアライズ禁止（v0.1-design 4.6）

## 開発プロセス

- TDD 必須: RED（失敗テスト）→ GREEN（実装）→ REFACTOR を別コミットで
- feature ブランチで作業し、main へは `--no-ff` マージ（`## Why / ## What / ## Impact` 形式）
- git フックが markdownlint を強制する（表は `| --- |` 区切り、コードフェンスは言語指定、コードスパンに前後空白なし）
- GitHub リモートは作成済み（`github.com/tagawa0525/lsp-det`）。PR + レビュー待ちフローで開発する
- テストは偽上流・偽クライアントで決定的に。実サーバー結合はローカル smoke のみ（CI に入れない）

## 現在地とマイルストーン

- **M1 完了**（feat/m1-passthrough-proxy ブランチ、2026-08-28）: 素通しプロキシ。フレーミング（`src/framing.rs`）・プロセス寿命（`src/process.rs`）・イベントループ（`src/proxy.rs`）・CLI（`src/cli.rs`）を TDD で実装。28 テスト・fmt/clippy 完全通過。pdeathsig 2 経路（上流→プロキシ、プロキシ→クライアント）を手動 smoke テストで検証済み。**未検証**: 実際の rust-analyzer / gopls との統合（この開発環境では M1 完了時点で rust-analyzer が起動不能だったため、偽 LSP サーバーで代替検証した。原因は 2026-08-28 に解消済み — 下記参照）。CC プラグイン（`.lsp.json`）としての実地投入は次の課題
- **M2（進行中）**: 準拠テストスイート（中心成果物）+ 拡張 S surface + ゲート（rust-analyzer）。4 PR に分割。
  - PR 1（状態追跡）: 覗き見（`src/peek.rs`）・ServerState（`src/state.rs`）・rust-analyzer アダプタ（`src/adapter.rs`）・capability 注入（`src/initialize.rs`）・プロキシ配線と状態遷移の stderr ログ。**実 rust-analyzer との結合を検証済み**（M1 の未検証項目を解消）
  - PR 2（surface + 準拠テスト）: 拡張 S surface（`experimental/serverState` / `serverStateChanged` / グレード宣言）と、仕様 7 章を実行可能にした**準拠テストスイート**（`tests/conformance.rs`、偽上流は `examples/fake_lsp_server.rs`）。被験者を差し替えるだけで実サーバーにも当たる。lib + bin に分割済み。rust-analyzer の保証グレードは 7.2 / 7.3 を実サーバーに当てて `{completeness, freshness}` に確定
  - PR 2.5（ADR 0008 の実装）: `Readiness::Unknown` / `Health::Unknown`、状態の保持を `src/tracker.rs` に分離（アダプタは `src/adapter.rs` で解釈だけを担う）。アダプタなしは両軸 `unknown` から始まり消失で `dead`。アダプタありも `health` は最初の信号まで `unknown`（追補 E）。上流自身が `serverStateProvider` を宣言していれば拡張 S について透過（追補 D）。準拠テストにアダプタなし・準拠上流の被験者を追加
  - PR 3（予定）: ゲート（保留キュー・キャンセル・非常口タイムアウト・`health` が `error` / `dead` のときの即時エラー）。保留・転送・エラーの判定表は v0.1-design 4.2 が正。あわせて 7.2 / 7.3 を lsp-det + 偽上流でも回せるようにする
  - **未決（PR 3 の前に判断）**: 上流自身が拡張 S を宣言して中継層が透過している間、ゲートは何を追跡するか。アダプタは上流の `serverStatus` しか読まず、拡張 S をネイティブに話す上流（`serverStatus` を送らなくなる将来の rust-analyzer 等）では tracker が `{unknown, initializing}` に留まり、ゲートが非常口タイムアウトまで保留する。候補: (a) 透過中はゲートを無効にする、(b) 透過時は上流の `serverStateChanged` を読むアダプタに切り替える。ADR 0008 追補 D-3 の「内部の追跡は続ける（ゲートが使う）」はこの点を定めていない
  - 残る実測: CC のリクエストタイムアウト / エラー表示。quiescent フラップは実測完了（ADR 0007：通常編集では往復しないため対策不要）、rust-analyzer のスナップショット方式も準拠テスト 7.2 / 7.3 で確認済み
- M3: gopls アダプタ（progress 再送の実測込み）
- M4（v0.1 後）: Serena 統合・拡張 A 再評価・上流 PR

### この開発環境の rust-analyzer 起動不能問題（2026-08-28 解消）

PATH 上の `rust-analyzer` が 2 箇所とも rustup プロキシ（`rust-analyzer -> rustup` のシンボリックリンク。実体ではない）で、`/run/current-system/sw/bin/rust-analyzer`（NixOS system-wide）と `/home/tagawa/.cargo/bin/rust-analyzer`（rustup 管理）が互いにフォールバックし合い `error: infinite recursion detected` になっていた。原因は lsp-det 側ではなく、アクティブトゥールチェーン（`stable-x86_64-unknown-linux-gnu`）に `rust-analyzer` コンポーネントが未インストールだったこと。`rustup component add rust-analyzer --toolchain stable-x86_64-unknown-linux-gnu` で解消済み。

## reference/

先行事例 27 リポジトリの浅い clone（git 追跡外）。一覧と参照目的は `reference/README.md`。実装で迷ったら該当実装を読む（例: フレーミングは ra-multiplex `src/lsp/transport.rs`）。
