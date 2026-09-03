# lsp-det

[English](README.en.md)

言語サーバーの「無言の嘘」を消す、サーバー状態プロトコルの参照実装。

LSP には、サーバーが要求に完全に答えられる状態かをクライアントが機械的に知る手段がない。その結果、インデックス未完了の空配列・壊れたサーバーの成功風の応答・編集を織り込まない結果を、クライアントは正当な答えとして受け取る。エディタでは人間の目とタイミング感覚がこれを補っていた。コーディングエージェントは補わない。`initialize` の直後に `textDocument/references` を投げ、空配列を「参照なし」と読み、そのままリネームや削除を実行する。

lsp-det は 2 つのものからなる。

- **サーバー状態プロトコル**（[docs/spec/server-state.md](docs/spec/server-state.md)）: サーバーの状態を `health` と `readiness` の 2 軸で表す語彙と、その状態のもとで応答の完全性と鮮度を保証する capability。最終目標は LSP 本体への提案
- **透過プロキシ `lsp-det`**（Rust、単一バイナリ）: クライアントと言語サーバーの間に挟まり、上記のプロトコルを両側に提供する。言語サーバーの語彙をプロトコルに写し、プロトコルを話さないクライアントに代わって横断リクエストを `ready` まで保留する

## 何が起きるか

Claude Code に rust-analyzer を lsp-det 経由で使わせた実測（[docs/research/claude-code-dogfooding.md](docs/research/claude-code-dogfooding.md)）:

| 場面                                                     | lsp-det なし                               | lsp-det あり                                                                |
| -------------------------------------------------------- | ------------------------------------------ | --------------------------------------------------------------------------- |
| `initialize` 応答の 6ms 後に `references`                | インデックス中の空配列を成功として受け取る | `ready` まで保留し、完全な結果（2 ファイル 6 箇所）を返す                   |
| Rust 1935 ファイルのワークスペースで 80 秒のインデックス | 同上                                       | 82 秒保留してから完全な結果。クライアント側のタイムアウトには掛からなかった |
| tsserver がクラッシュし言語サーバーだけ生き残る          | 以後の `references` が空配列の成功応答     | `health: error` として理由付きのエラーを返す                                |

保留に時間の上限はない。「一定時間で `ready` とみなす」合成は、消すはずの嘘を作るので持たない（仕様 6 章 6 項）。

## プロトコルの要点

```typescript
interface ServerState {
  health: "ok" | "warning" | "error";               // 機能しているか
  readiness: "initializing" | "indexing" | "ready"; // インデックスが完了しているか
  message?: string;                                 // 人間向け。機械判定に使わない
}
```

| 名前                                | 種別               | 内容                                                                               |
| ----------------------------------- | ------------------ | ---------------------------------------------------------------------------------- |
| `experimental/serverState`          | リクエスト         | 問い合わせ時点の `ServerState` を即答する                                          |
| `experimental/serverStateChanged`   | 通知               | `health` か `readiness` が変わるたびに送る（クライアントが購読を宣言したときのみ） |
| `serverStateProvider` (server cap.) | `InitializeResult` | `boolean` または `{completeness?, freshness?}`                                     |
| `serverState` (client cap.)         | `InitializeParams` | 通知の購読と「待つか進むかは自分で判断する」という意思表示                         |

保証は `ready` かつ `health` が `error` でないときに効く。`completeness` はワークスペース横断メソッド（`references` / `definition` / `implementation` / `workspace/symbol` / `rename` / call hierarchy 等の 11 個）の応答が完全であること、`freshness` は受信済みの `didChange` をすべて織り込んでいること。両者は独立で、実装は守れる保証だけを宣言する。

プロトコルの中に `dead` はない。プロセスの消失は接続の終了として伝わり、生き残った中継層が成功風の応答を返す状態は `health: "error"` で表す。中継層など外から観測する主体は、観測できない軸に `unknown` を使い、観測なしに `ok` や `ready` を名乗らない（8 章）。

既存の語彙との対応（rust-analyzer の `experimental/serverStatus`、gopls の `$/progress`、pyright の起動ログ、typescript-language-server の progress とクラッシュログ、jdtls の `language/status`、clangd の無信号）は仕様 10 章。

## プロキシの動作

```text
クライアント ──[素の LSP]── 下流側 ──[LSP + サーバー状態プロトコル]── 上流側 ──[素の LSP]── 言語サーバー
                            (代行)        lsp-det 内部の境界             (写像)
```

**上流側**は言語サーバーを代行する。上流が `InitializeResult.serverInfo`（なければ起動時のログ）で名乗る名前で写像を選び、上流の語彙を `ServerState` に写し、`serverStateProvider` を `InitializeResult` に足す。保証は準拠テスト 7.2 / 7.3 を通した版にだけ宣言する。上流が自らプロトコルを話していれば何も足さず、そのまま通す。

| 言語サーバー               | readiness の信号                                             | health の信号                         | 保証を宣言する版                     |
| -------------------------- | ------------------------------------------------------------ | ------------------------------------- | ------------------------------------ |
| rust-analyzer              | `experimental/serverStatus` の `quiescent`                   | 同 `health`                           | 1.98.0、2026-08-03                   |
| gopls                      | `$/progress` "Setting up workspace" の終了                   | "Error loading workspace" の progress | 0.23.0                               |
| pyright / basedpyright     | `window/logMessage` のファイル列挙完了（フォルダ数ぶん待つ） | なし（`unknown`）                     | pyright 1.1.412、basedpyright 1.39.8 |
| typescript-language-server | `$/progress` "Initializing JS/TS language features…"         | "[tsserver] Exited" のログ → `error`  | TypeScript 5.9.3                     |
| その他                     | なし（両軸 `unknown`）                                       |                                       | 宣言しない                           |

**下流側**はクライアントを代行する。クライアントが `experimental.serverState` を宣言していれば状態を転送するだけで待たない。宣言していなければ、仕様 9 章の推奨挙動を代わりに実行する。

| `health` \ `readiness`       | `initializing` / `indexing` | `ready`      | `unknown`    |
| ---------------------------- | --------------------------- | ------------ | ------------ |
| `ok` / `warning` / `unknown` | 横断リクエストを保留        | 転送         | 転送         |
| `error`                      | 即座にエラー                | 即座にエラー | 即座にエラー |

通知・単一ファイルの問い合わせ（hover / completion / documentSymbol 等）・ライフサイクル・サーバーからクライアントへの方向はすべて素通しする。保留中に `$/cancelRequest` や `shutdown` を受けたら、保留分にエラーを応答してから流す。応答を返さない要求は作らない。

メッセージのボディは原文バイトのまま転送する。写像に要る通知と `initialize` の往復だけをパースする。

## 使い方

```text
lsp-det -- <言語サーバーの起動コマンド> [args...]
```

フラグはない。写像の選択は上流の名乗りで決まり、時間の非常口も保留の切り替えもない。起動指定はクライアント側の設定に常在させる。

Claude Code のプラグイン（`.lsp.json`）:

```json
{
  "rust-analyzer-via-lsp-det": {
    "command": "lsp-det",
    "args": ["--", "rust-analyzer"],
    "extensionToLanguage": { ".rs": "rust" }
  },
  "pyright-via-lsp-det": {
    "command": "lsp-det",
    "args": ["--", "pyright-langserver", "--stdio"],
    "extensionToLanguage": { ".py": "python", ".pyi": "python" }
  }
}
```

4 サーバーぶんの実物は [dogfood/claude-plugin/.lsp.json](dogfood/claude-plugin/.lsp.json)、手順は [dogfood/README.md](dogfood/README.md)。Serena は `.serena/project.yml` の `ls_specific_settings.<言語>.ls_base_cmd` に同じコマンドを書く（[dogfood/serena/README.md](dogfood/serena/README.md)）。

lsp-det は stderr に写像の選択と状態遷移を出す。

```text
lsp-det: upstream is "rust-analyzer" version "2026-08-03"; using its mapping, declaring {"completeness":true,"freshness":true}
lsp-det: [0.041s] server state -> {"health":"unknown","readiness":"initializing"} (previous held 0.041s)
lsp-det: [0.213s] server state -> {"health":"ok","readiness":"indexing"} (previous held 0.172s)
lsp-det: [6.712s] server state -> {"health":"ok","readiness":"ready"} (previous held 6.499s)
```

対応 OS は Linux のみ。プロセス寿命の管理が `PR_SET_PDEATHSIG` に依存する。

## ビルドとテスト

依存は `serde` / `serde_json` / `thiserror` / `libc` だけで、非同期ランタイムは使わない。

```bash
nix develop            # rustc、rust-analyzer、gopls、pyright、typescript-language-server を固定する
cargo build --release  # target/release/lsp-det
cargo test             # 偽の言語サーバー・偽のクライアントによる決定的なテスト
cargo test --test conformance -- --ignored   # 実サーバー結合 19 件（ローカルのみ、CI では回さない）
```

テストは仕様をそのまま実行可能にしたもので、被験者を差し替えれば実サーバー・実クライアントにも当たる。

| テスト                        | 仕様の章     | 被験者                                                                                               |
| ----------------------------- | ------------ | ---------------------------------------------------------------------------------------------------- |
| `tests/conformance.rs`        | 7 章、8.4    | lsp-det の上流側。偽の上流（`examples/fake_lsp_server.rs`）と実サーバー 4 種                         |
| `tests/client_conformance.rs` | 9.1          | lsp-det の下流側。準拠した偽の上流と rust-analyzer を名乗る偽の上流                                  |
| `tests/upstream_dev.rs`       | 上流への変更 | 上流の fork に当てたパッチの受け入れ条件（[scripts/upstream/README.md](scripts/upstream/README.md)） |

## 上流への働きかけ

プロキシは一時的な置き場である。言語サーバーが自らプロトコルを話せば上流側の写像は恒等になり、クライアントが自ら状態を読めば下流側の代行は止まる。準拠する実装が増えるほど lsp-det は薄くなる。

そのための変更を上流の fork に用意し、ローカルで受け入れ条件を通してある（上流への提出は未着手）。

| 上流                       | 変更                                                                          |
| -------------------------- | ----------------------------------------------------------------------------- |
| pyright                    | `InitializeResult.serverInfo` を返す                                          |
| typescript-language-server | 同上                                                                          |
| rust-analyzer              | `experimental/serverStatus` の後継としてプロトコルを話す                      |
| gopls                      | プロトコルを話す（フォルダごとの初期ロードとロード失敗）                      |
| Serena                     | `experimental.serverState` を読み、自前の readiness 判定（約 285 行）を捨てる |

## 文書

| 文書                                                   | 内容                                                                                                                |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------- |
| [docs/spec/server-state.md](docs/spec/server-state.md) | サーバー状態プロトコルの規範。他文書と食い違えばこれが正                                                            |
| [docs/v0.1-design.md](docs/v0.1-design.md)             | プロキシの実装スコープ（上流側・下流側・写像・実行モデル）                                                          |
| [docs/adr/README.md](docs/adr/README.md)               | 設計判断の索引。生きている決定と却下した案                                                                          |
| [docs/vision.md](docs/vision.md)                       | 長期構想（宣言範囲・起動方法の宣言は凍結中）                                                                        |
| [docs/research/](docs/research/)                       | 調査と実測 18 本。各言語サーバーの readiness の実態、先行プロキシ、Serena / Claude Code / Zed / VS Code の LSP 統合 |

## 現在地

v0.1（rust-analyzer と gopls）と v0.2（pyright、typescript-language-server、Serena 統合）は完了している。次は上流への提出と、仕様の英訳。
