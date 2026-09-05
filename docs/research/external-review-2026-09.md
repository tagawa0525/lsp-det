# lsp-det フィードバック（2026-09-04〜06 の対話より）

対象: https://github.com/tagawa0525/lsp-det
読んだもの: `CLAUDE.md`、`docs/spec/server-state.md`、`docs/adr/0001,0008,0009`、`docs/research/serena-solidlsp.md`、`reference/README.md`

事実（調査で確認済み）と推測（構造からの推論）を分けて記す。

---

## 1. 方針への評価

### 同意する点

- health / readiness の 2 軸分離と「失敗は health で表す」（ADR 0008）。rust-analyzer の `{error, quiescent:true}` という実挙動に裏付けがある。
- 観測者が合成する値（`unknown`、観測者宣言の guarantees）を別層に切り出し、上流提案は仕様 3〜7 章だけで成立させる構成（ADR 0009 C-1）。提案時に `unknown` が削られても本体が壊れない。
- 保証を「準拠テストを通した版の範囲」でしか宣言しない（D-5）。7.3 のクロスファイル必須は正しい。単一ファイルは LSP の処理順序保証で通ってしまう。
- 「時間で `ready` を合成しない」原則。

### 懸念

1. **無期限保留の観測可能性**
   時間の非常口を廃止した帰結として、写像が信号を取り逃したときの症状は「横断リクエストが無期限に保留」になる。外側クライアントの timeout として現れ、lsp-det 側の問題だと分かりにくい。時間で `ready` にしないのは正しいとして、保留が続いている事実を stderr / `window/logMessage` 等で観測可能にする設計が要る。CLAUDE.md からは読み取れなかった。特に pyright のログ regex 依存は版更新で再発しやすい。

2. **上流提案の現実的経路**
   LSP #511 は 2018 年から開いたままで、メンテナは「`$/progress` で足りる」の立場。実効的な経路は、rust-analyzer / gopls / pyright がネイティブに `experimental/serverState` を喋る「事実上の標準化」を先に積むこと。v0.2 の「上流 issue は独立に進めてよい」を前倒しする方が期待値は高い。

3. **観測者が `freshness` を宣言することの強さ**
   7.3 は特定時点の一検体を通した証拠で、「受信した didChange をすべて織り込む」という全称の保証を版の範囲で言い切るには弱い。仕様上は限定しているので違反ではないが、提案時に「観測者の completeness/freshness 宣言は根拠が薄い」と突かれる。却下済みの `basis: "observed"` や「観測者は `serverStateProvider: true` のみ」案の再検討余地あり。

4. **gopls の検証版**
   `docs/spec/server-state.md` の gopls 行は v0.23.0。gopls は版更新が速いので `TESTED_VERSIONS` が現行版をカバーしているか継続的に確認が要る。

---

## 2. 先行実装・関連事例の追加候補

`docs/research` を grep して見当たらなかったもの。

| 対象                                            | 内容                                                                                      | 状態                                                                                         | 用途                                                                               |
| ----------------------------------------------- | ----------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| Dart analysis server `$/analyzerStatus`         | `{isAnalyzing: bool}` 通知。rust-analyzer の `quiescent` と同型                           | **実在確認済み**（Serena issue #1428 のログに「Unhandled method '$/analyzerStatus'」が多数） | 仕様 10 章の写像表に追加。Serena が拾い損ねている実例でもある                      |
| Sorbet `sorbet/showOperation`                   | "Indexing" / "Typechecking" の開始・終了通知                                              | 記憶ベース、未確認                                                                           | 粗いが写像可能                                                                     |
| JetBrains Dumb Mode / `DumbAware`               | インデックス中「結果が不完全かも」を IDE 全体の概念として持ち、処理を分ける               | 一般知識                                                                                     | 決定 D-7（indexing 中に通すメソッドと待つメソッドを分ける）の先例として引用可      |
| agent-lsp                                       | 言語サーバーをセッション横断で常駐させる MCP オーケストレーション層                       | 検索で確認                                                                                   | readiness 問題を「常駐で回避する」別解。下流側の被験者候補                         |
| isaacphi/mcp-language-server                    | Go 製の LSP→MCP ブリッジ                                                                  | `reference/` に未収録                                                                        | 下流側の被験者候補                                                                 |
| gopls コミット（foldingRange、LSP #1200 参照）  | パースエラー時に「結果が信用できないことを LSP で伝える手段がないので何も返さない」と明記 | 検索で確認                                                                                   | gopls チーム自身が「信頼できない結果を機械的に伝えられない」問題を認識している証拠 |
| LSP `ContentModified` / #1367 `ServerCancelled` | 「答えられるが今の答えは信頼できない」を表す既存語彙                                      | 既知だが位置づけの整理に                                                                     | 上流提案で「既存語彙では足りない理由」の説明材料                                   |

「readiness をゲートする透過プロキシ＋その語彙の上流提案」という組み合わせそのものの直接の先行例は見つからなかった。

---

## 3. Serena の上流活動の調査結果（事実）

質問: Serena の作者は各 LS に readiness の PR を出して却下されたのか。

**結論: 却下ではなく、出していない。**

GitHub issue 検索 API で、メンテナ 2 名（MischaPanch、opcode81）が `oraios/serena` 以外に出した issue / PR を全件確認（MischaPanch: issue 196 件・PR 112 件、opcode81: issue 132 件・PR 105 件）。

| 上流                               | MischaPanch | opcode81 |
| ---------------------------------- | ----------- | -------- |
| rust-lang/rust-analyzer            | 0           | 0        |
| golang/go（gopls）                 | 1           | 0        |
| microsoft/pyright                  | 0           | 0        |
| typescript-language-server         | 0           | 0        |
| microsoft/language-server-protocol | 0           | 0        |
| microsoft/multilspy                | 2           | 0        |

- gopls の 1 件: golang/go #73521「DocumentSymbol returns incorrect selectionRange」（2025-04）。readiness ではなくシンボル範囲の問題（lsp-det の旧「拡張 A」相当）。ラベル WaitingForInfo + FrozenDueToAge で情報不足のまま自動クローズ。
- multilspy の 2 件: 利用報告と、複数 LS でメソッド未実装の指摘。readiness ではない。
- 両者の外部活動は penpot（MCP 統合）、OpenHands、tianshou に集中。言語サーバー本体への働きかけはほぼ皆無。

調査の限界:

- 2 アカウントのみ。他コントリビュータの上流活動は未確認。
- Serena リポジトリ内の「なぜ上流に出さないか」の議論は未検索。
- GitHub API レート制限で #73521 の本文とコメントは未読（ラベルは goissues.org の一覧で確認）。

含意:

- 「上流は受け入れない」と「上流は誰にも頼まれていない」を現時点では区別できない。却下記録がないので、提案戦略の前提を「未試行」に置く。
- gopls チームは情報不足の issue をそのまま凍結させる運用。上流 issue を出すなら再現 fixture と実測ログ（`docs/research/*-measurement.md`）を最初から添える。

---

## 4. 「そもそも issue なのか」への整理

上流の立場では**バグではない**。LSP はインデックス未完了中の不完全な結果を禁じておらず、`references` の応答に完全性の約束はない。clangd は設計上、references が完全になることがない。「無言の嘘」はクライアント側の解釈（結果を完全とみなす）の問題であり、上流はそう見るし、それは筋が通っている。

したがって分類は **feature request（仕様の欠落）**。上流提案は「壊れている」ではなく「エージェントという新しいクライアント種別では、進捗バーを人間が見る代わりに機械可読な完全性の宣言が要る」の形でしか通らない。

**実害の証拠が薄い**: 「エージェントが空応答を信じて誤った行動をとった」記録が lsp-det のレポにも Serena の issue にも見当たらない。Claude Code のドッグフーディング記録は「保留すれば完全になる」の確認であり、「保留しなければどう間違えたか」の対照記録ではない。上流を動かす材料としては、仕様の整合性より具体的な一事例（「起動後 N 秒で references を投げると M 件中 0 件、エージェントは参照なしと判断してシンボルを削除した」など）の方が重い。**ドッグフーディングで対照実験（gate off）の記録を取ることを推奨。**

---

## 5. Serena の内部設計から学ぶ点

solidlsp の基底クラスが持つ抽象は `_get_wait_time_for_cross_file_referencing()`（既定 2 秒 sleep）と `_pre_open_for_cross_file_references()` の 2 フックのみで、readiness という概念は基底に存在しない。各言語クラスが `_server_ready` を勝手に立て、勝手に timeout し、勝手に proceed する。

- 「今 ready か」を外から問う API がなく、ツール層は結果の完全性を知れない。
- 同名イベントの意味が言語ごとに違う（Haxe は最初から set、Svelte は timeout で raise、他は proceed）。
- 既存信号を拾い損ねても（Dart `$/analyzerStatus`）、基底に報告義務がないので誰も気づかない。

Serena は「幅（30 言語、コントリビュータが数日で追加）」を取って「硬さ」を捨てた。取引自体は妥当。問題は捨てたことを `unknown` のような**外から見える形で表明しなかった**こと（硬さの欠如ではなく、柔らかさの不可視化）。

**M7 への含意**: `ls_base_cmd` で lsp-det を挟むだけでは solidlsp 側の固定 sleep と timeout が残り、二重に待つ。本当に効かせるには solidlsp の基底クラスに `experimental/serverState` を読む 1 経路を足す PR が要る。上流の言語サーバーより Serena の基底クラスの方が先に動かせる可能性が高い。

---

## 6. 語彙を硬くしてよい範囲（Serena のデータによる検証）

### 硬くしてよい: readiness の 1 軸

Serena の 6 類型（通知 / progress drain / ログ / 診断到着 / sleep / 即時）は全て「初回ワークスペーススキャンが終わったか」の 1 ビットに潰れる。`initializing | indexing | ready | unknown` はこれを写像できる（sleep / 即時は `unknown`）。30 の独立実装者が同じ 1 ビットを別々の方法で取りに行った事実が、需要と語彙の妥当性の証拠。

### Serena のデータでは検証できない軸

- **health**: Serena に失敗の概念がない（全実装が timeout → proceed）。壊れたサーバーと遅いサーバーを区別していない。lsp-det の health は rust-analyzer / gopls / jdtls 以外で写像できる信号があるか未確認。
- **再インデックス**: 起動時に一度待つだけ。`ready → indexing → ready` はほぼ扱っていない（pyrefly のリトライ、Kotlin の安全バッファ程度）。
- **freshness**: 全くモデル化されていない。LSP の処理順序保証に暗黙依存。「全インデックスかつ非同期」象限（tsserver 系）は M6 で初めて分かる。
- **複数サーバーの合成**: Vue / Svelte / Angular はコンパニオン TS サーバーとの conjunction で ready を決める。lsp-det は上流 1 つの設計で、語彙にも構造にもない。

### 硬くする前に当てるべき反例候補（Serena の表より）

| 言語                   | Serena の挙動                                           | 問題                                                                                                                                   |
| ---------------------- | ------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Metals                 | progress end 後に quiet_period 3 秒                     | end 信号を信用していない。時間なしで「静穏」を表現できるか。rust-analyzer のフラップ実測（ADR 0007）が他サーバーでも成り立つかの試金石 |
| Erlang                 | ログ + progress + settling sleep                        | 同上                                                                                                                                   |
| Nextflow               | references 要求ごとに明示同期                           | グローバル readiness ではなくリクエスト単位の鮮度確認モデル。`freshness` の別実装形態として仕様 10 章に載せる価値                      |
| Haxe                   | キャッシュありなら最初から ready、progress 発生で clear | `initializing` から始まらない。7.1 の「fixture の規模」前提を置いた判断の実例                                                          |
| AL                     | `al/hasProjectClosureLoadedRequest` をポーリング        | 通知でなくリクエストで状態取得。`experimental/serverState` リクエストで写像可能                                                        |
| Vue / Svelte / Angular | コンパニオン TS の indexing drain との conjunction      | 複数上流の合成。現設計の範囲外                                                                                                         |

### 提案: Serena の表を「机上コーパス」として使う

30 言語それぞれについて、Serena の手段を lsp-det の語彙に写像した値と、写像できない理由を 1 行ずつ書く。全部埋まれば readiness の語彙は硬くしてよい。埋まらない行が「硬くしてはいけない箇所」の一覧になる。実サーバーで 7.x を回すより桁違いに安く、上流提案時に「30 言語の既存実装がこの語彙に収まる」という添付資料になる。

---

## 7. アクション候補（優先度順）

1. 無期限保留の観測可能性を設計に入れる（§1-1）。
2. Serena の 30 言語表 → lsp-det 語彙の写像コーパスを作る（§6）。反例候補の Metals / コンパニオン系から着手。
3. ドッグフーディングで gate off の対照記録を取り、実害の一事例を作る（§4）。
4. `docs/research` に Dart `$/analyzerStatus` を追加（実在確認済み）。Sorbet は一次ソース確認後に追加。
5. M7 の設計を「`ls_base_cmd` 差し込み」から「solidlsp 基底クラスに `experimental/serverState` 経路を足す PR」へ（§5）。
6. 上流提案の前提を「却下された」ではなく「未試行」に置き、gopls 向けには再現 fixture + 実測ログを必ず添える（§3）。
7. `TESTED_VERSIONS` の gopls 現行版カバレッジ確認（§1-4）。
