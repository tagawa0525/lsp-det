# ADR 0013: `completeness` を `coverage` に改名し、`workspace/symbol` を保証の対象から外す

- 日付: 2026-09-04
- 状態: 採用
- 関連: [ADR 0004](0004-spec-grilling.md) 決定 3（名前を置き換える）、[ADR 0009](0009-success-criterion-and-two-sided-reference.md) 決定 B（造語を避け内容そのものの名前で呼ぶ）、[research/workspace-symbol-truncation-measurement.md](../research/workspace-symbol-truncation-measurement.md)、[research/serena-processing-around-lsp.md](../research/serena-processing-around-lsp.md) 4 章

## 経緯

仕様 5 章の `completeness` は「`ready` かつ `health` が `error` でないとき、7.0 に列挙する 11 のワークスペース横断メソッドの応答が完全である（後から同じ問い合わせの結果が増えることがない）」と約束し、lsp-det は rust-analyzer と gopls にこれを宣言していた。

Serena の調査で、Serena が rust-analyzer に `workspace.symbol.search.limit: 128` を渡していることが分かり、4 サーバーで `workspace/symbol` を測った（research/workspace-symbol-truncation-measurement.md）。

- rust-analyzer は 128 件、gopls は 100 件で**黙って**打ち切る。300 個のシンボルが一致する問い合わせに 128 件・100 件を普通の `result` 配列で返し、打ち切りを伝える手段は LSP にない（`isIncomplete` があるのは completion だけ）
- 上限の理由はソースの注釈にある。rust-analyzer は「VS Code のようなクライアントは絞り込みのたびに検索を出し直すので全結果を要らない」、gopls は「クライアントに送るべき結果の最大数」で、スコア順の上位を固定長の配列に保持する。`workspace/symbol` はエディタのピッカー向けのあいまい検索であり、列挙の契約を最初から持たない
- pyright と typescript-language-server は打ち切らない

つまり lsp-det の宣言は、2 つのサーバーに対して偽だった。さらに、打ち切りが上限の話であることを差し引いても、「完全」という語は保証の内容を超えている。保証できるのは「ワークスペース全体のインデックスに基づいて答えた」ことまでで、件数の上限やサーバーの検索の性質には及ばない。留保付きの保証は保証ではない（2026-09-04 の議論）。

## 決定

### A. `completeness` を `coverage` に改名する

capability の名前を `serverStateProvider.coverage` にし、定義を次に絞る。

> `readiness` が `"ready"` かつ `health` が `"error"` でないとき、7.0 の保証対象メソッドの応答はワークスペース全体のインデックスに基づく。インデックスの進行によって、後から同じ問い合わせの結果が増えることはない。

「完全」を名乗らない。約束するのは「全体を見て答えた」ことであり、`freshness`（受信済みの変更を織り込んだ）と対になる名詞として `coverage` を選ぶ。LSP に既存の語はない。日本語では「網羅」と呼ぶ。

### B. `workspace/symbol` を `coverage` の対象から外し、保留の対象には残す

仕様 7.0 を 2 段にする。

1. **ワークスペース横断メソッドの一覧**（11 個。従来どおり）: 9 章の「`ready` を待つ」対象であり、下流側の保留の対象
2. **`coverage` の保証対象**: 1 から `workspace/symbol` を除いた 10 個

`workspace/symbol` を保留に残すのは、インデックス中の応答が部分的であることは上限の有無と無関係に変わらないからである。保証から外すのは、上限による打ち切りをサーバーが伝えられず、`coverage` の定義にも入らないからである。仕様 10 章に rust-analyzer（128、`workspace.symbol.search.limit` で変更可）と gopls（100、固定）の上限と理由を記し、列挙が要るなら `textDocument/documentSymbol` をファイルごとに取ることを添える（Serena もそうしている）。

### C. 改名の範囲

仕様、設計、README、写像の宣言、準拠テスト、fork に用意した rust-analyzer と gopls のパッチ（`serverStateProvider` の宣言と rust-analyzer の `lsp-extensions.md`）。ADR 0004〜0011 の本文は書き換えない（ADR は採用後に本文を変えない）。索引に改名を記す。

### 却下した案

| 案                                                                               | 却下理由                                                                                                               |
| -------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| 名前は `completeness` のまま、定義だけ「後から増えない」に絞る                   | 語が約束を超えたままになる。読者は「完全」と読む                                                                       |
| `workspace/symbol` を対象に残し、上限を仕様に注記する                            | 留保付きの保証は保証ではない。上限を伝える語彙がサーバー側にない以上、クライアントは打ち切りを知りようがない           |
| capability に対象外のメソッドを列挙する構造（`coverage: {except: [...]}`）を足す | 今は 1 メソッドのためだけで、構造が増える。必要になったときに再検討する                                                |
| lsp-det が rust-analyzer への `initializationOptions` に大きな上限を注入する     | gopls には効かず、片方だけの対処になる。クライアントの設定を書き換えることにもなる                                     |
| `indexed` / `wholeWorkspace` などの別名                                          | `indexed` は形容詞で `freshness` と品詞が揃わない。`wholeWorkspace` は長く、`ready` の定義（インデックス完了）と重なる |

## 影響

### 仕様（docs/spec/server-state.md）

- 5 章の capability の型と注釈、5.1 の表、6 章 1 項、7.0（2 段に分ける）、7.2 の見出し、8.2 の 5、9 章 4 項、10 章の表と上限の注記
- `experimental/serverState` の応答や通知の形は変わらない。変わるのは `InitializeResult` の capability のキー名だけ

### 設計と README

- v0.1-design.md 4.2・4.3・5.2・8 章の `{completeness, freshness}` を `{coverage, freshness}` に。README 両版の要点と stderr の例
- vision.md 2.2 の記述

### 実装（ADR 0014・0015 の後にまとめて）

- `src/state.rs` の宣言の構造体とシリアライズ名、写像 4 つの宣言、`tests/conformance.rs` / `tests/client_conformance.rs` の断言、`src/gate.rs` のコメント
- 準拠テストは名前を追従させるだけで、7.2 の内容（`references` の完全な結果との一致）は変わらない
- fork のパッチ（`tagawa0525/rust-analyzer` の `server-state`、`tagawa0525/tools` の `server-state`）の宣言名と文書

### 過去の ADR

ADR 0004 決定 3 の名前は本 ADR が置き換える。0004・0007〜0011 の本文に残る `completeness` は当時の名前として読む。
