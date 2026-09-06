# lsp-det

サーバー状態プロトコル（LSP に欠けている「サーバーの状態」の語彙）の参照実装となる透過プロキシ（Rust）。言語サーバーの「無言の嘘」（インデックス未完了の空応答・壊れたサーバーの成功風応答・編集を織り込まない応答）を消す。**上流側**が言語サーバーを、**下流側**がクライアントを代行し、どちらも言語サーバー本体・クライアント本体に足りないものを示す。最終目標はプロトコルの LSP 本体への提案。

## 文書の読む順序と優先度

1. `docs/adr/README.md` — ADR の索引。**生きている決定だけ**が列挙されている。廃止された決定を読む必要はない
2. `docs/spec/server-state.md`（英語が正。日本語版は `docs/spec/server-state.ja.md`）— サーバー状態プロトコルの**規範**。食い違いはすべてここが正。3〜7 章がサーバーの義務、8 章が観測者（中継層等）の合成する値、9 章がクライアントの推奨挙動
3. `docs/v0.1-design.md` — 実装スコープ（上流側・下流側・写像・実行モデル・マイルストーン）
4. `docs/adr/` — 決定の経緯と却下案。成功基準と構造の根拠は ADR 0009、採用しなかった依存（tokio 等）の理由は ADR 0005
5. `docs/vision.md` — 長期構想（宣言範囲・起動方法の宣言は凍結中）
6. `docs/glossary.md` — 日本語と英語の対訳表。仕様・README・コードのコメントの訳語はここに合わせる
7. `docs/research/` — 調査報告 25 本。実装中の疑問はまずここを検索（先行プロキシの落とし穴、各サーバーの readiness 挙動、Serena / CC の統合仕様が実測済み、CC 経由のドッグフーディング観測は `claude-code-dogfooding.md`）

## 絶対の制約

- **仕様・設計・ADR を実装の都合で書き換えない**。実装中に仕様の矛盾・実装不能を見つけたら、勝手に直さず**報告して止まる**。仕様変更はユーザーの承認と ADR 追記が必須
- 依存の追加禁止。許可済み: `serde` / `serde_json` / `thiserror` / `libc`（ADR 0005。tokio / rayon / tracing は理由付きで不採用）
- テストの失敗を回避策で隠さない（tolerance 緩和・失敗するテストの skip 化・期待値の曖昧化は禁止）。実サーバーを要するローカル smoke テストを設計段階から `#[ignore]` にしておくのは「CI で回さない」という分類であり、失敗の隠蔽ではない（v0.1-design 6 章）
- メッセージのボディは原文バイトのまま転送する。完全パース + 再シリアライズ禁止（v0.1-design 4.4）
- **時間に基づく判定を持たない**。保留の打ち切りタイマーも、一定時間で `ready` とみなす合成も禁止（仕様 6 章 6 項、ADR 0009）
- 造語を作らない。「拡張 S」「グレード」は廃止済み。概念は内容そのものの名前で呼び、LSP に既存の語彙があればそれに合わせる（ADR 0009 決定 B）
- **信号は他の実装から推測しない**。Serena 等の待ち方（sleep、安全バッファ、正規表現）には CI の都合や手癖が混ざる。写像を書く前に、そのサーバー自身の文書とソースで信号の有無と意味を確かめる（ADR 0018 決定 C）
- 外部へのアクション（上流の PR、issue、報告）は 0.5.0 の後。それまでは理想を追い根源的に解く。仕様が動くなら写像・テスト・fork のパッチを追従させ、手戻りの大きさを変えない理由にしない（ADR 0018 決定 D、ADR 0019）

## 言語（ADR 0017）

- 英語が正: `README.md`、`docs/spec/server-state.md`。日本語版（`README.ja.md`、`docs/spec/server-state.ja.md`）は**同じコミット**で追従させ、見出しの構成を 1 対 1 に保つ。レビューは日本語版で行う
- 英語: `src` / `tests` / `examples` のコメントとテスト名、実行時のメッセージ、`dogfood/README.md`
- 日本語: ADR、`docs/research/`、`docs/v0.1-design.md`、`docs/vision.md`、`scripts/*/README.md`、`dogfood/serena/README.md`、本ファイル、コミットメッセージ、PR 本文、CHANGELOG
- 訳語は `docs/glossary.md` に合わせる。変えるときは表を先に直す

## 開発環境

- `flake.nix` の `default` はビルドの道具だけ、`servers` は言語サーバー全部（rust-analyzer・gopls・pyright・basedpyright・typescript-language-server。版の固定はここ。nixpkgs はシステム構成と同じ rev）。実サーバーの結合テストとドッグフーディングは `nix develop .#servers` か direnv（`.envrc` は `use flake .#servers` + `PATH_add target/release`。グローバルの gitignore に負けるので `git add -f` で追跡している）で入る
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
- 0.3.0（ADR 0013〜0016: `coverage` への改名、`didChangeWatchedFiles` の鮮度と先読み、下流側の代行 2 つ、欠けを名指しする宣言の形）も完了。ADR 0017 の英訳 3 つとドッグフーディング第 5 回も完了
- 外部レビュー（ADR 0018）への対応と 0.4.0（ADR 0019）を進行中。済み: 保留の開始と解放のログ、ドッグフーディング第 6 回、文書 3 件（仕様 10 章の Dart / Sorbet、gopls #1200 の証拠、提出メモの前提）。M14 devShell の分割と M8 コーパス（`docs/research/readiness-vocabulary-corpus.md`。70 サーバー全部が 4 値に写り、新しい値は要らない）も済み。検証する言語は ADR 0019 の追補（決定 F）で 11 に確定。M9 Metals は済み（`docs/research/metals-readiness-measurement.md`。時間なしで写像でき、`freshness.fileChanges` は空）。M10 Expert（Elixir）は済み（`docs/research/expert-readiness-measurement.md`。readiness は写像できるが、読み込んだ索引の再索引が続くかを区別する信号がなく保証は宣言しない）。M12 Nextflow の言語サーバーも済み（`docs/research/nextflow-readiness-measurement.md`。走査の完了を示す信号がなく、観測者が `workspaceFolders` を歩いて走査の集合を再現する。`serverInfo` がなく版が語彙に現れないので保証は宣言しない）。M11 Kotlin（JetBrains 版）は最新 release v262.9593.0 が "This build of intellij-server has expired" で起動せず、次の release まで保留。M15 haskell-language-server も済み（`docs/research/haskell-language-server-readiness-measurement.md`。トークンは lsp ライブラリの 1 秒の抑制でほぼ出ず、索引中の `references` は増え続ける部分応答。readiness は `unknown`、health は cradle の診断から。保証は宣言しない）。M16 pyrefly も済み（`docs/research/pyrefly-readiness-measurement.md`。起動時の索引はプロトコルに出ず、両軸 `unknown`。写像なし）。M17 crystalline も済み（`docs/research/crystalline-readiness-measurement.md`。M18 sourcekit-lsp は nixpkgs が 5.10.1 で `backgroundIndexing`（6.0 以降）を測れず保留（`docs/research/sourcekit-lsp-readiness-measurement.md`。5.10.1 は `libIndexStore.so` がなく索引を読めない）。M19 Gleam も済み（`docs/research/gleam-readiness-measurement.md`。次: M20 Haxe → M13 Vue（合成の測定） → 0.5.0（Dart、Sorbet、jdtls、clangd）→ 外向きの提出（`docs/upstream-submissions.md` の順。文面を作ってユーザーの確認をもらってから出す）
- 実サーバーの結合テストは `cargo test --test conformance -- --ignored`（46 件。Metals、Expert、Nextflow、HLS、pyrefly、crystalline、Gleam は `nix develop .#servers` で）と `cargo test --test process_lifetime -- --ignored`（4 件）。`TESTED_VERSIONS` を動かすのはこれらを通してから

ドッグフーディングは `dogfood/README.md` の手順。観測結果は `docs/research/claude-code-dogfooding.md` に追記する（第 1〜3 回で、経路の成立・起動直後の横断リクエストが保留されて完全な結果になること・82 秒の保留でも CC がタイムアウトしないこと・gopls 経路・`error` の拒否の見せ方を確認済み。第 4 回で CC が送る通知の全数、第 5 回（CC 2.1.261）で `didChangeWatchedFiles` の代行が効くことと、Write の再 `didOpen` が CC 側で直ったことを確認済み。第 6 回で実害の一事例（直接では tsls と gopls の両方でエージェントが使われている関数を消しビルドが壊れる。lsp-det 経由では消さない）を記録済み）。観測項目: CC がサーバーをいつ起動しいつ最初の横断リクエストを投げるか、CC のリクエストタイムアウトとエラーの見せ方、CC が未知の通知をどう扱うか。quiescent フラップは実測完了（ADR 0007: 通常編集では往復しない）。

### この開発環境の rust-analyzer 起動不能問題（2026-08-28 解消）

PATH 上の `rust-analyzer` が 2 箇所とも rustup プロキシ（`rust-analyzer -> rustup` のシンボリックリンク。実体ではない）で、`/run/current-system/sw/bin/rust-analyzer`（NixOS system-wide）と `/home/tagawa/.cargo/bin/rust-analyzer`（rustup 管理）が互いにフォールバックし合い `error: infinite recursion detected` になっていた。原因は lsp-det 側ではなく、アクティブトゥールチェーン（`stable-x86_64-unknown-linux-gnu`）に `rust-analyzer` コンポーネントが未インストールだったこと。`rustup component add rust-analyzer --toolchain stable-x86_64-unknown-linux-gnu` で解消済み。

## reference/

先行事例 27 リポジトリの浅い clone（git 追跡外）。一覧と参照目的は `reference/README.md`。実装で迷ったら該当実装を読む（例: フレーミングは ra-multiplex `src/lsp/transport.rs`）。
