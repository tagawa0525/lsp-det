# ADR 0016: 保証の宣言を、真偽値ではなく欠けているものを名指しする形にする

- 日付: 2026-09-06
- 状態: 採用
- 関連: [ADR 0003](0003-extension-s-zero-based.md) 決定 3、[ADR 0004](0004-spec-grilling.md) 決定 3、[ADR 0009](0009-success-criterion-and-two-sided-reference.md) 決定 B（造語を避け LSP の語彙に合わせる）、[ADR 0013](0013-coverage-instead-of-completeness.md) 決定 B（置き換える）、[ADR 0014](0014-freshness-covers-watched-file-changes.md) 追補、[research/workspace-symbol-truncation-measurement.md](../research/workspace-symbol-truncation-measurement.md)、[research/disk-edit-propagation-measurement.md](../research/disk-edit-propagation-measurement.md)、[research/serena-solidlsp.md](../research/serena-solidlsp.md)

## 経緯

保証の宣言は `serverStateProvider: boolean | {coverage?: boolean, freshness?: boolean}` だった。実装を進めるうちに、真偽値では表せない現実のずれが 3 つ見つかった。

1. **件数の上限で結果を切る**: rust-analyzer は `workspace/symbol` を 128 件（設定で変更可）、gopls は 100 件（固定）で黙って打ち切る（research/workspace-symbol-truncation-measurement.md）。ADR 0013 決定 B はこれを「`workspace/symbol` を保証の対象から外す」で処理した。つまり現状のサーバーの上限に合わせて仕様を狭めた
2. **開いている文書だけから答える**: AL の言語サーバーは開くまでディレクトリのシンボルしか返さず、Svelte / Vue / Angular の相方の tsserver は事前に開いたファイルしか見ない（Serena が約 40 言語で埋めている穴の分類。research/serena-solidlsp.md と 2026-09-06 の調査）
3. **知らされた変更の取り込みが終わる前に `ready` を名乗る**: pyright と typescript-language-server は、新しいファイルの Created を知らされた後、取り込みの開始を伝えずに直後の問い合わせに古い答えを返す（research/disk-edit-propagation-measurement.md の追記）。ADR 0014 の追補は当初これを「`freshness` を宣言しない」で処理しようとした。真偽値なので、`didChange` と Changed で成り立っている保証まで落とすことになる

2026-09-06 の議論で方針を正した。**このプロトコルの目的は LSP にあるべき状態を示すことであり、現状のサーバーの上限に合わせて仕様を決めるのは筋が悪い。現実がそうなっているから仕方なくそうなっている、ということをサーバーに自覚させる形にする。** 仕様はあるべき姿を書き、宣言の側に「何が欠けているか」を機械可読で言わせる。欠けを名指しできれば、クライアントは欠けを読んで対処でき（問い合わせを絞る、ファイルを開く、応答の件数を上限と比べる）、上流には「この項目を埋めれば宣言から欠けが消える」と示せる。

## 決定

### A. `serverStateProvider` は常にオブジェクト。保証は範囲と欠けで表す

```typescript
serverStateProvider?: {
  coverage?: {
    scope: "workspace" | "openDocuments";
    incomplete: { [method: string]: number };
  };
  freshness?: {
    fileChanges: ("Created" | "Changed" | "Deleted")[];
  };
};
```

- `{}` は状態の通知だけを約束する（従来の `true`）。`true` という値はなくし、`boolean | object` の union を使わない（serde の `untagged` も要らなくなる）
- キーがあれば対応、値は options オブジェクト、値は文字列の列挙とメソッド名、という LSP の capability の慣行に倣う。動的登録と状態の通知への範囲の変化の同梱は採らない（範囲が起動後に変わる例はすべて一時的な穴で、`readiness` が表す）
- あるべき姿は `coverage: {scope: "workspace", incomplete: {}}`、`freshness: {fileChanges: ["Created", "Changed", "Deleted"]}`。そこからの欠けを書く
- `scope`（`"openDocuments"`）は lsp-det の 4 写像には当面使わないが、恒久の実例（AL、相方の tsserver）があるので語彙に含める。準拠テストは実例の写像を書くときに足す
- `incomplete` はメソッド名から上限の件数への対応。件数を入れるのは、クライアントが応答の件数を上限と比べて打ち切りを機械的に判断できるからで、上限が固定か設定可能かはクライアントからは分からない。観測者は測って知っている上限だけを宣言し、上限が分からないメソッドを載せることはできない（載せられなければ `coverage` そのものを宣言しない）。名前は completion の `isIncomplete` に寄せた
- `freshness.fileChanges` は織り込む `workspace/didChangeWatchedFiles` の変更の種類で、値は LSP の `FileChangeType` の名前。`textDocument/didChange` を織り込むことは `freshness` を宣言する前提であり、段階として名付けない。段階の語（`"documents"` / `"files"` / `"workspace"` など）を発明する案は、語だけで境目が伝わらず、LSP の値の名前を並べれば足りるので採らない

### B. 仕様 7.0 を 1 つの一覧に戻す

`workspace/symbol` を `coverage` の対象に戻す。上限は `incomplete` に宣言する。ADR 0013 決定 B を置き換える。

### C. 準拠要件を宣言の内容に対応させる

- 7.2 に「`incomplete` に挙げたメソッドは上限の件数を返し、挙げていないメソッドはすべてを返す」を足す
- 7.3 を `fileChanges` の種類ごとに分ける（`didChange`、Changed、Created、Deleted）
- 観測者が宣言できるのは、宣言する内容に対応する要件を通した場合だけ（8.2 の 5）

### D. 各サーバーの宣言（実測に基づく）

| サーバー                   | coverage                                                                                                                             | freshness                                          | 根拠                                                                                     |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| rust-analyzer              | `{scope: "workspace", incomplete: {"workspace/symbol": 128}}`（上限は `initializationOptions.workspace.symbol.search.limit` を読む） | `{fileChanges: ["Created", "Changed", "Deleted"]}` | Created / Deleted は ADR 0014 追補 決定 D の先読み。Deleted は実装の段で 7.3 の 4 を通す |
| gopls                      | `{scope: "workspace", incomplete: {"workspace/symbol": 100}}`                                                                        | 同上                                               | 同期的に取り込む。Deleted は実装の段で通す                                               |
| pyright / basedpyright     | `{scope: "workspace", incomplete: {}}`                                                                                               | `{fileChanges: ["Changed"]}`                       | Created / Deleted の取り込みの開始を伝えない                                             |
| typescript-language-server | `{scope: "workspace", incomplete: {}}`                                                                                               | `{fileChanges: ["Changed"]}`                       | 同上（TypeScript の再帰監視が 1 秒のタイマー）                                           |

### 却下した案

| 案                                                                      | 却下理由                                                                           |
| ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| 真偽値のまま、欠けは仕様 10 章の注記で表す                              | 現状に合わせて仕様を狭めるか、留保付きの保証になる。クライアントは機械的に読めない |
| `workspace/symbol` を保証の対象から外す（ADR 0013 決定 B）              | あるべき姿を仕様から消す。上限を宣言に書けば対象に戻せる                           |
| pyright と tsls の `freshness` を落とす（ADR 0014 追補の当初案）        | 1 つの真偽値のために、成り立っている保証まで落とす                                 |
| 真偽値とオブジェクトの union（LSP の `renameProvider` の型）            | 型が混在し serde で `untagged` が要る。常にオブジェクトで足りる                    |
| `freshness` を段階の語で表す（`documents` / `files` / `workspace` 等）  | 語だけで境目が伝わらず、説明に頼る。`FileChangeType` の名前を並べれば足りる        |
| `incomplete` をメソッド名の一覧にする（件数なし）                       | 件数があればクライアントが打ち切りを機械的に判断できる                             |
| 動的登録（`client/registerCapability`）や状態の通知に範囲の変化を載せる | 範囲が起動後に変わる例はすべて一時的な穴で、`readiness` が表す                     |

## 影響

### 仕様（docs/spec/server-state.md）

5 章の型と 5.1、6 章 1・2 項、7.0（1 つの一覧）、7.2（上限の要件）、7.3（種類ごと）、8.2 の 5、10 章の表（各サーバーの宣言と上限の読み方。rust-analyzer の上限は `initializationOptions` から読み、起動後の `workspace/didChangeConfiguration` による変更は反映されない）

### 実装

- `src/state.rs`: `ServerStateProvider` を struct に（`untagged` の enum をなくす）。`Guarantees` の `coverage: Option<Coverage>`、`freshness: Option<Freshness>`
- 写像 4 つの宣言。rust-analyzer は `initialize` の `initializationOptions` から上限を読む（`Mapping::observe_client` の初出）
- `tests/conformance.rs` の宣言の期待値、7.2 の 2（上限の実測。rust-analyzer と gopls）、7.3 の 4（Deleted。rust-analyzer と gopls）
- fork のパッチ（rust-analyzer、gopls）の宣言、README 両版

### 過去の ADR

ADR 0013 決定 B と ADR 0014 追補の当初の「`freshness` を落とす」案を置き換える。0013 決定 A（`coverage` の名前と定義）、0014 決定 A〜D は生きている。
