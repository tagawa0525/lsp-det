# 上流に出す変更をローカルで確かめる

lsp-det の次段階は上流への働きかけである（ADR 0009 決定 A-3、ADR 0010 決定 A-4、ADR 0011 決定 C）。出す前に、上流の clone に変更を当ててビルドし、lsp-det の準拠テストと受け入れ条件のテストを当てる。

| 上流                                     | 変更の内容                                                                                                                                                   | 受け入れ条件（`tests/upstream_dev.rs`）                                                                |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------ |
| pyright                                  | `InitializeResult.serverInfo` を返す（ADR 0011 決定 C）                                                                                                      | `pyright_names_itself_in_server_info`                                                                  |
| typescript-language-server               | 同上                                                                                                                                                         | `typescript_language_server_names_itself_in_server_info`                                               |
| typescript-language-server               | 非 0 の exit code のときと同じく、signal で殺された tsserver でも終了する（上流 #302 / #305）                                                                | `typescript_language_server_exits_when_tsserver_is_killed`                                             |
| rust-analyzer                            | サーバー状態プロトコルを自ら話す（仕様 3〜7 章、10 章の対応表）                                                                                              | `rust_analyzer_speaks_the_server_state_protocol`                                                       |
| rust-analyzer                            | （別案）`experimental/serverStatus` に `readiness` の field を足す。写像は field があればそれを読む                                                          | `rust_analyzer_reports_readiness_in_server_status`                                                     |
| gopls                                    | 同上                                                                                                                                                         | `gopls_speaks_the_server_state_protocol`                                                               |
| Serena                                   | tsserver のクラッシュを 2 回目以降の横断要求でも例外にする（M7 の観測の (a)）。registry（oraios/serena#1988）に lsp-det を載せる提案は issue で              | `scripts/serena/probe.py` の `CRASH=1 VIA_LSP_DET=0` がコード 0 で終わること（下記）                   |
| LSP 本体（`vscode-languageserver-node`） | `proposed.serverState.ts` と同名の `.md`（`workspace/serverState`、`workspace/serverStateChanged`、`serverStateProvider`。メタモデルに proposed として載る） | なし。`npm run compile:protocol`、`protocol` の `lint` / `test:node` / `generate:metaModel` が通ること |

## 手順

1. `reference/<repo>` に変更を当てる（clone は浅い。上流に出すときは fork を別に用意する）
2. ビルドする。`nix develop .#servers`（または direnv）の中で（pnpm と node はそこにある）:

   ```bash
   scripts/upstream/build-pyright.sh
   scripts/upstream/build-typescript-language-server.sh
   scripts/upstream/build-rust-analyzer.sh   # rustup の stable を使う（上流の rust-version が flake の rustc より新しい）
   scripts/upstream/build-gopls.sh
   ```

   起動子が `target/upstream/bin/` に置かれる（`git` 追跡外）。
3. PATH の先頭に置いて受け入れ条件を回す:

   ```bash
   PATH="$PWD/target/upstream/bin:$PATH" cargo test --test upstream_dev -- --ignored
   ```

   変更を当てる前は**失敗するのが正しい**。通ったら上流に出す。
4. 既存の準拠テストも同じ PATH で回し、退行がないことを見る:

   ```bash
   PATH="$PWD/target/upstream/bin:$PATH" cargo test --test conformance -- --ignored
   ```

   ソースビルドは配布版と違う版を名乗る（rust-analyzer は `0.0.0 (<sha> <date>)`、gopls は `v0.0.0-<date>-<sha>`、pyright は clone の版）。`TESTED_VERSIONS` にないので保証は宣言されず、「測った版に保証を宣言する」テストは失敗する。これは想定どおりで、それ以外（7.1 / 7.2 / 7.3、拒否、再発行）が通ればよい。また `serverInfo` を足す変更を当てたビルドでは、現状の上流を記録した「前提が崩れている。… が serverInfo を返すようになった」の断言が失敗する。これも変更が効いている印で、上流に取り込まれたら準拠テスト側の前提を書き換える

## 用意してある変更（fork のブランチ）

| 上流                       | ブランチ                                                                                                                                            | 内容                                                                                                                                                                                                                                                                                                                                                                                                     | 受け入れ条件                                                                                                                                                                                          |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| pyright                    | `tagawa0525/pyright` の `server-info`                                                                                                               | `languageServerBase.ts` の `initialize()` に `serverInfo: {name: productName, version}`                                                                                                                                                                                                                                                                                                                  | 通過                                                                                                                                                                                                  |
| typescript-language-server | `tagawa0525/typescript-language-server` の `server-info`                                                                                            | `src/version.ts` に版の読み取りをまとめ、`initialize` の結果に `serverInfo: {name, version}`                                                                                                                                                                                                                                                                                                             | 通過（lint・build も通る）                                                                                                                                                                            |
| typescript-language-server | `tagawa0525/typescript-language-server` の `tsserver-exit-by-signal`（上流 master 起点。`server-info` とは独立）                                    | `lsp-server.ts` の `onExit` から `if (exitCode)` の条件を外し、signal で殺された tsserver（`exitCode: null`）でもサーバーを止める。#585 以来クライアントは tsserver を殺す前に exit handler を消すので、自分の shutdown は `onExit` に届かない。`ts-client.test.ts` に 2 件。却下した `no-server-request-failed`（`RequestFailed` の経路。`docs/upstream-submissions.md`）と入れ替え                     | 通過（`typescript_language_server_exits_when_tsserver_is_killed`。素の 6.0.0 は生き残って `[]` を返す）。vitest 141 件と lint・typecheck も通る。fork の CI（3 OS × Node 22 / 24）は fork の PR #1 で |
| rust-analyzer              | `tagawa0525/rust-analyzer` の `server-state`                                                                                                        | `experimental/serverState` 一式（`lsp/ext.rs`、`reload.rs` の `current_server_state`、`main_loop.rs` の通知、capability、`lsp-extensions.md` と hash）。ワークスペース未発見は `error`                                                                                                                                                                                                                   | 通過（`cargo xtask tidy`、lib tests 99 件も通る）。恒等写像で 7.1 / 7.2 / 7.3 も通る                                                                                                                  |
| rust-analyzer              | `tagawa0525/rust-analyzer` の `server-status-readiness`（`server-state` と同じ起点。issue に両案を並べ、相手が選んだ方だけを出す）                  | `ServerStatusParams` に `readiness`（`initializing` / `indexing` / `ready`。`reload.rs` の `current_readiness`）、初期の `last_reported_status` は `initializing`（通知の回数は変わらない）、`lsp-extensions.md` の field と「この field は人向けでなく答えを信じてよいかを決めるクライアント向け」の注記と hash、`editors/code` の型                                                                    | 通過（`cargo xtask tidy`、lib tests 99 件も通る）。写像が field を読み、7.1 / 7.2 / 7.3 も通る。実測は `scripts/rust-analyzer/status-probe.py`                                                        |
| LSP 本体                   | `tagawa0525/vscode-languageserver-node` の `server-state`                                                                                           | `protocol/src/common/proposed.serverState.ts`（型、要求、通知、`$ServerStateClientCapabilities` / `$ServerStateServerCapabilities`）、`proposed.serverState.md`（仕様の本文）、`api.ts` の `Proposed`、再生成した `metaModel.json`。依存は root で `npm ci --ignore-scripts && node ./build/bin/all.js install && npm run symlink`（root の `npm ci` は postinstall で playwright まで入れるので避ける） | 通過（`compile:protocol`、`lint`、`test:node` 5 件、`generate:metaModel` は追加だけの差分）                                                                                                           |
| Serena                     | `tagawa0525/serena` の `tsserver-crash-on-request-path`（上流 main 起点。oraios/serena#1978 と同じ関数を触るので、提出はその帰趨を見てから rebase） | `_wait_for_cross_file_references_if_needed` の冒頭、latch の前で `_raise_if_crashed()` を呼ぶ。latch 後にクラッシュを観測した状態のテスト 1 件、CHANGELOG の 1 項目                                                                                                                                                                                                                                      | 通過（probe がコード 0。素の HEAD では 1。`test_typescript_timeout_policy.py` 30 件、`test/solidlsp/typescript` 20 件、ruff と ty も通る。#1978 の上でも同じ）                                        |
| gopls                      | `tagawa0525/tools` の `server-state`                                                                                                                | `server_state.go`（フォルダごとの初期ロードで `indexing` → `ready`、ロード失敗と "Error loading workspace" で `error`）、`protocol.go` に `ServerStateProvider` フック（生成コードの外で `experimental/serverState` を配送）と通知                                                                                                                                                                       | 通過（`go test ./internal/server ./internal/protocol` も通る）。恒等写像で 7.2 / 7.3 も通る                                                                                                           |

本プロトコルを話す上流（rust-analyzer / gopls のパッチ版）では、lsp-det は恒等写像になり上流の通知をそのまま流す。準拠テストの次の断言は、観測者としての写像の前提なのでパッチ版では失敗するのが正しい:

- `gopls_spec_7_1_through_lsp_det_with_real_gopls` の「`initialize` 直後は `ready` でない」: 小さな fixture では上流が正直に `ready` を答える（仕様 7.1 の 1 は ADR 0009 決定 C-5 で緩めてある）
- `gopls_does_not_reemit_workspace_setup_on_go_mod_change`: 上流の初期ロードの通知（`indexing` → `ready`）が `wait_until_ready` の後にも受信待ちに残っていて拾われる。go.mod の変更で上流が再発行しているのではない（gopls は go.mod 変更後のリロードをリクエストの中で同期的に行うので `ready` のまま正しい）

typescript-language-server の `tsserver-exit-by-signal` のビルドでは、準拠テストの 2 つの断言が失敗し、それが正しい: `typescript_language_server_tsserver_crash_becomes_health_error_with_real_server`（言語サーバーが tsserver と一緒に終了するので lsp-det も終了し、テストの書き込みが Broken pipe で失敗する。嘘をつくのではなく上流が消える。仕様 8 章）と `typescript_language_server_spec_7_1_through_lsp_det_with_real_server`（ソースビルドが同梱する TypeScript が `TESTED_VERSIONS` にないので保証が宣言されない）。

lsp-det 側は、名前の大文字小文字を区別せず（pyright は "Pyright" と名乗る）、`serverInfo` の版で保証の根拠を置き換えるかを写像が決める（typescript-language-server の版は包み紙の版）ようにしてある。rust-analyzer のパッチに当てたことで、恒等写像の初期状態の問い合わせが `initialized` より前だった不具合も見つかり直した（PR #26）

上流に出す変更を当てる clone（下の「fork とリモート」の表の 6 つ）は `origin` が fork、`upstream` が本家で、グローバルの git hook は `upstream` リモートのある clone ではこちらの規約の検査（Conventional Commits の件名、markdownlint の自動修正、ruff、rustfmt）を飛ばす。上流の規約は上流の固定版の道具と CI が見る。`reference/` の他の clone（`reference/README.md` の浅い clone）は `origin` だけで、コミットしない

## Serena

Serena 側の変更は `reference/serena`（`uv sync --frozen --all-groups --all-extras` で dev 依存まで入れる）に当て、`scripts/serena/probe.py` で references を取って確かめる。fork の `tsserver-crash-on-request-path` の受け入れ条件は、`CRASH=1 VIA_LSP_DET=0` がコード 0 で終わること（tsserver を落とした直後の references が `TypeScriptServerCrashedError` になり、"references after crash raised TypeScriptServerCrashedError" の行が出る）。素の上流は 0 件を成功として返し、probe は "NO ERROR SURFACED" を出してコード 1 で終わる。lsp-det 経由の比較にも同じ道具を使う:

```bash
# 受け入れ条件（コード 0 で終わること。a.ts の 0 行 16 桁は export された関数名）
cd reference/serena && CRASH=1 VIA_LSP_DET=0 uv run --frozen python ../../scripts/serena/probe.py typescript /path/to/repo a.ts 0 16
# lsp-det 経由の比較
cd reference/serena && uv run --frozen python ../../scripts/serena/probe.py python /path/to/repo a.py 0 4
```

`VIA_LSP_DET=0` で lsp-det なし、`CRASH=1` で tsserver を落とした直後の見え方を比べられる。観測の記録は `docs/research/serena-integration-measurement.md`。

## fork とリモート

上流に出す 6 リポジトリは tagawa0525 に公開 fork してあり、`reference/` の clone は `origin` が fork、`upstream` が元のリポジトリを指す（2026-09-03。`vscode-languageserver-node` は 2026-09-09）。変更は fork のブランチに push して上流へ PR を出す。

| clone                                  | origin（fork）                                     | upstream                                                           |
| -------------------------------------- | -------------------------------------------------- | ------------------------------------------------------------------ |
| `reference/pyright`                    | `github.com/tagawa0525/pyright`                    | `github.com/microsoft/pyright`                                     |
| `reference/typescript-language-server` | `github.com/tagawa0525/typescript-language-server` | `github.com/typescript-language-server/typescript-language-server` |
| `reference/rust-analyzer`              | `github.com/tagawa0525/rust-analyzer`              | `github.com/rust-lang/rust-analyzer`                               |
| `reference/golang-tools`               | `github.com/tagawa0525/tools`                      | `github.com/golang/tools`                                          |
| `reference/serena`                     | `github.com/tagawa0525/serena`                     | `github.com/oraios/serena`                                         |
| `reference/vscode-languageserver-node` | `github.com/tagawa0525/vscode-languageserver-node` | `github.com/microsoft/vscode-languageserver-node`                  |

clone は浅い（`--depth 1`）。ブランチを切って push するぶんには足りるが、履歴が要るときは `git fetch --unshallow upstream`。

## 注意

- `reference/` は git 追跡外の浅い clone。変更は上流に出すまでの作業場で、lsp-det のリポジトリには入れない
- 上流の版は `TESTED_VERSIONS` に載せない。載せるのは flake.nix が固定する配布版で 7.2 / 7.3 を通したものだけ
