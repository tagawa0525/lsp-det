# `workspace/symbol` の打ち切りの実測（2026-09-04）

ADR 0013 の根拠。本プロトコルの `completeness`（当時の名前）は 7.0 の 11 メソッドに `workspace/symbol` を含めて「応答が完全である」と約束していた。Serena の調査（[serena-processing-around-lsp.md](serena-processing-around-lsp.md) 4 章）で rust-analyzer に `limit: 128` を渡していることが分かり、4 サーバーで打ち切りの有無を測った。

## 結論

| サーバー                   | 版                        | 一致する 300 個への応答 | 打ち切り                                                                     |
| -------------------------- | ------------------------- | ----------------------- | ---------------------------------------------------------------------------- |
| rust-analyzer              | 2026-08-03（nixpkgs）     | 128                     | **あり**。`workspace.symbol.search.limit` の既定 128。1000 にすると 300      |
| gopls                      | 0.23.0                    | 100                     | **あり**。`workspace_symbol.go:29` の `maxSymbols = 100`。設定で変えられない |
| pyright                    | 1.1.412                   | 300                     | なし                                                                         |
| typescript-language-server | 5.3.0（TypeScript 5.9.3） | 300                     | なし                                                                         |

打ち切ったサーバーはそれを一切伝えない。応答は普通の `result` 配列で、`isIncomplete` に当たるものは `workspace/symbol` には存在しない（LSP にあるのは completion だけ）。ログも出ない。

## 上限の理由（ソース）

- rust-analyzer `crates/rust-analyzer/src/config.rs:1068-1071`: 「VS Code のようなクライアントは結果の絞り込みのたびに検索を出し直すので、最初の検索で全結果を要らない。全結果を先に要るクライアントは上限を上げる必要があるかもしれない」
- gopls `gopls/internal/golang/workspace_symbol.go:27-29`: 「クライアントに送るべき結果の最大数」。fuzzy のスコア順に上位 100 件を固定長の配列で保持する。同ファイルの注釈に「LSP の `workspace/symbol` はサーバー側のフィルタを持たない（microsoft/language-server-protocol#941）」

どちらもエディタのピッカー向けの、スコア順のあいまい検索として作られている。列挙の契約は最初からない。

## 測定方法

各言語で、接頭辞 `wsymprobe` を共有する 300 個のトップレベルのシンボル（3 ファイル × 100）を持つ fixture を作り、サーバーを lsp-det を挟まずに直接起動して `initialize` → `initialized` → readiness の信号（rust-analyzer は `quiescent: true`、gopls は "Setting up workspace" の end、pyright は "Found 3 source files"、tsls は `didOpen` 後の "Initializing JS/TS language features" の end）を待ってから `workspace/symbol` に `{"query": "wsymprobe"}` を送り、結果のうち名前に `wsymprobe` を含むものを数えた。rust-analyzer は `initializationOptions.workspace.symbol.search.limit = 1000` でも測った。

空の query（`""`）の応答も記録した: rust-analyzer は crate のルート 6 個、gopls は `null`、pyright は `[]`、tsls は 300。

## 一般化してはならない点

- 測ったのは flake.nix が固定する版だけ。上限は版で変わりうる（rust-analyzer は設定でも変わる）
- 4 サーバー以外（jdtls、clangd 等）は未測定
- 打ち切りの有無を測っただけで、スコア順の並びが「エージェントの欲しい順」かは別の問題
