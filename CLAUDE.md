# lsp-det

サーバー状態プロトコル（LSP に欠けている「サーバーの状態」の語彙）の参照実装となる透過プロキシ（Rust）。言語サーバーの「無言の嘘」（インデックス未完了の空応答・壊れたサーバーの成功風応答・編集を織り込まない応答）を消す。**上流側**が言語サーバーを、**下流側**がクライアントを代行し、どちらも言語サーバー本体・クライアント本体に足りないものを示す。最終目標はプロトコルの LSP 本体への提案。

## 文書の読む順序と優先度

1. `docs/adr/README.md` — ADR の索引。**生きている決定だけ**が列挙されている。廃止された決定を読む必要はない
2. `docs/spec/server-state.md` — サーバー状態プロトコルの**規範**。食い違いはすべてここが正。3〜7 章がサーバーの義務、8 章が観測者（中継層等）の合成する値、9 章がクライアントの推奨挙動
3. `docs/v0.1-design.md` — 実装スコープ（上流側・下流側・写像・実行モデル・マイルストーン）
4. `docs/adr/` — 決定の経緯と却下案。成功基準と構造の根拠は ADR 0009、採用しなかった依存（tokio 等）の理由は ADR 0005
5. `docs/vision.md` — 長期構想（宣言範囲・起動方法の宣言は凍結中）
6. `docs/research/` — 調査報告 22 本。実装中の疑問はまずここを検索（先行プロキシの落とし穴、各サーバーの readiness 挙動、Serena / CC の統合仕様が実測済み、CC 経由のドッグフーディング観測は `claude-code-dogfooding.md`）

## 絶対の制約

- **仕様・設計・ADR を実装の都合で書き換えない**。実装中に仕様の矛盾・実装不能を見つけたら、勝手に直さず**報告して止まる**。仕様変更はユーザーの承認と ADR 追記が必須
- 依存の追加禁止。許可済み: `serde` / `serde_json` / `thiserror` / `libc`（ADR 0005。tokio / rayon / tracing は理由付きで不採用）
- テストの失敗を回避策で隠さない（tolerance 緩和・失敗するテストの skip 化・期待値の曖昧化は禁止）。実サーバーを要するローカル smoke テストを設計段階から `#[ignore]` にしておくのは「CI で回さない」という分類であり、失敗の隠蔽ではない（v0.1-design 6 章）
- メッセージのボディは原文バイトのまま転送する。完全パース + 再シリアライズ禁止（v0.1-design 4.4）
- **時間に基づく判定を持たない**。保留の打ち切りタイマーも、一定時間で `ready` とみなす合成も禁止（仕様 6 章 6 項、ADR 0009）
- 造語を作らない。「拡張 S」「グレード」は廃止済み。概念は内容そのものの名前で呼び、LSP に既存の語彙があればそれに合わせる（ADR 0009 決定 B）

## 開発環境

- `flake.nix` が Rust ツールチェーン・rust-analyzer・go・gopls を固定する（nixpkgs はシステム構成と同じ rev）。`nix develop` か direnv（`.envrc` は `use flake` + `PATH_add target/release`。グローバルの gitignore に負けるので `git add -f` で追跡している）で入る
- 対応 OS は Linux・macOS・Windows（ADR 0012）。プロセス寿命の追従は `src/process/{linux,macos,windows}.rs` に分かれている。他 OS のコンパイルは `scripts/check-targets.sh`（rustup の stable でクロスターゲットの `cargo check`）で push の前に確かめ、挙動は GitHub Actions の CI（`.github/workflows/ci.yml`、3 OS で `cargo test`）が確かめる。`v*` のタグで `.github/workflows/release.yml` が各 OS のバイナリを Release に添付する
- 言語サーバーの版は保証の宣言に直結する（`src/adapter/*/TESTED_VERSIONS`）。`flake.lock` を更新して版が変わったら `cargo test --test conformance -- --ignored` を通してから一覧を動かす
- ドッグフーディングは `dogfood/README.md`（`cargo build --release` → `claude --plugin-dir dogfood/claude-plugin`）。Serena は `dogfood/serena/README.md`
- 上流に出す変更は `scripts/upstream/README.md` の手順でローカルに確かめる（pyright・typescript-language-server・rust-analyzer・gopls の 4 つの上流に当てるパッチは fork のブランチに用意済み。上流への PR はユーザー確認のうえで出す。出すものの一覧と順序は `docs/upstream-submissions.md`）（`reference/` の clone をビルドして `target/upstream/bin` を PATH の先頭に置き、`tests/upstream_dev.rs` の受け入れ条件と準拠テストを当てる）。Serena 側は `scripts/serena/probe.py`

## 開発プロセス

- TDD 必須: RED（失敗テスト）→ GREEN（実装）→ REFACTOR を別コミットで
- feature ブランチで作業し、main へは `--no-ff` マージ（`## Why / ## What / ## Impact` 形式）
- git フックが markdownlint を強制する（表は `| --- |` 区切り、コードフェンスは言語指定、コードスパンに前後空白なし）
- GitHub リモートは作成済み（`github.com/tagawa0525/lsp-det`）。PR + レビュー待ちフローで開発する
- テストは偽上流・偽クライアントで決定的に。実サーバー結合はローカル smoke のみ（CI に入れない）

## 現在地

成功基準は「仕様・上流側と下流側それぞれの準拠テスト・上流側と下流側の参照実装が自己無矛盾で、rust-analyzer と gopls に当てて通ること」（ADR 0009）。作者の Claude Code 環境での稼働は成功基準ではなく観測手段。

- v0.1（M1〜M4: 素通しプロキシ、上流側、下流側、gopls の写像）と v0.2（ADR 0010 の M5〜M7: pyright、typescript-language-server、Serena 統合。ADR 0012 の 3 OS 対応）は完了。マイルストーンごとの内容と日付は `CHANGELOG.md`
- 次は 0.3.0: ADR 0013（`completeness` を `coverage` に改名し `workspace/symbol` を保証から外す）、0014（`didChangeWatchedFiles` を `freshness` に加える）、0015（下流側の代行: `didChangeWatchedFiles` の代行と重複 `didOpen` の書き換え）、0016（保証の宣言を、欠けを名指しする形に: `coverage: {scope, incomplete}`、`freshness: {fileChanges}`）。ADR を先に書いてから実装する。外向きの提出は `docs/upstream-submissions.md`
- 実サーバーの結合テストは `cargo test --test conformance -- --ignored`（19 件）と `cargo test --test process_lifetime -- --ignored`（4 件）。`TESTED_VERSIONS` を動かすのはこれらを通してから

ドッグフーディングは `dogfood/README.md` の手順。観測結果は `docs/research/claude-code-dogfooding.md` に追記する（第 1〜3 回で、経路の成立・起動直後の横断リクエストが保留されて完全な結果になること・82 秒の保留でも CC がタイムアウトしないこと・gopls 経路・`error` の拒否の見せ方を確認済み）。観測項目: CC がサーバーをいつ起動しいつ最初の横断リクエストを投げるか、CC のリクエストタイムアウトとエラーの見せ方、CC が未知の通知をどう扱うか。quiescent フラップは実測完了（ADR 0007: 通常編集では往復しない）。

### この開発環境の rust-analyzer 起動不能問題（2026-08-28 解消）

PATH 上の `rust-analyzer` が 2 箇所とも rustup プロキシ（`rust-analyzer -> rustup` のシンボリックリンク。実体ではない）で、`/run/current-system/sw/bin/rust-analyzer`（NixOS system-wide）と `/home/tagawa/.cargo/bin/rust-analyzer`（rustup 管理）が互いにフォールバックし合い `error: infinite recursion detected` になっていた。原因は lsp-det 側ではなく、アクティブトゥールチェーン（`stable-x86_64-unknown-linux-gnu`）に `rust-analyzer` コンポーネントが未インストールだったこと。`rustup component add rust-analyzer --toolchain stable-x86_64-unknown-linux-gnu` で解消済み。

## reference/

先行事例 27 リポジトリの浅い clone（git 追跡外）。一覧と参照目的は `reference/README.md`。実装で迷ったら該当実装を読む（例: フレーミングは ra-multiplex `src/lsp/transport.rs`）。
