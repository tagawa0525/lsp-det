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
- **M2（次）**: 準拠テストスイート（中心成果物）+ 拡張 S surface + ゲート（rust-analyzer）。実測: CC のリクエストタイムアウト / エラー表示、rust-analyzer のスナップショット方式。rust-analyzer の起動確認は解消済み（下記）のため実施可能
- M3: gopls アダプタ（progress 再送の実測込み）
- M4（v0.1 後）: Serena 統合・拡張 A 再評価・上流 PR

### この開発環境の rust-analyzer 起動不能問題（2026-08-28 解消）

PATH 上の `rust-analyzer` が 2 箇所とも rustup プロキシ（`rust-analyzer -> rustup` のシンボリックリンク。実体ではない）で、`/run/current-system/sw/bin/rust-analyzer`（NixOS system-wide）と `/home/tagawa/.cargo/bin/rust-analyzer`（rustup 管理）が互いにフォールバックし合い `error: infinite recursion detected` になっていた。原因は lsp-det 側ではなく、アクティブトゥールチェーン（`stable-x86_64-unknown-linux-gnu`）に `rust-analyzer` コンポーネントが未インストールだったこと。`rustup component add rust-analyzer --toolchain stable-x86_64-unknown-linux-gnu` で解消済み。

## reference/

先行事例 27 リポジトリの浅い clone（git 追跡外）。一覧と参照目的は `reference/README.md`。実装で迷ったら該当実装を読む（例: フレーミングは ra-multiplex `src/lsp/transport.rs`）。
