# 言語サーバーは stdin の EOF で終了するか（2026-09-04）

ADR 0012 決定 B の根拠。macOS には、lsp-det が `SIGKILL` 等で不意に死んだときに上流の言語サーバーを道連れにする機構（Linux の `PR_SET_PDEATHSIG`、Windows の Job Object に当たるもの）がない。lsp-det と共に上流の stdin の書き込み側が消えるので、上流が stdin の EOF で終了するなら孤児は残らない。4 つの言語サーバーがそうするかを測った。

## 結論

4 つとも、`initialize` に答えた後に stdin が閉じると、`shutdown` / `exit` を待たずに終了する。EOF から終了までは 4 件合わせて 0.13 秒以内（`cargo test` の計測）。

| 言語サーバー               | 版                        | EOF で終了するか | 終了コード |
| -------------------------- | ------------------------- | ---------------- | ---------- |
| rust-analyzer              | 2026-08-03（nixpkgs）     | する             | 1          |
| gopls                      | 0.23.0                    | する             | 0          |
| pyright-langserver         | 1.1.412                   | する             | 1          |
| typescript-language-server | 5.3.0（TypeScript 5.9.3） | する             | 1          |

終了コードが 1 のサーバーは、`shutdown` を経ない EOF を異常終了として数えている。lsp-det の観点では終了することだけが要件で、コードは問わない。

## 測定方法

`tests/process_lifetime.rs` の `real_*_exits_on_stdin_eof`（`#[ignore]`、ローカル専用）。各サーバーを lsp-det を挟まずに直接起動し、`initialize` を送って応答を受け、`initialized` を送ってから stdin を閉じる。stdout は捨て続ける（パイプが詰まって終了できない状態を作らない）。30 秒以内に終了すれば通る。

```bash
cargo test --test process_lifetime -- --ignored --nocapture
```

応答を待ってから閉じるのは、起動に失敗して即終了したものを「EOF で終了した」と数えないため。

## 一般化してはならない点

- 測ったのは flake.nix が固定する版だけ。別の版や別の言語サーバー（jdtls、clangd 等）は未測定
- fixture は 2 ファイルで、インデックスの途中で EOF が来る場面は測っていない。rust-analyzer はメインループが stdin の終端で抜けるので、途中でも同じはずだが実測ではない
- Linux で測った。EOF の伝わり方は OS に依らないが、macOS での実測は CI（`tests/process_lifetime.rs` の非 ignore の 2 件は偽上流）が偽上流でしか行っていない
