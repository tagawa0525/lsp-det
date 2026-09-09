# lsp-det

サーバー状態プロトコル（LSP に欠けている「サーバーの状態」の語彙）の参照実装となる透過プロキシ（Rust）。言語サーバーの「無言の嘘」（インデックス未完了の空応答・壊れたサーバーの成功風応答・編集を織り込まない応答）を消す。**上流側**が言語サーバーを、**下流側**がクライアントを代行し、どちらも言語サーバー本体・クライアント本体に足りないものを示す。最終目標はプロトコルの LSP 本体への提案。

## 文書の読む順序と優先度

1. `docs/adr/README.ja.md` — ADR の索引。**生きている決定だけ**が列挙されている。廃止された決定を読む必要はない
2. `docs/spec/server-state.md`（英語が正。日本語版は `docs/spec/server-state.ja.md`）— サーバー状態プロトコルの**規範**。食い違いはすべてここが正。3〜7 章がサーバーの義務、8 章が観測者（中継層等）の合成する値、9 章がクライアントの推奨挙動
3. `docs/v0.1-design.ja.md` — 実装スコープ（上流側・下流側・写像・実行モデル・マイルストーン）
4. `docs/adr/` — 決定の経緯と却下案。成功基準と構造の根拠は ADR 0009、採用しなかった依存（tokio 等）の理由は ADR 0005
5. `docs/vision.ja.md` — 長期構想（宣言範囲・起動方法の宣言は凍結中）
6. `docs/glossary.md` — 日本語と英語の対訳表。仕様・README・コードのコメントの訳語はここに合わせる
7. `docs/research/` — 調査報告 43 本。実装中の疑問はまずここを検索（先行プロキシの落とし穴、各サーバーの readiness 挙動、Serena / CC の統合仕様が実測済み、CC 経由のドッグフーディング観測は `claude-code-dogfooding.md`）

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

- 英語が正: `README.md`、`docs/spec/server-state.md`、そして `README.md` から直接リンクする 6 本（`docs/vision.md`、`docs/v0.1-design.md`、`docs/research/claude-code-dogfooding.md`、`docs/adr/README.md`、`scripts/upstream/README.md`、`dogfood/serena/README.md`。追補 F）。日本語版（同名の `.ja.md`）は**同じコミット**で追従させ、見出しの構成を 1 対 1 に保つ。レビューは日本語版で行う
- 英語のみ: `src` / `tests` / `examples` のコメントとテスト名、実行時のメッセージ、`dogfood/README.md`、`docs/research/README.md`（調査報告の索引）
- 日本語: 個々の ADR、`docs/research/` の各報告、`scripts/serena/README.md` 等の upstream 以外の `scripts/*/README.md`、本ファイル、コミットメッセージ、PR 本文、CHANGELOG。英語の文書からこれらへリンクするときはリンクテキストに "(Japanese)" を付ける
- 訳語は `docs/glossary.md` に合わせる。変えるときは表を先に直す

## 開発環境

- `flake.nix` の `default` はビルドの道具だけ、`servers` は言語サーバー全部（rust-analyzer・gopls・pyright・basedpyright・typescript-language-server・clangd。版の固定はここ。nixpkgs はシステム構成と同じ rev）。実サーバーの結合テストとドッグフーディングは `nix develop .#servers` か direnv（`.envrc` は `use flake .#servers` + `PATH_add target/release`。グローバルの gitignore に負けるので `git add -f` で追跡している）で入る
- 対応 OS は Linux・macOS・Windows（ADR 0012）。プロセス寿命の追従は `src/process/{linux,macos,windows}.rs` に分かれている。他 OS のコンパイルは `scripts/check-targets.sh`（rustup の stable でクロスターゲットの `cargo check`）で push の前に確かめ、挙動は GitHub Actions の CI（`.github/workflows/ci.yml`、3 OS で `cargo test`）が確かめる。`v*` のタグで `.github/workflows/release.yml` が各 OS のバイナリを Release に添付する
- 言語サーバーの版は保証の宣言に直結する（`src/adapter/*/TESTED_VERSIONS`）。`flake.lock` を更新して版が変わったら `cargo test --test conformance -- --ignored` を通してから一覧を動かす
- ドッグフーディングは `dogfood/README.md`（`cargo build --release` → `claude --plugin-dir dogfood/claude-plugin`）。Serena は `dogfood/serena/README.ja.md`
- 上流に出す変更は `scripts/upstream/README.ja.md` の手順でローカルに確かめる（pyright・typescript-language-server・rust-analyzer・gopls の 4 つの上流に当てるパッチは fork のブランチに用意済み。上流への PR はユーザー確認のうえで出す。出すものの一覧と順序は `docs/upstream-submissions.md`）（`reference/` の clone をビルドして `target/upstream/bin` を PATH の先頭に置き、`tests/upstream_dev.rs` の受け入れ条件と準拠テストを当てる）。Serena 側は `scripts/serena/probe.py`

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
- 0.4.0（ADR 0018〜0019: 外部レビューへの対応、コーパス、反例の実サーバーでの検証）も完了。写像は Metals、Expert、Nextflow、haskell-language-server、crystalline、Gleam、haxe-language-server を足し（各 `docs/research/*-readiness-measurement.md`）、pyrefly は写像なし、Vue は合成の測定のみ、Kotlin と sourcekit-lsp は入手できる版の都合で保留。版が語彙に現れないサーバーには保証を宣言しない
- 0.5.0（ADR 0020: Dart、Sorbet、jdtls、clangd）も完了（各 `docs/research/*-readiness-measurement.md`。マイルストーンごとの内容は `CHANGELOG.md`）。Dart（`$/progress` の token `ANALYZING`）と jdtls（`language/status` の `ServiceReady`、health は `ProjectStatus` とプロジェクト自身の URI の診断）は要求をサーバー自身が待たせ、通した版に coverage と freshness を宣言（Dart の freshness は `didChange` だけ。ディスク上の変更はサーバー自身の監視が非同期に拾い、通知の直後の問い合わせに窓がある）。Sorbet（`sorbet/showOperation` の入れ子カウント。名乗りは通知そのもので、`initializationOptions` の注入はコマンド名 `sorbet` / `srb` に限る。決定 D）は版が語彙に現れず保証なし。clangd（背景索引の `$/progress`、token `backgroundIndexProgress`）はサーバー自身が要求を待たせず索引中は部分応答なので lsp-det の保留が効き、coverage のみ宣言（`didChange` の後に信号のない古い窓があり、ディスク上の変更は取り込まれない）。compile_commands.json のないワークスペースは、最初の `didOpen` で観測者が clangd 自身の探索（`compile_commands.json` / `build/compile_commands.json` / `compile_flags.txt`、`--compile-commands-dir` があればそこだけ）をなぞってデータベースの所在を読み、見つからなければ `unknown` にする（ADR 0020 追補 2026-09-09。ユーザーの決定。当初の決定 (a)「`initializing` のまま」は横断リクエストが永久に保留されるため改めた）。仕様 10 章の clangd の行と 8.2 の 3 の例はユーザーの承認を得て訂正済み。
- 0.6.0（ADR 0021: nixd、nil、ドッグフーディングの本番化）も完了。nixd（"evaluating …" の `$/progress` を数える。索引に依る `definition` はサーバー自身が待たせる）と nil（固定 token 3 つの `$/progress`、health は `window/showMessage` の type 1 / 2。flake.lock の読み込みには信号がない）は `references` が要求のあった文書に閉じるので、仕様 5 章の `coverage.scope` に `"document"` を足し（決定 E。ユーザーの決定）、通した版に `coverage` だけ宣言する。nil は begin が一度も来ない workspace（flake がない、flake.lock がない、nixpkgs の入力がない、入力の store path がない）の扱いを (b) `unknown` から始めるに決めた（2026-09-09、ADR 0021 追補。`initialize` の `workspaceFolders` の `flake.lock` の root の入力に `nixpkgs` があるかで判定し、begin 前の type 2 の `window/showMessage` も readiness を `unknown` にする。永遠に `initializing` のまま保留し続ける (a) は使いにくいという判断）。ruff の LSP は横断要求がなく写像しない。ドッグフーディングは nixfiles が lsp-det を flake input として取り込み `~/.claude/skills/lsp-det-dogfood` に置く形（`packages.default`。`--plugin-dir` は作業木の変更を観るときだけ）。保留の Kotlin と sourcekit-lsp は次の版が入手できたら測る
- 0.7.0（ADR 0022: 仕様を安定版 1.0 に。ADR 0020 / 0021 の追補: begin の来ない workspace の clangd と nil は `unknown` から始めて保留しない。対外戦略は `docs/upstream-submissions.md`、その批判的レビューは `docs/research/upstream-strategy-review-2026-09.md`）も完了。仕様の版は 1.0 で、以後の変更は版を上げて 11 章に記す。README.md から直接リンクする文書は英語が正（ADR 0017 追補 F）。準備 2（tsls のパッチの作り直し）は済で、typescript-language-server/typescript-language-server#1125 として提出済み（2026-09-09。第 1 段の最初の提出）: fork の `tsserver-exit-by-signal`（`onExit` の `if (exitCode)` を外す。受け入れ条件は `typescript_language_server_exits_when_tsserver_is_killed`、再測定は `docs/research/typescript-language-server-readiness-measurement.md` の末尾）。準備 3（gopls を health に縮める）も済: `docs/research/gopls-health-measurement.md`（失敗中の要求は明示的なエラー、失敗の信号は `window` を宣言しないクライアントには type 4 の showMessage に落ちる、回復後の go.mod 変更に約 1 秒の窓）。golang/go#78273 へのコメントと窓の新規 issue golang/go#81400 として提出済み（2026-09-09）。準備 4（rust-analyzer の `serverStatus` への field 追加案）も済: fork の `server-status-readiness`（`ServerStatusParams` に `readiness` を足し初期状態を `initializing` に。受け入れ条件は `rust_analyzer_reports_readiness_in_server_status`、写像は field があればそれを読む）。実測（`scripts/rust-analyzer/status-probe.py`、`docs/research/rust-analyzer-quiescent-measurement.md` の末尾）で Cargo.toml のないディレクトリでは最初のロードの前に `quiescent: true` が送られることを確認。issue の草案は `docs/upstream-submissions.md`。準備 5（LSP 本体向けの `proposed.serverState.ts`）も済: fork `tagawa0525/vscode-languageserver-node` の `server-state`（`proposed.serverState.ts` と同名の `.md`、`api.ts` の `Proposed`、`metaModel.json`。方法名は採用後の `workspace/serverState` / `workspace/serverStateChanged`、capability は `workspace.serverState: boolean` と `serverStateProvider`）。#511 へのコメントと proposal issue の草案は `docs/upstream-submissions.md`。準備 6（Serena の再測定）も済: 上流 HEAD `701e7c84` で前回と同じ結果（`docs/research/serena-integration-measurement.md` の末尾）。クラッシュ検知が 2 回目以降の横断要求に効かない穴は HEAD にも oraios/serena#1978 にも残り、修正は fork `tagawa0525/serena` の `tsserver-crash-on-request-path`（latch の前で `_raise_if_crashed()`。受け入れ条件は `scripts/serena/probe.py` の `CRASH=1 VIA_LSP_DET=0` がコード 0 で終わること）。不具合 4 件は HEAD に残る。PR・issue 4 件・registry（oraios/serena#1988）への提案の草案は `docs/upstream-submissions.md`。準備 7（README の Nix を使わない導入手順の点検）も済: v0.7.0 の Linux のバイナリが glibc 2.39 に動的リンクされ「静的」の記述が誤りだったので、Release の Linux を musl の静的リンクに切り替え（ADR 0012 追補 2026-09-09。次の `v*` のタグから）、README を事実に合わせた。0.7.1（2026-09-09）は Linux の Release バイナリを musl に直した版で、lsp-det の挙動は 0.7.0 と同じ。第 1 段の提出は済（2026-09-09。Claude Code へのコメント 4 件 anthropics/claude-code#76870、#82416、#85225、#16360 と新規 issue 2 件 #93103（`shutdown` の `params: {}`）、#93104（`didClose`）、pyright の enhancement request microsoft/pyright#11723。出した文面は `docs/upstream-submissions.md` の草案の節にそのまま残す。出す直前の再測定は `scripts/claude-code/tee-probe` で CC 2.1.266）。次: 応答待ちと第 2 段（Serena、rust-analyzer。文面を作ってユーザーの確認をもらってから出す）
- 実サーバーの結合テストは `cargo test --test conformance -- --ignored`（75 件。全件は `--test-threads=1` で回す。並列では tsls の 7.3 の Changed が負荷で揺れる。Metals、Expert、Nextflow、HLS、pyrefly、crystalline、Gleam、haxe-language-server、Dart、Sorbet、jdtls、clangd、nixd、nil は `nix develop .#servers` で。nixd は `NIX_PATH` に nixpkgs、nil は `nix` が要る）と `cargo test --test process_lifetime -- --ignored`（4 件）。`TESTED_VERSIONS` を動かすのはこれらを通してから

ドッグフーディングは `dogfood/README.md` の手順。観測結果は `docs/research/claude-code-dogfooding.ja.md` に追記する（第 1〜3 回で、経路の成立・起動直後の横断リクエストが保留されて完全な結果になること・82 秒の保留でも CC がタイムアウトしないこと・gopls 経路・`error` の拒否の見せ方を確認済み。第 4 回で CC が送る通知の全数、第 5 回（CC 2.1.261）で `didChangeWatchedFiles` の代行が効くことと、Write の再 `didOpen` が CC 側で直ったことを確認済み。第 6 回で実害の一事例（直接では tsls と gopls の両方でエージェントが使われている関数を消しビルドが壊れる。lsp-det 経由では消さない）を記録済み。第 7 回（CC 2.1.263）で日常の環境（flake input と skills-as-plugins）から Nix・Rust・Python の 3 経路の保留と解放を確認済み。CC は `workspace/configuration` を支持しない）。観測項目: CC がサーバーをいつ起動しいつ最初の横断リクエストを投げるか、CC のリクエストタイムアウトとエラーの見せ方、CC が未知の通知をどう扱うか。quiescent フラップは実測完了（ADR 0007: 通常編集では往復しない）。

### この開発環境の rust-analyzer 起動不能問題（2026-08-28 解消）

PATH 上の `rust-analyzer` が 2 箇所とも rustup プロキシ（`rust-analyzer -> rustup` のシンボリックリンク。実体ではない）で、`/run/current-system/sw/bin/rust-analyzer`（NixOS system-wide）と `/home/tagawa/.cargo/bin/rust-analyzer`（rustup 管理）が互いにフォールバックし合い `error: infinite recursion detected` になっていた。原因は lsp-det 側ではなく、アクティブトゥールチェーン（`stable-x86_64-unknown-linux-gnu`）に `rust-analyzer` コンポーネントが未インストールだったこと。`rustup component add rust-analyzer --toolchain stable-x86_64-unknown-linux-gnu` で解消済み。

## reference/

先行事例 27 リポジトリの浅い clone（git 追跡外）。一覧と参照目的は `reference/README.md`。実装で迷ったら該当実装を読む（例: フレーミングは ra-multiplex `src/lsp/transport.rs`）。
