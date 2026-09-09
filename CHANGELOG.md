# Changelog

マイルストーンの完了と、その決定の出所（ADR）を版ごとに記す。設計判断の経緯は `docs/adr/`、実測は `docs/research/`。

## 予定

- 外向きの提出（戦略・順序・規則は `docs/upstream-submissions.md`。文面を作ってユーザーの確認をもらってから出す）: 提出前の準備（済: typescript-language-server のパッチの作り直し（fork の `tsserver-exit-by-signal`、2026-09-09。同日 typescript-language-server/typescript-language-server#1125 として提出）。gopls を health に縮める（2026-09-09、`docs/research/gopls-health-measurement.md`。同日 golang/go#78273 へのコメントと golang/go#81400 として提出）。rust-analyzer の `serverStatus` への field 追加案（2026-09-09、fork の `server-status-readiness`。issue の草案は両案を並べる）。LSP 本体向けの `proposed.serverState.ts`（2026-09-09、fork `tagawa0525/vscode-languageserver-node` の `server-state`。#511 へのコメントと proposal issue の草案も）。Serena の再測定（2026-09-09、上流 HEAD `701e7c84` で前回と同じ結果。クラッシュ検知の穴の修正は fork の `tsserver-crash-on-request-path`、不具合 4 件は HEAD に残る。PR・issue・registry への提案の草案も）。README の Nix を使わない導入手順の点検（2026-09-09。Linux の Release バイナリを musl の静的リンクに。ADR 0012 追補））→ 第 1 段（済: typescript-language-server/typescript-language-server#1125、Claude Code のコメント 4 件と新規 issue 2 件（anthropics/claude-code#93103、anthropics/claude-code#93104）、microsoft/pyright#11723。2026-09-09）→ 第 2 段（Serena の registry、rust-analyzer の両案、gopls の health）→ 第 3 段（12 サーバー、LSP 本体）
- 保留の再測定: Kotlin（次の release）、sourcekit-lsp（nixpkgs に 6.x が来たら）

## 0.7.1（2026-09-09）

Release の Linux バイナリを直す版。提出前の準備はすべて済み、同日に第 1 段を提出した（予定の節）。

- **Linux の Release バイナリ**（ADR 0012 追補、PR #97）: v0.7.0 までの `*-unknown-linux-gnu` はランナーの glibc 2.39 に動的リンクされ、Ubuntu 22.04 や Debian 12 では起動できなかった。`x86_64-unknown-linux-musl` と `aarch64-unknown-linux-musl` の静的リンクに切り替え、ワークフローに静的であることの確認を足した。README の導入手順を事実に合わせ（バイナリの名前、署名なしの macOS の隔離、引数なしの起動での確認）、`scripts/check-targets.sh` に musl を足した
- **提出前の準備**（PR #91、#92、#94、#96）: rust-analyzer の `serverStatus` への field 追加案（fork の `server-status-readiness`）、LSP 本体向けの `proposed.serverState.ts`（fork `tagawa0525/vscode-languageserver-node` の `server-state`）、Serena の上流 HEAD での再測定とクラッシュ検知の修正（fork の `tsserver-crash-on-request-path`）。草案は `docs/upstream-submissions.md`
- 実サーバー結合テストと lsp-det の挙動は 0.7.0 から変わらない

## 0.7.0（2026-09-09）

外向きの提出に向けた版。対外戦略を決め、begin の来ない workspace で永遠に保留する 2 つの写像を `unknown` に改め、仕様を安定版 1.0 にした。

- **対外戦略**（PR #80、#83）: `docs/upstream-submissions.md` を備忘録から戦略の文書に。別のセッションの批判的レビュー（`docs/research/upstream-strategy-review-2026-09.md`）を受け、上流が既に持っている答え（tsls #305、gopls の `awaitLoaded` と `gopls mcp`、rust-analyzer #10888）の上に載せる順序と規則に改めた。消費者を Claude Code → lsp-det → Serena の順に立てる
- **nil**（ADR 0021 追補、PR #81）: begin が一度も来ない workspace（flake がない、flake.lock がない、`nixpkgs` の入力がない、入力の store path がない）の扱いを (a) `initializing` のままから (b) `unknown` に。`initialize` の `workspaceFolders` の `flake.lock` の root の入力に `nixpkgs` があるかで判定し、begin 前の type 2 の `window/showMessage` も readiness を `unknown` にする。ユーザーの決定
- **clangd**（ADR 0020 追補、PR #82）: compile_commands.json のない workspace の決定 (a) を改め、最初の `didOpen` で観測者が clangd と同じ場所（ファイルのディレクトリから根まで `compile_commands.json` / `build/compile_commands.json` / `compile_flags.txt`。`--compile-commands-dir` があればそこだけ）を探し、見つからなければ `unknown` に。上流の引数を写像に知らせる `learn_upstream_arguments` を追加。ユーザーの決定
- **仕様 1.0**（ADR 0022、PR #84）: `Status` を安定版にし、仕様に固有の版 1.0 を v0.7.0 で凍結。6 章 4 項にサーバー側の保留との関係（保留は `health` と `freshness` を言えず判断をクライアントから奪う。保留は禁じない）。8.1 の `readiness: "unknown"` に「信号が来ないと観測者が判断した」を含める。11 章に変更記録
- **英語版と体裁**（ADR 0017 追補 F、PR #86）: README.md から直接リンクする 6 本（vision、v0.1-design、ドッグフーディングの記録、ADR の索引、`scripts/upstream/README.md`、`dogfood/serena/README.md`）は英語が正になり、日本語版は同名の `.ja.md`。`docs/research/README.md` に英語の索引。README にバッジ・導入・License の節。MIT OR Apache-2.0 の二重ライセンス（ユーザーの決定）、Cargo.toml のメタデータ、GitHub の description と topics

## 0.6.0（2026-09-08）

作者の日常の言語（Rust、Python、Nix）のうち写像のなかった Nix を当て、ドッグフーディングを日常の環境に載せる ADR 0021 のバッチ。2 サーバーとも `references` が要求のあった文書に閉じ、仕様の `coverage.scope` に `"document"` を足した。

- **ADR 0021**（2026-09-08）: 順序は nixd → nil → ドッグフーディングの本番化（M25〜M27）、1 つずつ PR。ruff の LSP は横断要求も走査もなく写像しない（A-4）。`references` が単一文書に閉じる 2 サーバーの `coverage.scope` の問い（決定 E）は実測の後にユーザーが (b) と決めた。外向きの提出は 0.6.0 の後
- **nixd**（M25、PR #73）: "evaluating …" の `$/progress`（token は乱数の整数。既定で 2 本並行）を未完了の集合で数え、空になった end で `ready`。索引に依る `definition` はサーバー自身が評価の完了まで待たせる。health の信号はなし（失敗の end も "evaluated …"、worker の死で nixd 自身が SIGPIPE で落ちる）。上流への提案 3 件
- **nil**（M26、PR #76）: 固定 token 3 つの `$/progress` を数え、`window/showMessage` の type 1 / 2 を health の `error` / `warning` に。flake.lock の読み込み（約 100 ms）には信号がなく、nil は要求を待たせないので、その窓の `definition` は宣言しないクライアントには lsp-det の保留が埋める。begin が来ない workspace では `initializing` のまま（(a)。(b) との選択は保留）。上流への提案 3 件
- **仕様の変更**（決定 E、PR #78。ユーザーの決定）: 5 章の `coverage.scope` に `"document"`（要求が名指した文書だけ）。8.1 の 1 と 7.2 の 1 を追従。nixd 2.9.2 と nil 2026-07-23 に `coverage: {scope: "document", incomplete: {}}` を宣言し、`freshness` は宣言しない（7.3 は横断を要する）
- **ドッグフーディングの本番化**（M27、PR #74、#77、nixfiles PR #180）: rust-analyzer 1.97.1（rust-overlay）を結合テスト 6 件に通して `TESTED_VERSIONS` に。`.lsp.json` に `.nix`（nixd）。flake に `packages.default`（lsp-det 本体）を足し、nixfiles が flake input として取り込んで `~/.claude/skills/lsp-det-dogfood` に置く（skills-as-plugins。`--plugin-dir` は要らない）。第 7 回で Nix・Rust・Python の 3 経路の保留と解放を確認
- **実測の記録**: `docs/research/` に 2 本（nixd、nil の readiness）とドッグフーディング第 7 回。コーパスに nil と ruff の行
- 実サーバーの結合テストは 73 件（直列で全部通過）

## 0.5.0（2026-09-07）

固まった語彙に易しい 4 サーバーを当てる ADR 0020 のバッチ。4 つとも実測して写像を書き、3 つは「サーバー自身が要求を待たせる」型で、1 つ（clangd）は lsp-det の保留がそのまま効く型だった。

- **ADR 0020**（2026-09-06）: 順序は Dart → Sorbet → jdtls → clangd（M21〜M24）、1 サーバー 1 PR。Sorbet は rubygems の `sorbet-static` の prebuilt を derivation に（決定 B）。Sorbet の `supportsOperationNotifications` は `initializationOptions` に、lsp-det が起動したコマンドが `sorbet` / `srb` のときだけ注入する（決定 D。コマンド名は注入にしか使わない）。版が語彙に現れないサーバーには保証を宣言しない（決定 E）
- **Dart analysis server**（M21、PR #67）: `$/progress`（token `ANALYZING`）の begin で `indexing`、end で `ready`。サーバー自身が要求を解析の完了まで待たせるので先読みは要らない。`workspace/didChangeWatchedFiles` は読まれず type 1 の `showMessage` が返る。ディスク上の変更はサーバー自身の監視が非同期に拾い、通知の直後の問い合わせに信号のない窓がある（66 件の直列実行で 7.3 の 3 が 1 度落ちた。PR #71）ので、3.13.0 に coverage と `didChange` だけの freshness を宣言
- **Sorbet**（M22、PR #69）: `sorbet/showOperation` の要求に伴わない操作（`Indexing`、`SlowPathBlocking`、`SlowPathNonBlocking`、`FastPath`）を入れ子ぶん数え、未完了がなくなった end で `ready`。`serverInfo` がなく名乗りは通知そのもので、版が語彙に現れず保証なし。ディスク上の変更は watchman が root を watch しているときだけ拾い、Sorbet の `subscribe` は `watch-project` を発行しない
- **jdtls**（M23、PR #68）: `language/status` の `ServiceReady` で `ready`（`$/progress` は読まない。JDT の検索が索引の完了を待つ）。health は `ProjectStatus` の OK / WARNING、`Error`、プロジェクト自身の URI への診断（壊れた classpath は `ProjectStatus` に出ず診断に出る）。1.60.0-SNAPSHOT に coverage と freshness を宣言
- **clangd**（M24、PR #70）: 背景索引の `$/progress`（token `backgroundIndexProgress`、title "indexing"）の begin で `indexing`、end で `ready`。索引中の `references` は空応答から増え続ける部分応答で、lsp-det の保留で最初の答えから完全になる。`didChange` の後に信号のない古い窓があり、ディスク上の変更は取り込まれないので coverage のみ宣言。`compile_commands.json` のないワークスペースでは begin が来ず `initializing` のまま（決定 (a)）
- **仕様の訂正**（ユーザーの承認済み）: 10 章の clangd の行「信号なし」を実測に置き換え、8.2 の 3 の例を clangd から pyrefly に、5.1 の象限の表の例を「compile_commands.json のない clangd」にする
- **lsp-det の直し**: tracker が通知から写像を選ぶとき、その通知を写像にも読ませる（Sorbet の名乗りは入れ子の外側の start そのもので、捨てると内側の end で `ready` を言っていた）。`ServerStateProvider::coverage_only`。上流のコマンド名の basename を小文字に正規化して `.exe` を落とす
- **実測の記録**: `docs/research/` に 4 本（dart、sorbet、jdtls、clangd の readiness）。上流への提出候補は `docs/upstream-submissions.md` に 4 サーバー分を追記
- 実サーバーの結合テストは 65 件（直列で全部通過）

## 0.4.0（2026-09-06）

外部レビュー（ADR 0018）への対応と、コーパスの反例を実サーバーで検証する ADR 0019 のバッチ。写像を 7 つ足し、2 つは「写像を書かない」が正直な答えだと確かめ、2 つは入手できる版の都合で保留にした。

- **ADR 0018**（2026-09-06）: 外部レビューの採否。保留の開始と解放を理由付きで stderr に出す（A-1、PR #49）。ドッグフーディング第 6 回で実害の一事例を記録（A-2）。仕様 10 章に Dart / Sorbet の行と gopls #1200 の一次資料（A-3、A-4、PR #50）。提出メモに「上流は未試行」の前提（A-6）。信号は他の実装から推測しない（C）。外向きの提出は 0.5.0 の後（D）
- **ADR 0019**（2026-09-06、追補で 11 言語に確定）: devShell を `default`（道具だけ）と `servers`（言語サーバー全部）に分ける（M14）。Serena の 70 サーバーの readiness の語彙をコーパスにし、全部が 4 値に写り新しい値は要らないと確かめる（M8、`docs/research/readiness-vocabulary-corpus.md`）
- **写像を足したもの**: Metals（M9。`coverage` あり、`freshness.fileChanges` は空。「時間でしか終わりを言えない」を覆した）、Expert（M10。readiness のみ）、Nextflow の言語サーバー（M12。走査の完了を示す信号がなく、観測者が `workspaceFolders` を歩いて走査の集合を再現する）、haskell-language-server（M15。readiness は `unknown`。`$/progress` は lsp ライブラリの 1 秒の抑制でほぼ出ず、索引中の `references` は増え続ける。health は cradle の診断から）、crystalline（M17。readiness は起動ログ "LSP server is ready."）、Gleam（M19。依存ダウンロードのトークンはダウンロードするものがなくても出る）、haxe-language-server（M20。起動系の title 3 つで readiness、`window/showMessage` と "Haxe connected!" で health）。`serverInfo` のないサーバーを `InitializeResult` の `executeCommandProvider.commands` や起動時の通知で識別する経路を足した。版が語彙に現れないサーバー（Nextflow、HLS、crystalline、Gleam、Haxe）には保証を宣言しない
- **写像を書かなかったもの**: pyrefly（M16。起動時の索引は stderr にしか出ず両軸 `unknown`）。Vue（M13。相方サーバーとの合成は決定 B-5 のとおりクライアントの責務で、横断の答えを出す tsls の接続の保留だけで完全。vision に記載）
- **保留**: Kotlin（M11。JetBrains kotlin-lsp の最新 release が期限切れで起動しない）、sourcekit-lsp（M18。nixpkgs は 5.10.1 で `backgroundIndexing` は 6.0 以降。`libIndexStore.so` がなく索引を読めない）
- **lsp-det の直し**: `InitializeResult` の `experimental: null` を欠落と同じに扱う。`didOpen` / `didClose` も写像に見せる。準拠テストのクライアントがサーバーからの要求に応答し、通知が来ないときは被験体の stderr を出す
- **実測の記録**: `docs/research/` に 9 本（metals、expert、nextflow、haskell-language-server、pyrefly、crystalline、sourcekit-lsp、gleam、haxe-language-server の readiness、vue の合成）。上流への提出候補は `docs/upstream-submissions.md` に 6 サーバー分を追記（提出は 0.5.0 の後）
- 実サーバーの結合テストは 48 件（直列で全部通過。並列では tsls の 7.3 の Changed が負荷で揺れることがある）

## 0.3.0（2026-09-06）

2026-09-04 に決めたバッチ。ADR 4 本を先に書き、続けて実装した。

- **ADR 0013**（2026-09-04）: `completeness` を `coverage` に改名し、定義を「ワークスペース全体のインデックスに基づき、インデックスの進行によって後から結果が増えない」に絞る。`workspace/symbol` の扱いは 0016 が置き換えた
- **ADR 0014**（2026-09-04、追補 2026-09-06）: `freshness` の対象に、受信した `workspace/didChangeWatchedFiles` を加える。準拠テスト 7.3 を Changed / Created / Deleted に分ける。追補: 通知の後に完了の信号が必ず来ると測った写像（rust-analyzer）だけが、Created / Deleted の通知で `indexing` を先読みしてよい
- **ADR 0015**（2026-09-04）: 下流側の代行 2 つ。capability を宣言せず通知も送らないクライアントに代わって、7.0 のリクエストごとに `git ls-files` の一覧の mtime を比べて `didChangeWatchedFiles` を送る（`src/watched_files.rs`。写像は関与せず、git 管理外では行わない。4269 ファイルの zed で 1 回 22ms）。既に開いている uri への `didOpen` を全文の `didChange` に書き換える（`src/documents.rs`）
- **ADR 0016**（2026-09-06）: 保証の宣言を、真偽値ではなく欠けを名指しする形にする。`serverStateProvider` は常にオブジェクトで、`coverage: {scope: "workspace" | "openDocuments", incomplete: {メソッド: 上限}}`、`freshness: {fileChanges: FileChangeType の一覧}`。仕様はあるべき姿を書き、現実のずれは宣言で自覚させる。7.0 は 1 つの一覧に戻す
- **各サーバーの宣言**（実測に基づく）: rust-analyzer は `incomplete: {"workspace/symbol": 128}`（`initializationOptions` の上限を読む）と `fileChanges` 3 種、gopls は 100 と 3 種、pyright と typescript-language-server は `incomplete: {}` と `["Changed"]`（Created / Deleted の取り込みの開始を伝えない）
- **実測**: `workspace/symbol` の打ち切り、ディスク上の編集の伝わり方（4 サーバー × 4 場面、Claude Code の `initialize` の capability の原文、通知後の完了の信号）、Serena が MCP ツールと LSP の間で行う処理、言語サーバーが埋められている穴の分類（約 40 言語）
- **上流**: fork の rust-analyzer と gopls のパッチを新しい宣言に追従（受け入れ条件は通過）。`docs/upstream-submissions.md` に提出の備忘録

## 0.2.0（2026-09-04）

ADR 0010 の 3 マイルストーンと、ADR 0012 の OS 対応。

- **M5 pyright の写像**（2026-09-03、ADR 0011）: `src/adapter/pyright.rs`。readiness の信号は `window/logMessage` のファイル列挙完了（"Found N source files"）で `$/progress` ではない。pyright は `serverInfo` を返さないので起動ログの名乗りで写像を選ぶ。7.2 / 7.3 を pyright 1.1.412 と basedpyright 1.39.8 で通し、製品ごとの一覧で保証を宣言
- **M6 typescript-language-server の写像**（2026-09-03）: `src/adapter/typescript_language_server.rs`。progress "Initializing JS/TS language features…" で readiness、"[tsserver] Exited. Code:" の Error ログで health `error`（言語サーバーは生き残って空配列を成功として返すので、下流側の拒否が効く）。名乗りは "Using Typescript version …" のログと `$/typescriptVersion`。保証は TypeScript 5.9.3 に宣言。実サーバーの結合テストは 19 件
- **M7 Serena 統合**（2026-09-03）: `ls_specific_settings.<言語>.ls_base_cmd` の設定だけで lsp-det を挟める（`dogfood/serena/README.ja.md`）。tsserver のクラッシュ後の references が、Serena 単体では空配列の成功応答、lsp-det 経由では理由付きのエラーになることを実測
- **上流の名乗りへの追従**（2026-09-03）: 名前の突き合わせは大文字小文字を区別せず、`serverInfo` の版で保証の根拠を置き換えるかは写像が決める。恒等写像の初期状態の問い合わせは `initialized` を流した後に送る
- **上流に出す変更の検証環境**（2026-09-03）: `scripts/upstream/`、`tests/upstream_dev.rs`。pyright / typescript-language-server の `serverInfo`、rust-analyzer / gopls のサーバー状態プロトコルのパッチを fork のブランチに用意し、受け入れ条件を通した
- **README**（2026-09-04）: 英語版 `README.md` と日本語版 `README.ja.md`
- **macOS と Windows**（2026-09-04、ADR 0012）: プロセス寿命の 2 経路を OS ごとの機構（Linux: `PR_SET_PDEATHSIG`、macOS: `kqueue` の `EVFILT_PROC`、Windows: 親ハンドル待ちと Job Object）で実装。多プロセスの結合テスト `tests/process_lifetime.rs` を 3 OS の CI で回す。`v*` タグで 5 ターゲットのバイナリを Release に添付する
- **調査**: Claude Code のドッグフーディング 3 回、Serena が MCP ツールと LSP の間で行う処理、言語サーバーが stdin の EOF で終了すること

## 0.1.0（2026-09-03）

v0.1-design.md の 4 マイルストーン。成功基準は ADR 0009（仕様・両側の準拠テスト・参照実装の自己無矛盾、rust-analyzer と gopls で通ること）。

- **M1 素通しプロキシ**（2026-08-28）: フレーミング（`src/framing.rs`）、プロセス寿命（`src/process/`）、イベントループ（`src/proxy.rs`）、CLI（`src/cli.rs`）
- **M2 上流側（rust-analyzer）**（2026-09-03）: 覗き見（`src/peek.rs`）、状態の保持（`src/tracker.rs`）、rust-analyzer の写像、capability 注入と `serverInfo` の読み取り（`src/initialize.rs`）、`experimental/serverState` / `serverStateChanged`、保証の宣言、上流側の準拠テスト（`tests/conformance.rs`、偽上流は `examples/fake_lsp_server.rs`）。ADR 0009 の追従（`dead` の削除、`serverInfo.name` による写像選択、`window/workDoneProgress/create` の自前応答、テスト済みの版の一覧、CLI の縮小）
- **M3 下流側**（2026-09-03）: `src/gate.rs`（判定表・保留キュー・キャンセル・`shutdown` と上流消失での drain）。下流側の準拠テスト `tests/client_conformance.rs`（仕様 9.1）。恒等写像のときは初期状態を自ら問い合わせる。打ち切りタイマーはない
- **M4 gopls の写像**（2026-09-03）: `src/adapter/gopls.rs`（`$/progress` の "Setting up workspace" と "Error loading workspace" からの合成）。写像は `adapter::Mapping` trait に統一。gopls v0.23.0 で 7.1 / 7.2 / 7.3 を確認
- **仕様と ADR**: サーバー状態プロトコル（`docs/spec/server-state.md`）、ADR 0001〜0009
