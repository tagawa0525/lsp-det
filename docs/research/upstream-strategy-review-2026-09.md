# 対外戦略への批判的レビュー（2026-09-09）

対象: `docs/upstream-submissions.md` の戦略の草稿（位置づけ・順序の原則・規則・段階・未調査。PR #80 の最初の版）
レビュー担当: 別の Claude（Fable）のセッション。読み取り専用で、`gh` による上流リポジトリの調査を許可し、外部への書き込みは禁じた
依頼: 戦略の最大の弱点、順序の是非、各上流のメンテナの立場での却下理由、位置づけの説得力、規則の欠けと過剰、出す前に調べること、文書の形。甘い評価は不要で、最悪の展開を想像しそれを避ける変更を述べること

レビューの本文を以下に写す（issue 番号には owner/repo を補い、それ以外は原文のまま）。採否は末尾の節。

---

## 1. 最大の弱点: 「相手の痛み」の裏付けが上流側の設計意図と噛み合っていない

- **tsls の PR は、メンテナが 2021 年に決めた設計に逆行する**（事実）。typescript-language-server/typescript-language-server#302 で rchl は「tsserver の再起動は state が多すぎて不可。LS 本体を落としてクライアントに再起動させる」と決め、#305 "fix: exit the server if tsserver process crashes" を入れた。現在の `src/lsp-server.ts` L217–222 は `onExit: (exitCode, signal) => { this.shutdown(); if (exitCode) throw … }` で、SIGKILL は `exitCode: null` なので落ちない。つまり lsp-det が測った「死後に `[]`」は **#305 の 1 行の取りこぼし**（`signal` を見ていない）であり、fork の 26 行の `RequestFailed` 経路は「設計と違う直し方」に見える。草案の「再起動は次の段階」は #302 で却下済みの案を再提示する形になる
- **gopls には起動直後の無言の嘘がない**（事実）。`gopls/internal/cache/snapshot.go` の `ReverseDependencies` / `WorkspaceMetadata` は `awaitLoaded` → `AwaitInitialized`（`initialWorkspaceLoad` を待つ）を呼ぶ。`references.go` は L149/L155/L289 でこれらを使う。第 6 回の gopls の実害（`docs/research/claude-code-dogfooding.md`）は CC が `didChangeWatchedFiles` を送らないことが原因で、gopls の readiness ではない。gopls への「serverState を話す PR」の痛みは `health`（"Error loading workspace"）だけになる。加えて gopls は自前の MCP サーバー（`gopls/internal/mcp`、`go_symbol_references`、`instructions.md`）を持つ。「エージェント用途」への gopls チームの答えは既に「gopls mcp を使え」であり、`docs/` にはこの事実がない（grep 0 件）
- 崩れると全体が成り立たない前提: 「上流は未試行」（ADR 0018 A-6）は正しいが、**「上流が既に別の答えを持っている」を調べていない**。tsls（#305）、gopls（awaitLoaded・mcp）、rust-analyzer（rust-lang/rust-analyzer#10888 を 2024 年に Veykril が「serverStatus で既にある」と閉鎖）はいずれも先に答えを出している

## 2. 順序

- 第 1 段の tsls は**出し直しが要る**。#305 の取りこぼし修正（`if (exitCode || signal)`）として出す方が受かる。ただしそれが通ると tsls は落ちるので、CC は anthropics/claude-code#82416 の Symptom 2（死んだと知らず "server is running"）に落ちる。#82416 の Symptom 1 は「tsserver は生きているが RSS 1 MB で SIGTERM も効かない」で、`NoServer` ではなく PR の対象外。「痛みは #82416 が裏付ける」は半分しか成り立たない
- **消費者の 2 番目は Serena より Claude Code**。anthropics/claude-code#76870 は bcherny（CC の作者）が 08-17 に「両方再現、tracking」と明言し、本文の "Proposed fix" は「サーバーの初期索引の信号を待つ、または bounded retry / settle」。**信号を待つ実装を CC が今まさに探している**のに、戦略は CC を「報告先」としか扱っていない。`experimental.serverState` を宣言して待つ、を CC に提案し lsp-det を今日の橋として添えるのが最も期待値が高い（推測）
- 第 2 段の gopls は削るか `health` の issue に縮める。第 3 段の「12 サーバー一括」は規則 3「同じ上流には一度に 1 本ずつ」と矛盾しないが、LSP 本体の proposal は `contributing.md`（main）が「issue → `vscode-languageserver-node` の TypeScript 定義 → 仕様への PR → `vscode-languageclient` の参照実装が望ましい」を要求する。**`protocol.serverState.proposed.ts` の用意が段階に入っていない**

## 3. 各上流で最も可能性の高い却下理由

| 提出先        | 却下理由（推測。根拠は事実）                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| tsls          | 「#305 の意図どおり落とせばよい。この PR は別の経路を足している」（上記）                                                                                                                                                                                                                                                                                                                                                                                   |
| Claude Code   | 既存 issue の重複と自動クローズ。`didClose` は anthropics/claude-code#64276 が NOT_PLANNED（stale）で閉じ済み、`workspace/configuration` は anthropics/claude-code#16360（open、53 コメント、oncall）に既にある。#33035 / #32595 / #50271 も NOT_PLANNED。新規 3 件のうち 2 件は重複で「読まれない」（ADR 0018 の却下案と同じ結果）                                                                                                                         |
| pyright       | `CONTRIBUTING.md`「新機能は先に enhancement request」。`serverInfo` は HEAD の `languageServerBase.ts` L649 にまだないが、「どのクライアントが要るのか」への答えが lsp-det だけだと弱い                                                                                                                                                                                                                                                                     |
| Serena        | `CONTRIBUTING.md`「小さな修正以外は issue から」。3 週間で外部 PR 25 本を取り込む速さで、`fix(dart): wait for analyzer readiness signal`（09-03）のように**サーバーごとの待ちを足す**のが彼らの流儀。(c)「285 行を捨てる」は設計思想と衝突し、Rust バイナリの依存を Python ツールに持ち込む。さらに open の oraios/serena#1988（言語サーバー registry）と oraios/serena#1978（tsls の cross-file wait。(a) と同じ箇所）が動いていて、09-03 の測定は既に古い |
| rust-analyzer | 「`serverStatus` に field を足せ。第 2 の通知は要らない」。`lsp-extensions.md` は「非 Rust 固有の拡張は上流化を目指すが優先度は低い」。仕様 10 章の「中継層で 2 本の流れが混ざる」は lsp-det の都合で、r-a の理由にならない                                                                                                                                                                                                                                 |
| gopls         | 「references は初期ロードを待っている。エージェントは gopls mcp を」                                                                                                                                                                                                                                                                                                                                                                                        |
| LSP 本体      | 「`$/progress` の規約で足りる」（microsoft/language-server-protocol#511 で bjorn3 が r-a に同じ答え）と「ネイティブ実装ゼロ、VS Code クライアント実装なし」。#511 の最後（2021-08）は matklad で、「serverStatus は人向け。要求の可否は `ContentModified` で扱う」                                                                                                                                                                                          |

## 4. 位置づけの説得力

- 「最初の消費者」: 相手の反論は「プロキシは消費者ではなく、ユーザーが別途入れるもの」。Zed は既に `experimental/serverStatus` を読む（`crates/project/src/lsp_store/rust_analyzer_ext.rs`）ので、r-a には消費者がいる。CC を消費者に立てられれば反論は消える
- 「乱立の実測」: 相手の反論は「乱立の多くは**サーバー自身が要求を待たせる**形で解決済み（Dart、jdtls、Sorbet、gopls、nixd。CLAUDE.md 現在地）で、通知は不要」。仕様 9 章は「クライアントが待つ」を推奨するが、**なぜサーバー側の保留より通知が良いのか**（health の区別、hover を待たせない、待つか進むかの選択権）の答えを文書に書いていない。これが LSP 本体で最初に来る反論

## 5. 規則の欠けと過剰

- 欠け: (a) **閉じた issue も含めた重複確認**（CC は stale で自動クローズし、再提出を促す）、(b) **上流の設計意図の確認**（issue 履歴。tsls #302）、(c) **上流が別の直し方をしたときの受け入れ条件**。CC が settle timer で直せば #76870 は閉じ、`tests/upstream_dev.rs` の条件では判定できない。仕様 7.x の振る舞いで書く、(d) **最新リリースでの再測定**。tsls は 08-20 に 6.0.0（Node ≥22.22.2）が出たが flake は 5.3.0、`TESTED_VERSIONS` は TypeScript 5.9.3。「上流 HEAD に rebase」だけでは足りない、(e) 仕様の `Status: draft` と「`v0.6.0` で凍結」の矛盾。提案文が指す先が draft では「安定してから来い」と言われる
- 過剰: 「fork は保守しない」は gopls の Gerrit CL では「合意後に出す」で十分。「催促 1 回」は妥当

## 6. 出す前に調べること（「未調査」の 2 点以外）

- gopls mcp の readiness の扱いと、CC ユーザーが使う vtsls / ty（#76870 の報告者。lsp-det に写像なし。basedpyright はある）
- CC の修正の方向（信号待ちか timer か）と時期。bcherny のコメントは 08-17 で、2.1.261 でも未修正（第 5 回）
- Serena #1988 / #1978 の帰趨。(a)(c) の前提
- tsls #305 の経路で SIGKILL 時に LS を落としたとき、lsp-det と CC がどう見えるか（lsp-det は上流消失、CC は #82416 Symptom 2）
- `README.md` の導入は L132 に Release バイナリの言及があるが、Nix なしの手順としては薄い（未調査 2 は妥当）

## 7. 文書の形

- 本文に決定を宣言的に置くのは適切。ただし「催促 1 回」「fork は保守しない」は却下案を持つ決定で、ユーザー自身の方針（`record-rejected-decisions`）なら根拠は ADR に置き、本文は規則だけにする
- 一覧表は Nextflow〜nil の行で「未着手（0.5.0 の後）」が根拠列に入り、状態列が空（列ずれ）。提出物の索引として直してから外部に見せる

## 最悪の展開と、避けるための変更

**展開**: tsls が「#305 を直せ」で閉じ、CC の #82416 コメントが閉じた PR を指す。CC が timer で直して #76870 を閉じ、lsp-det の角度が消える。Serena は #1978/#1988 を merge して (a)(c) が陳腐化。r-a は「field を足せ」、gopls は「待っている、mcp を」、LSP は #511 の隣で backlog。7 件が**プロトコルそのものと無関係な理由**で全滅し、`experimental/` を話すのは永久に lsp-det だけになる。

**変更**: (1) tsls は #305 の取りこぼし修正として出し直す。(2) 第 1 段の主役を CC にし、#76870 に「待つべき信号 = `experimental/serverState`。lsp-det が今日の橋」を提案。didClose と configuration は既存 issue へのコメントに変える。(3) gopls は `health` の issue に縮め、awaitLoaded と mcp を先に認める。(4) r-a には「serverStatus への field 追加」案を自分から並べ、どちらでも受けると書く（仕様変更なら ADR 0006 決定 4 の再検討。ユーザーの承認事項）。(5) LSP は `.proposed.ts` を用意し、「サーバー側の保留ではなく通知である理由」を仕様か提案文に足す。(6) 規則に 5 節の (a)〜(e) を足す。

---

## 採否（2026-09-09、ユーザーの決定。PR #80）

作者側で事実を確かめたもの: tsls の `onExit` が `exitCode` しか見ないこと、anthropics/claude-code#76870 の作者のコメント、#64276 の NOT_PLANNED、#16360 の存在、oraios/serena#1978 / #1988 が open であること、gopls の `references` が初期ロードを待つこと（`docs/research/gopls-readiness-measurement.md` の実測とも一致。起動直後の無言の嘘は主張していなかった）。

| 指摘                                                       | 採否                                                                                                                                     |
| ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| tsls は #305 の取りこぼし修正として出し直す                | 採用。fork の `RequestFailed` 経路は却下した案に                                                                                         |
| Claude Code を消費者に立て、#76870 に信号を提案する        | 採用。消費者の順を Claude Code → lsp-det → Serena に。新規 issue は `shutdown` の 1 件だけに縮め、残りは既存 issue へのコメント          |
| gopls は `health` に縮める                                 | 採用。`awaitLoaded` と `gopls mcp` を先に認める                                                                                          |
| rust-analyzer に `serverStatus` への field 追加案を並べる  | 採用。両案を並べ、相手が選んだ方を出す。既存の語彙を消させる必要はなく「良い方が流行るだけ」（ユーザー）。仕様は写像で読めるので変えない |
| LSP 本体向けに `.proposed.ts` と「通知にする理由」         | 採用。提出前の準備に入れ、「通知にする理由」は仕様を安定版にする作業の中で書く                                                           |
| 規則の欠け (a)〜(e)                                        | 採用。閉じた issue の重複確認、設計意図の確認、受け入れ条件を仕様の振る舞いで書く、最新リリースでの再測定、仕様を安定版にしてから出す    |
| 「fork は保守しない」は過剰                                | 一部採用。「提出の直前にだけ追従させる」に言い換えた                                                                                     |
| Serena の (c) は設計思想と衝突                             | 採用。捨てさせるのではなく oraios/serena#1988 の registry に lsp-det を外部実装として載せる形に変えた                                    |
| 却下案の根拠は ADR に                                      | 不採用。戦略は設計変更ではないので、却下した案は本文書の末尾の表に置く（ユーザーはどこでもよいとした）                                   |
| 一覧表の列ずれ                                             | 採用。12 行を直した                                                                                                                      |
| 調べること（vtsls / ty、CC の修正の方向、Serena の帰趨等） | 採用。「未調査」に足した                                                                                                                 |
