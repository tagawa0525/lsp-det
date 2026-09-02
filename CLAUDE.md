# lsp-det

サーバー状態プロトコル（LSP に欠けている「サーバーの状態」の語彙）の参照実装となる透過プロキシ（Rust）。言語サーバーの「無言の嘘」（インデックス未完了の空応答・壊れたサーバーの成功風応答・編集を織り込まない応答）を消す。**上流側**が言語サーバーを、**下流側**がクライアントを代行し、どちらも言語サーバー本体・クライアント本体に足りないものを示す。最終目標はプロトコルの LSP 本体への提案。

## 文書の読む順序と優先度

1. `docs/adr/README.md` — ADR の索引。**生きている決定だけ**が列挙されている。廃止された決定を読む必要はない
2. `docs/spec/server-state.md` — サーバー状態プロトコルの**規範**。食い違いはすべてここが正。3〜7 章がサーバーの義務、8 章が観測者（中継層等）の合成する値、9 章がクライアントの推奨挙動
3. `docs/v0.1-design.md` — 実装スコープ（上流側・下流側・写像・実行モデル・マイルストーン）
4. `docs/adr/` — 決定の経緯と却下案。成功基準と構造の根拠は ADR 0009、採用しなかった依存（tokio 等）の理由は ADR 0005
5. `docs/vision.md` — 長期構想（宣言範囲・起動方法の宣言は凍結中）
6. `docs/research/` — 調査報告 13 本。実装中の疑問はまずここを検索（先行プロキシの落とし穴、各サーバーの readiness 挙動、Serena / CC の統合仕様が実測済み）

## 絶対の制約

- **仕様・設計・ADR を実装の都合で書き換えない**。実装中に仕様の矛盾・実装不能を見つけたら、勝手に直さず**報告して止まる**。仕様変更はユーザーの承認と ADR 追記が必須
- 依存の追加禁止。許可済み: `serde` / `serde_json` / `thiserror` / `libc`（ADR 0005。tokio / rayon / tracing は理由付きで不採用）
- テストの失敗を回避策で隠さない（tolerance 緩和・失敗するテストの skip 化・期待値の曖昧化は禁止）。実サーバーを要するローカル smoke テストを設計段階から `#[ignore]` にしておくのは「CI で回さない」という分類であり、失敗の隠蔽ではない（v0.1-design 6 章）
- メッセージのボディは原文バイトのまま転送する。完全パース + 再シリアライズ禁止（v0.1-design 4.4）
- **時間に基づく判定を持たない**。保留の打ち切りタイマーも、一定時間で `ready` とみなす合成も禁止（仕様 6 章 6 項、ADR 0009）
- 造語を作らない。「拡張 S」「グレード」は廃止済み。概念は内容そのものの名前で呼び、LSP に既存の語彙があればそれに合わせる（ADR 0009 決定 B）

## 開発プロセス

- TDD 必須: RED（失敗テスト）→ GREEN（実装）→ REFACTOR を別コミットで
- feature ブランチで作業し、main へは `--no-ff` マージ（`## Why / ## What / ## Impact` 形式）
- git フックが markdownlint を強制する（表は `| --- |` 区切り、コードフェンスは言語指定、コードスパンに前後空白なし）
- GitHub リモートは作成済み（`github.com/tagawa0525/lsp-det`）。PR + レビュー待ちフローで開発する
- テストは偽上流・偽クライアントで決定的に。実サーバー結合はローカル smoke のみ（CI に入れない）

## 現在地とマイルストーン

成功基準は「仕様・上流側と下流側それぞれの準拠テスト・上流側と下流側の参照実装が自己無矛盾で、rust-analyzer と gopls に当てて通ること」（ADR 0009）。作者の Claude Code 環境での稼働は成功基準ではなく観測手段。

- **M1 完了**（2026-08-28）: 素通しプロキシ。フレーミング（`src/framing.rs`）・プロセス寿命（`src/process.rs`）・イベントループ（`src/proxy.rs`）・CLI（`src/cli.rs`）を TDD で実装。pdeathsig 2 経路を手動 smoke テストで検証済み
- **M2 — 上流側（rust-analyzer）**: 完了分は、覗き見（`src/peek.rs`）・状態の保持（`src/tracker.rs`）・rust-analyzer の写像（`src/adapter.rs`）・capability 注入（`src/initialize.rs`）・`experimental/serverState` / `serverStateChanged`・保証の宣言・上流側の準拠テスト（`tests/conformance.rs`、偽上流は `examples/fake_lsp_server.rs`）。7.2 / 7.3 を実 rust-analyzer に当てて `{completeness, freshness}` を確定済み。**残り（ADR 0009 の追従）**: `dead` の削除と上流消失時の保留分へのエラー応答、`serverInfo.name` による写像選択と無条件の capability 注入、版の範囲、`warning` の補正（"Failed to discover workspace." → `error`）、CLI の縮小（`lsp-det -- <上流コマンド>` のみ）、準拠テストの仕様 8.4 への追従。**現状の実装と準拠テストは ADR 0009 以前の仕様に基づく**（`Dead`、`--adapter` 等）
- **M3 — 下流側**: 保留キュー・キャンセル・`shutdown` 時のエラー応答・`error` での即時エラー・再インデックス待機。判定表は v0.1-design 4.3 が正。下流側の準拠テスト（仕様 9.1）を先に書く（RED）。打ち切りタイマーは作らない
- **M4 — gopls の写像**: progress の title からの合成。go.mod 変更時の progress 再送を実測。7.2 / 7.3 を実 gopls に当てて保証を確定
- **M5（v0.1 後）**: Serena 統合・宣言範囲の再評価・上流 PR

観測項目（ドッグフーディングで拾う事実）: CC がサーバーをいつ起動しいつ最初の横断リクエストを投げるか、CC のリクエストタイムアウトとエラーの見せ方、CC が未知の通知をどう扱うか。quiescent フラップは実測完了（ADR 0007：通常編集では往復しない）。

### この開発環境の rust-analyzer 起動不能問題（2026-08-28 解消）

PATH 上の `rust-analyzer` が 2 箇所とも rustup プロキシ（`rust-analyzer -> rustup` のシンボリックリンク。実体ではない）で、`/run/current-system/sw/bin/rust-analyzer`（NixOS system-wide）と `/home/tagawa/.cargo/bin/rust-analyzer`（rustup 管理）が互いにフォールバックし合い `error: infinite recursion detected` になっていた。原因は lsp-det 側ではなく、アクティブトゥールチェーン（`stable-x86_64-unknown-linux-gnu`）に `rust-analyzer` コンポーネントが未インストールだったこと。`rustup component add rust-analyzer --toolchain stable-x86_64-unknown-linux-gnu` で解消済み。

## reference/

先行事例 27 リポジトリの浅い clone（git 追跡外）。一覧と参照目的は `reference/README.md`。実装で迷ったら該当実装を読む（例: フレーミングは ra-multiplex `src/lsp/transport.rs`）。
