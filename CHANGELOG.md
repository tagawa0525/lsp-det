# Changelog

マイルストーンの完了と、その決定の出所（ADR）を版ごとに記す。設計判断の経緯は `docs/adr/`、実測は `docs/research/`。

## 予定（0.3.0）

2026-09-04 に決めた次のバッチ。ADR 3 本を先に書き（仕様と設計の変更を含めて）マージしてから、実装に入る。

- ADR 0013（仕様）: `completeness` を `coverage` に改名し、定義を「応答はワークスペース全体のインデックスに基づく。インデックスの進行によって後から結果が増えることはない」に絞る。`workspace/symbol` は保証の対象から外す（rust-analyzer は 128 件、gopls は 100 件で黙って打ち切る。ピッカー向けのあいまい検索であり列挙の契約を持たない）。保留の対象には残す
- ADR 0014（仕様）: `freshness` の対象に、クライアントから受信した `workspace/didChangeWatchedFiles` を加える。準拠テスト 7.3 に第 2 のテスト（ディスク上の変更と新規ファイル）を足す
- ADR 0015（設計 4.3）: 下流側の代行 2 つ。capability `workspace.didChangeWatchedFiles` を宣言せず、通知 `workspace/didChangeWatchedFiles` も送らないクライアントに代わって、保留の対象のリクエストごとに `git ls-files` の一覧の mtime を比べて通知を送る（写像は関与せず、言語ごとの一覧は持たない。git 管理外では代行しない）。既に開いている uri への `didOpen` を全文の `didChange` に書き換える
- 外向きの提出は `docs/upstream-submissions.md` の順で、詰めてから

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
