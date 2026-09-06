# Changelog

マイルストーンの完了と、その決定の出所（ADR）を版ごとに記す。設計判断の経緯は `docs/adr/`、実測は `docs/research/`。

## 予定

- 外向きの提出（`docs/upstream-submissions.md` の順。文面を作ってユーザーの確認をもらってから出す）: typescript-language-server の不具合修正 PR、Claude Code への報告（既存 issue 3 件へのコメントと新規 2 件）、Serena の不具合と提案、fork の 4 パッチ、0.4.0 と 0.5.0 で見つけた 10 サーバー分の提案
- 保留の再測定: Kotlin（次の release）、sourcekit-lsp（nixpkgs に 6.x が来たら）

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
- **M7 Serena 統合**（2026-09-03）: `ls_specific_settings.<言語>.ls_base_cmd` の設定だけで lsp-det を挟める（`dogfood/serena/README.md`）。tsserver のクラッシュ後の references が、Serena 単体では空配列の成功応答、lsp-det 経由では理由付きのエラーになることを実測
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
