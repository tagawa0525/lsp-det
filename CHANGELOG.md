# Changelog

マイルストーンの完了と、その決定の出所（ADR）を版ごとに記す。設計判断の経緯は `docs/adr/`、実測は `docs/research/`。

## 予定

- 0.4.0（ADR 0019、M8〜M20）: コーパス（済み）と devShell の分割（済み）、反例の実サーバーでの検証 10（Metals、Elixir、Kotlin、Nextflow、haskell-language-server、pyrefly、crystalline、sourcekit-lsp、Gleam、Haxe）、相方サーバーとの合成の測定（Vue）
- 0.5.0: Dart、Sorbet、jdtls、clangd（着手時に ADR）
- 外向きの提出は 0.5.0 の後、`docs/upstream-submissions.md` の順で。typescript-language-server の不具合修正 PR、Claude Code への報告（既存 issue 3 件へのコメントと新規 2 件）、Serena の不具合と提案、fork の 4 パッチ

## 未リリース

外部レビュー（ADR 0018、2026-09-06）への対応。保留の開始と解放を理由付きで stderr に出す（決定 A-1、PR #49）。ドッグフーディング第 6 回で実害の一事例を記録（A-2）。仕様 10 章に Dart / Sorbet の行と、gopls #1200 の一次資料（A-3、A-4、PR #50）。提出メモに「上流は未試行」の前提（A-6、PR #47）。

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
