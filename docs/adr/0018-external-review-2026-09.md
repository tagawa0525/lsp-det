# ADR 0018: 外部レビュー（2026-09）への対応

- 日付: 2026-09-06
- 状態: 採用
- 関連: [research/external-review-2026-09.md](../research/external-review-2026-09.md)（レビュー本文）、[ADR 0006](0006-external-review-fixes.md)（前回の外部レビュー）、[ADR 0009](0009-success-criterion-and-two-sided-reference.md) 決定 C-4（時間判定の排除）、[ADR 0016](0016-declaration-shape.md)、[ADR 0017](0017-languages-of-documents-and-code.md)、[ADR 0019](0019-v0.4-corpus-and-counterexamples.md)（本 ADR で決めた検証の範囲）、[research/claude-code-dogfooding.md](../research/claude-code-dogfooding.md) 第 5 回

## 経緯

別の対話で本プロジェクトを評価してもらったフィードバック（2026-09-04〜06。`CLAUDE.md`、仕様、ADR 0001 / 0008 / 0009、research/serena-solidlsp.md を読んだうえでの指摘）を [research/external-review-2026-09.md](../research/external-review-2026-09.md) に置いた。外向きの提出（`docs/upstream-submissions.md`）に着手する直前だったので、出す前に取り入れるものを決める。

レビューの時点（09-04）から動いたものがある。保証の宣言は真偽値から欠けを名指しする形になり（ADR 0016）、Claude Code でのドッグフーディング第 5 回で lsp-det あり／なしの対照を取り、gopls の `TESTED_VERSIONS`（0.23.0）は最新タグと一致している。以下はそれを踏まえた採否である。

## 決定

### A. 取り入れる

1. **無期限保留の観測可能性**（レビュー §1-1）。時間の非常口を廃した帰結として、写像が信号を取り逃したときの症状は「横断リクエストが無期限に保留」になり、クライアントのタイムアウトとしてしか見えない。下流側は保留の開始（メソッド、リクエスト id、そのときの `ServerState`）と解放（保留時間、解放の理由: `ready`、`error` による拒否、`$/cancelRequest`、`shutdown`、上流の消失）を stderr に出す。時計は使わず、出来事のたびに出す。クライアントへ `window/logMessage` は送らない（上流にない通知を中継層が足さない。仕様 8.2 の 6 の趣旨）。Claude Code は `--debug` でしか言語サーバーの stderr を残さないので、`dogfood/README.md` にそう書く
2. **実害の一事例**（§4）。第 5 回の道具で、「参照がなければその関数を削除せよ」と指示した走行と、「使われていない関数を整理して」と自然に頼んだ走行の両方を、tsls の起動直後と gopls の Bash で作ったファイルの 2 つ × 直接 / lsp-det 経由で残す（第 6 回）。指示した版は決定的で、自然な版は「命じたから消した」という読まれ方への備え
3. **仕様 10 章に Dart と Sorbet の行**（§2）。Dart analysis server の `$/analyzerStatus`（`isAnalyzing`）と Sorbet の `sorbet/showOperation`（Indexing / Typechecking の開始と終了。文書に「Find All References は Idle でしか答えない」と明記）は rust-analyzer の `quiescent` と同型の語彙で、一次資料で確かめた。測っていないので jdtls / clangd と同じ「見込み」の行として英日両方に書く
4. **gopls の foldingRange の証拠**（§2）。`gopls/internal/golang/folding_range.go` に「パースエラー時に結果が信用できないことを LSP で伝える手段がない（microsoft/language-server-protocol#1200）。エラーを返すのも怖いので空を返す」とある。gopls チーム自身が問題を認めている一次資料として research に記し、gopls への提案の文面で引く
5. **Serena の 30 言語を本プロトコルの語彙に写す机上コーパス**（§6）。1 言語 1 行で、Serena の手段、写像した値（`initializing` / `indexing` / `ready` / `unknown`）、写像できない理由を書く。上流提案の添付資料であり、語彙を硬くしてよい範囲の根拠になる。反例の実サーバーでの検証は ADR 0019
6. **上流は「却下した」のではなく「誰も頼んでいない」**（§3）。Serena のメンテナ 2 名は言語サーバー本体に readiness の提案を出していない。`docs/upstream-submissions.md` の方針にこの前提を記す

### B. 取り入れない

1. **観測者の `freshness` 宣言は根拠が薄い**（§1-3）。宣言は欠けを名指しする形（ADR 0016）で、宣言する種類ごとに 7.3 の要件を通した版にだけ宣言する（仕様 8.2 の 5）。「一検体で全称は言えない」はあらゆる準拠テストに当てはまる話で、宣言の意味は「この版でこの要件を通した」以上でも以下でもない。rust-analyzer と gopls の提案ではサーバー自身が宣言するので、この反論は当たらない。仕様は変えず、上流に出す文面でこの答えを用意する
2. **JetBrains の Dumb Mode、agent-lsp、isaacphi/mcp-language-server**（§2）。前者は LSP 本体への提案の段で先例として引けば足りる。後者 2 つは下流の被験者候補として vision に名前を残す

### C. 信号は他の実装から推測しない

コーパスの「写像に疑問」の行を選ぶ前提として、**他の実装（Serena 等）の待ち方から信号を推測しない**。Serena の sleep や安全バッファには、サーバーの挙動ではなく CI の不安定さや貢献者の手癖で入ったものが混ざる。写像を書く前に、そのサーバー自身の文書とソースで信号の有無と意味を確かめる。`CLAUDE.md` の絶対の制約に加える。

### D. 外部へのアクションは 0.5.0 の後

提出（`docs/upstream-submissions.md` の全部。プロトコルと無関係な typescript-language-server の不具合修正 PR も含む）は、反例の検証（ADR 0019、0.4.0）と易しい 4 サーバー（0.5.0）の後にする。理由は「上流に通した後で仕様を変えたくなれば相手に迷惑」であり、外部に出すまでは理想を追い根源的に解く。Claude Code への報告は、既存の issue 3 件（#85225 Created を通知しない、#76870 起動直後の不完全な結果、#82416 tsls のクラッシュ後の空応答）へのコメントと、新規 2 件（`shutdown` の `params: {}`、`didClose` を送らない）の形にする。

### 却下した案

| 案                                                                    | 却下理由                                                                                                    |
| --------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| 保留のログをクライアントへ `window/logMessage` でも送る               | 上流にない通知を中継層が足すことになる。任意の LSP サーバーの stderr を見る手段はクライアントが持つべきもの |
| 環境変数でログファイルを指定できるようにする                          | 知っている人しか使わない知識が増える。第 6 回で不足を感じたら再検討                                         |
| 実害の一事例は指示した版だけにする                                    | 「命じたから消した」と読まれる。自然な版も残す                                                              |
| typescript-language-server の不具合修正 PR だけ先に出す               | 外部に出すまでは全部止める方針（決定 D）                                                                    |
| Claude Code への報告を新規 4 件で出す                                 | 既存の報告と重複し読まれない                                                                                |
| §1-3 に応えて仕様に「観測者は保証を宣言しない」を戻す                 | 決定 B-1                                                                                                    |
| 語彙を変える閾値（「変える証拠が重くなければ変えない」）を ADR に書く | 外部に出すまでは理想を追う。手戻りを理由に変えないことをしない                                              |

## 影響

- `docs/research/external-review-2026-09.md`: レビュー本文（`reference/` から移動して追跡）
- `CLAUDE.md`: 決定 C の制約、現在地の順序（保留のログ → 第 6 回 → 文書 3 件 → コーパス → 反例の検証 → 0.5.0 → 提出）
- `CHANGELOG.md` の予定、`docs/upstream-submissions.md` の方針（決定 D、A-6）
- 実装: 決定 A-1（`src/gate.rs` / `src/proxy.rs` のログ。`dogfood/README.md`）
- 文書: 決定 A-2（research/claude-code-dogfooding.md 第 6 回）、A-3（仕様 10 章、英日）、A-4（research）、A-5（research の新しい文書）、B-2（vision）
