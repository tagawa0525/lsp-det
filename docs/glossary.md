# 対訳表

日本語の文書（ADR、research、設計）と英語の文書（README、仕様）・コードのコメントで同じ概念を同じ語で呼ぶための表（ADR 0017 決定 D）。訳語を変えるときは、まずこの表を直してから全体を直す。LSP に既存の語があればそれを使い、造語を作らない（ADR 0009 決定 B）。

## プロトコル

| 日本語                     | English                                           | 備考                                                                           |
| -------------------------- | ------------------------------------------------- | ------------------------------------------------------------------------------ |
| サーバー状態プロトコル     | the server state protocol                         |                                                                                |
| サーバー状態               | server state                                      | 型は `ServerState`                                                             |
| 無言の嘘                   | silent lies                                       | インデックス未完了の空応答・壊れたサーバーの成功風応答・編集を織り込まない応答 |
| 軸                         | axis                                              | `health` と `readiness` の 2 軸 = the two axes                                 |
| 保証                       | guarantee                                         | `coverage` と `freshness`。動詞は guarantee                                    |
| 網羅                       | coverage                                          | 6 章 1 項の見出し。保証の名前と同じ語                                          |
| 鮮度                       | freshness                                         |                                                                                |
| 宣言（する）               | declaration / declare                             | capability に書くこと                                                          |
| 欠け（を名指しする）       | what is missing (name what is missing)            | あるべき姿 = the ideal からの欠け                                              |
| あるべき姿                 | the ideal                                         | `coverage: {scope: "workspace", incomplete: {}}` 等                            |
| 織り込む                   | incorporate                                       | 変更を応答に反映すること。織り込み済み = incorporated                          |
| 打ち切り（件数の上限）     | cap                                               | 「上限で切る」= cap at N。時間の打ち切りは timeout                             |
| 横断（ワークスペース横断） | cross-workspace                                   | 7.0 の一覧 = the cross-workspace methods                                       |
| 単一ファイルの問い合わせ   | single-file queries                               | hover、completion、documentSymbol 等                                           |
| 準拠要件                   | conformance requirements                          | 7 章、8.4、9.1                                                                 |
| 準拠テスト                 | conformance tests                                 | `tests/conformance.rs`、`tests/client_conformance.rs`                          |
| 規範 / 非規範              | normative / non-normative                         |                                                                                |
| 推奨挙動                   | recommended behavior                              | 9 章                                                                           |
| 信号                       | signal                                            | サーバーが出す通知・ログなど、状態を読み取る根拠                               |
| 合成する                   | synthesize                                        | 観測者が値を作ること。8 章                                                     |
| 送出主体                   | who may emit                                      | 8.3 の表                                                                       |
| 再インデックス             | reindexing                                        | 6 章 3 項                                                                      |
| 時間に基づく判定           | time-based judgment                               | 禁止事項。「一定時間で ready とみなす」= treat as ready after some time        |
| 前方互換                   | forward compatibility                             | 3 章                                                                           |
| 状態の問い合わせ / 通知    | the state request / the state change notification | `experimental/serverState` / `experimental/serverStateChanged`                 |

## 実装者と構造

| 日本語                   | English                                 | 備考                                                                        |
| ------------------------ | --------------------------------------- | --------------------------------------------------------------------------- |
| サーバー（言語サーバー） | server (language server)                |                                                                             |
| クライアント             | client                                  | エディタ、エージェント、ブリッジ                                            |
| 観測者                   | observer                                | サーバーの外から見る主体。プロキシ、クライアントライブラリ、エディタ本体    |
| 中継層                   | relay                                   | 観測者のうち、間に立つもの                                                  |
| 上流 / 下流              | upstream / downstream                   | 上流 = 言語サーバー、下流 = クライアント                                    |
| 上流側 / 下流側          | the upstream side / the downstream side | lsp-det の 2 つの半分                                                       |
| 写像                     | mapping                                 | サーバーの語彙を `ServerState` に対応づけるもの。`Mapping` trait            |
| 恒等写像                 | the identity mapping                    | 上流が自らプロトコルを話すときの写像                                        |
| 名乗り                   | what the server calls itself            | `serverInfo.name` または起動時のログ                                        |
| 代行（する）             | stand in for / stand-in                 | クライアントに足りないものを下流側が行うこと                                |
| 保留（する）             | hold                                    | 横断リクエストを `ready` まで止めること。保留中 = held                      |
| 素通し                   | pass through                            |                                                                             |
| 透過プロキシ             | transparent proxy                       |                                                                             |
| 先読み                   | predict                                 | Created / Deleted の通知から `indexing` を先に立てること（ADR 0014 決定 D） |
| 偽上流 / 偽クライアント  | fake upstream / fake client             | `examples/fake_lsp_server.rs` / `examples/pseudo_client.rs`                 |
| 実サーバー               | real server                             |                                                                             |
| 監視対象のファイル       | watched files                           | `workspace/didChangeWatchedFiles` の対象                                    |
| 重複した `didOpen`       | duplicate `didOpen`                     | 既に開いている uri への `didOpen`                                           |
| フラップ                 | flap                                    | `quiescent` の往復                                                          |
| 宣言範囲                 | declaration ranges                      | vision の凍結中の項目                                                       |
| 起動方法の宣言           | launch manifest                         | 同上                                                                        |
| 造語                     | coined term                             |                                                                             |

## 文書の型

| 日本語     | English                 | 備考                       |
| ---------- | ----------------------- | -------------------------- |
| 経緯       | Context                 | ADR の節                   |
| 決定       | Decision                |                            |
| 却下した案 | Rejected alternatives   |                            |
| 影響       | Consequences            |                            |
| 追補       | Addendum                |                            |
| 廃止       | Superseded              | 置き換え先 = superseded by |
| 実測       | measurement             | research の実測 = measured |
| 見込み     | expected (not measured) | 10 章の「見込み」          |
