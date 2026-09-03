# 上流に出す変更をローカルで確かめる

lsp-det の次段階は上流への働きかけである（ADR 0009 決定 A-3、ADR 0010 決定 A-4、ADR 0011 決定 C）。出す前に、上流の clone に変更を当ててビルドし、lsp-det の準拠テストと受け入れ条件のテストを当てる。

| 上流                       | 変更の内容                                                                      | 受け入れ条件（`tests/upstream_dev.rs`）                  |
| -------------------------- | ------------------------------------------------------------------------------- | -------------------------------------------------------- |
| pyright                    | `InitializeResult.serverInfo` を返す（ADR 0011 決定 C）                         | `pyright_names_itself_in_server_info`                    |
| typescript-language-server | 同上                                                                            | `typescript_language_server_names_itself_in_server_info` |
| rust-analyzer              | サーバー状態プロトコルを自ら話す（仕様 3〜7 章、10 章の対応表）                 | `rust_analyzer_speaks_the_server_state_protocol`         |
| gopls                      | 同上                                                                            | `gopls_speaks_the_server_state_protocol`                 |
| Serena                     | `experimental.serverState` を宣言し、自前の readiness 判定を捨てる（M7 の観測） | `scripts/serena/probe.py`（Python。下記）                |

## 手順

1. `reference/<repo>` に変更を当てる（clone は浅い。上流に出すときは fork を別に用意する）
2. ビルドする。`nix develop`（または direnv）の中で:

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

| 上流                       | ブランチ                                                 | 内容                                                                                         | 受け入れ条件               |
| -------------------------- | -------------------------------------------------------- | -------------------------------------------------------------------------------------------- | -------------------------- |
| pyright                    | `tagawa0525/pyright` の `server-info`                    | `languageServerBase.ts` の `initialize()` に `serverInfo: {name: productName, version}`      | 通過                       |
| typescript-language-server | `tagawa0525/typescript-language-server` の `server-info` | `src/version.ts` に版の読み取りをまとめ、`initialize` の結果に `serverInfo: {name, version}` | 通過（lint・build も通る） |

lsp-det 側は、名前の大文字小文字を区別せず（pyright は "Pyright" と名乗る）、`serverInfo` の版で保証の根拠を置き換えるかを写像が決める（typescript-language-server の版は包み紙の版）ようにしてある

## Serena

Serena 側の変更（`experimental.serverState` を宣言して `experimental/serverState` と `serverStateChanged` を読む）は `reference/serena` に当て、`scripts/serena/probe.py` で lsp-det 経由の references を取って確かめる:

```bash
cd reference/serena && uv run --frozen python ../../scripts/serena/probe.py python /path/to/repo a.py 0 4
```

`VIA_LSP_DET=0` で lsp-det なし、`CRASH=1` で tsserver を落とした直後の見え方を比べられる。観測の記録は `docs/research/serena-integration-measurement.md`。

## fork とリモート

上流に出す 5 リポジトリは tagawa0525 に公開 fork してあり、`reference/` の clone は `origin` が fork、`upstream` が元のリポジトリを指す（2026-09-03）。変更は fork のブランチに push して上流へ PR を出す。

| clone                                  | origin（fork）                                     | upstream                                                           |
| -------------------------------------- | -------------------------------------------------- | ------------------------------------------------------------------ |
| `reference/pyright`                    | `github.com/tagawa0525/pyright`                    | `github.com/microsoft/pyright`                                     |
| `reference/typescript-language-server` | `github.com/tagawa0525/typescript-language-server` | `github.com/typescript-language-server/typescript-language-server` |
| `reference/rust-analyzer`              | `github.com/tagawa0525/rust-analyzer`              | `github.com/rust-lang/rust-analyzer`                               |
| `reference/golang-tools`               | `github.com/tagawa0525/tools`                      | `github.com/golang/tools`                                          |
| `reference/serena`                     | `github.com/tagawa0525/serena`                     | `github.com/oraios/serena`                                         |

clone は浅い（`--depth 1`）。ブランチを切って push するぶんには足りるが、履歴が要るときは `git fetch --unshallow upstream`。

## 注意

- `reference/` は git 追跡外の浅い clone。変更は上流に出すまでの作業場で、lsp-det のリポジトリには入れない
- 上流の版は `TESTED_VERSIONS` に載せない。載せるのは flake.nix が固定する配布版で 7.2 / 7.3 を通したものだけ
