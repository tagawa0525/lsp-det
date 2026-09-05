# ADR 0017: 仕様とコードは英語、決定と調査の記録は日本語にする

- 日付: 2026-09-06
- 状態: 採用
- 関連: [ADR 0010](0010-python-typescript-mappings-for-client-adoption.md) 決定 A-4（上流 issue の前提として README と仕様の英訳）、[ADR 0009](0009-success-criterion-and-two-sided-reference.md) 決定 B（造語を避け LSP の語彙に合わせる）、[docs/upstream-submissions.md](../upstream-submissions.md)、[docs/glossary.md](../glossary.md)

## 経緯

文書もコードのコメントも日本語で書いてきた。README だけは 2026-09-05 に英語を正（`README.md`）、日本語を訳（`README.ja.md`）にした。0.3.0 が完了し、次が外向きの提出（rust-analyzer と gopls への PR、Claude Code と Serena への報告）なので、相手が読める形にする範囲を決める必要がある。

読み手で分けると 3 種類ある。

1. **上流のレビュアーとクライアントの実装者**: 仕様（`docs/spec/server-state.md`）と README を読む。上流の PR は仕様を参照するので、仕様が日本語のままでは提案にならない
2. **参照実装を読む人**（Serena、Claude Code、採用を検討する人）: `src` / `tests` / `examples` を読む。コメントが日本語だと、なぜそう書いてあるかが伝わらない。lsp-det は参照実装が目的なので、読めない注釈は価値が半減する
3. **保守者**（作者と Claude Code）: ADR・research・設計・vision を読む。合わせて 5,200 行あり、決定の経緯と実測の記録である。外部の読み手は仕様と README で足りる

作者は英語の細部を精査できない。正を英語にする文書では、日本語版を同じコミットで追従させて、レビューは日本語版で行う。

## 決定

### A. 対象ごとの言語

| 対象                                                           | 言語                                            | 理由                                                           |
| -------------------------------------------------------------- | ----------------------------------------------- | -------------------------------------------------------------- |
| `README.md` / `README.ja.md`                                   | 英語が正、日本語が訳（2026-09-05 の決定を追認） | 最初に読まれる                                                 |
| `docs/spec/server-state.md` / `docs/spec/server-state.ja.md`   | 英語が正、日本語が訳                            | 上流の PR と LSP 本体への提案が参照する規範。正は 1 本に決める |
| `src` / `tests` / `examples` のコメントとテスト名              | 英語                                            | 参照実装の注釈。今後書くものも英語にし、混在を固定化しない     |
| 実行時のメッセージ（stderr のログ、エラー）                    | 英語（すでにそうなっている）                    | 利用者が読む                                                   |
| `dogfood/README.md`                                            | 英語                                            | 利用者向けの手順                                               |
| ADR・`docs/research/`・`docs/v0.1-design.md`・`docs/vision.md` | 日本語                                          | 保守者の記録。必要になった ADR だけ個別に訳す                  |
| `scripts/*/README.md`・`dogfood/serena/README.md`・`CLAUDE.md` | 日本語                                          | 保守者の手順                                                   |
| コミットメッセージ・PR 本文・CHANGELOG                         | 日本語                                          | 保守者が読む。フックの形式（`## 変更点` など）もそのまま       |

### B. 正と訳の追従

英語が正の文書（README、仕様）に手を入れるときは、日本語版を**同じコミット**で追従させる。日本語版の見出しの構成は英語版と 1 対 1 に保つ。レビューは日本語版で行い、両者の同値は書き手（Claude Code）が保証する。

### C. ディレクトリは動かさない

`docs/` を `docs.ja/` に改名して `docs/` を英語にする案は採らない。ADR と research が日本語のままなので名前と中身が食い違い、40 本の文書と CLAUDE.md と fork の説明文が相対パスで互いを参照しているので移動で切れる。README と同じく、同じディレクトリに `.ja.md` を並べる。

### D. 訳語は対訳表で固定する

日本語と英語の対応は [docs/glossary.md](../glossary.md) に置く。仕様・README・コードのコメントはこの表の訳語を使う。訳語を変えるときは表を直してから全体を直す。

### E. 進める順序

1. 本 ADR と対訳表、CLAUDE.md の規則
2. 仕様の英訳（`server-state.md` を英語に、現行を `server-state.ja.md` に）
3. `src` / `tests` / `examples` のコメントの英訳（1 つの PR で機械的に）
4. `dogfood/README.md` の英訳
5. Claude Code でのドッグフーディング第 5 回（ADR 0015 の代行 2 つを実物で確かめる）
6. 外向きの提出（`docs/upstream-submissions.md` の順）

### 却下した案

| 案                                            | 却下理由                                                                                                 |
| --------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| すべての文書を英訳する                        | ADR と research は保守者の記録で 5,200 行ある。訳を保つ費用に対して外部の読み手がいない                  |
| 仕様は日本語を正のままにし、英語版を訳にする  | 上流の PR が参照するのは英語版で、「食い違いはここが正」の文書が相手に読めないのは提案として成り立たない |
| `docs/` を `docs.ja/` にして `docs/` を英語に | 決定 C                                                                                                   |
| コードのコメントは日本語のまま                | 参照実装を読む人に理由が伝わらない。今後の混在も避けたい                                                 |
| コミットメッセージと PR 本文も英語にする      | 上流は読まない。保守者が読み、フックの形式も日本語で固まっている                                         |

## 影響

- `CLAUDE.md` に言語の規則（本 ADR の A・B・D）を足す
- `docs/glossary.md` を新設する
- `README.md` の「Documents other than this README are written in Japanese」と仕様の「Japanese; an English translation is planned」は、仕様の英訳の PR で直す
