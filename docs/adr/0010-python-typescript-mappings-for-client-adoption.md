# ADR 0010: v0.2 は Python と TypeScript の写像を足し、Serena を下流の被験者にする

- 日付: 2026-09-03
- 状態: 採用
- 関連: [ADR 0001](0001-tool-first-readiness-gate.md)、[ADR 0009](0009-success-criterion-and-two-sided-reference.md)、[research/server-readiness.md](../research/server-readiness.md)、[research/serena-solidlsp.md](../research/serena-solidlsp.md)、[vision.md 5 章](../vision.md)

## 経緯

v0.1 の成功基準（ADR 0009 決定 A-2）は満たした。仕様・上流側と下流側の準拠テスト・両側の参照実装が自己無矛盾で、rust-analyzer 1.98.0 / 2026-08-03 と gopls v0.23.0 に当てて通っている。Claude Code を被験者にしたドッグフーディングも 3 回行い（[research/claude-code-dogfooding.md](../research/claude-code-dogfooding.md)）、設計判断を変える事実は出なかった。

次段階は ADR 0009 決定 A-3 のとおり順序付きで、最終目標は LSP 本体への提案である。vision 5 章の経路は「公開 → 各上流に実測付きの issue → LSP 本体に提案」だが、その前段として**クライアント（エージェント）に上流側の写像を取り込んでもらう**ことが提案の重みになる。ここで問題になるのは写像の言語の範囲である。

- Serena も Claude Code も 1 クライアントで全言語を扱う。写像が Rust と Go だけでは、クライアントにとって「取り込む理由」にならない
- Serena の readiness 判定は、Python（pyright / basedpyright）がログの正規表現 `Found \d+ source files?` と 60 秒の打ち切り、TypeScript が `$/typescriptVersion` 通知と 10 秒・30 秒の打ち切りである（research/serena-solidlsp.md）。本プロトコルが消す対象そのものであり、写像に置き換えれば消える行数を実測で示せる
- Serena は rust・python・typescript なら `ls_base_cmd` で起動コマンドを公式に差し替えられ、go は差し替えられない（v0.1-design 9 章）。Python と TypeScript を選ぶと、凍結していた Serena 統合がそのまま次の被験者になる
- pyright と typescript-language-server の readiness の表出は調査済みで（research/server-readiness.md）、どちらも gopls と同じ「`$/progress` 由来の写像」として書ける

## 決定

### A. 範囲

1. v0.1 は現在の main で完了とする。以後の作業は v0.2 の範囲とし、v0.1 設計書（[v0.1-design.md](../v0.1-design.md)）の 8 章にマイルストーンを追記する形で進める。仕様（[spec/server-state.ja.md](../spec/server-state.ja.md)）は変更しない
2. v0.2 の範囲は、**pyright の写像**、**typescript-language-server の写像**、**Serena を下流の被験者にした観測**の 3 つである
3. 宣言範囲と起動方法の宣言は凍結のまま（ADR 0001 決定 2）。jdtls・clangd の写像は範囲外。提案を 1 つに絞る
4. rust-analyzer と gopls への上流 issue（vision 5 章 2）は v0.2 の完了を待たずに出してよい。ただし相手が読める形（README と仕様の英訳）が先に要る。これは v0.2 のマイルストーンとは独立の小さい作業として扱う

### B. マイルストーン

- **M5 — pyright の写像**: `$/progress`（`window.workDoneProgress` を宣言したときの経路。lsp-det は無条件に注入済み）の begin / end から readiness を合成する。end は「残り解析ファイル数が 0」と厳密に一致する（research/server-readiness.md 1.1）。basedpyright は pyright の通知名を継承しているので、写像は共有し `serverInfo.name` で両方を選ぶ。保証は実サーバーで 7.2 / 7.3 を通した版にだけ宣言する（仕様 10 章の見込みは completeness のみ。freshness は測ってから決める）
- **M6 — typescript-language-server の写像**: `$/progress`（title "Initializing JS/TS language features…"）の begin / end から readiness を、`window/logMessage`（error）の "Exited. Code:" から health を合成する。tsserver のクラッシュでも progress の end が来る（research/server-readiness.md 2.2）ので、end 単独で `ready` にしてよいのは health が `error` でないときだけ。仕様 10 章の見込みは completeness のみ
- **M7 — Serena 統合**: Serena の `ls_base_cmd` を lsp-det 経由に向け、Serena 側の打ち切り（60 秒 / 10 秒 / 30 秒）と下流側の保留の相互作用を観測する。観測結果は research に記録し、Serena に「写像で消える行数」を示す材料にする

各マイルストーンは M4 と同じ手順で進める。flake.nix にサーバーを足す → 実測記録（gopls と同じ形式）→ RED → GREEN → REFACTOR → 実サーバーの結合テスト（`#[ignore]`）→ `TESTED_VERSIONS`。

### C. 引き継ぐ制約

1. **時間に基づく判定を持たない**（仕様 6 章 6 項、ADR 0009 決定 C-4）。research/server-readiness.md 4.2 の「end の直後に一定時間エラーログや接続断がないことを確認してから ready に遷移させる」という示唆は**採らない**。typescript-language-server のクラッシュは、`window/logMessage` が progress の end より先に来るならそれで health を `error` にし、来ないなら接続の終了（EOF）で伝わる（ADR 0009 決定 C-3、D-9）。どちらの順で来るかは M6 の実測で確かめ、その結果に基づいて写像を書く
2. **保証は準拠テストを通した版の名乗りの一覧でのみ宣言する**（ADR 0009 決定 D-5）。仕様 10 章の「見込み」は宣言ではない
3. **小規模ワークスペースで progress が一度も出ない**（pyright、research/server-readiness.md 1.1 の注意点）場合、写像は `initializing` のまま `ready` に進めない可能性がある。これは写像の欠陥ではなく pyright の語彙の限界であり、gopls の go.mod 変更時と同じく「サーバー本体が本プロトコルを話すことでしか埋まらない」として記録する。準拠テストの fixture は progress が観測できる規模にする（仕様 7 章冒頭）
4. 造語を作らない（ADR 0009 決定 B）

### 却下した案

| 候補                                                                    | 不採用の理由                                                                                                                                                                     |
| ----------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 先に rust-analyzer と gopls への上流 issue だけを進める                 | 言語サーバー側の採用とクライアント側の採用は独立に進められ、どちらも提案の重みになる。写像が 2 言語のままではクライアントに取り込む理由がない。issue は A-4 のとおり並行してよい |
| Serena 統合を go から始める                                             | Serena は go の起動コマンドを差し替えられず、PATH の偽装と `version` CLI 応答の偽装が要る（v0.1-design 8 章 M5 旧記述）。python / typescript なら公式の設定だけで済む            |
| jdtls も同時に足す                                                      | 専用通知 `language/status` を持ち写像は容易だが、エージェントの主要言語ではなく Serena の `ls_base_cmd` 対象でもない。提案を薄めないために範囲外                                 |
| typescript-language-server のクラッシュ判定に「end 後の猶予時間」を置く | 時間判定の禁止（C-1）。信号の順序は実測で確かめるものであり、時間で代用すると嘘に戻る                                                                                            |
| 写像を足す前に宣言範囲の凍結を解く                                      | 提案は 1 つに絞る（A-3）。宣言範囲は別の issue になる（vision 5 章 3）                                                                                                           |
| Claude Code の公式プラグインに lsp-det を組み込む提案を先に出す         | Claude Code は Python / TypeScript のプラグインを持つので、写像がその 2 言語を覆ってからでないと提案にならない。M5 / M6 の後                                                     |

## 影響

### 設計（docs/v0.1-design.md）

- 8 章の M5 を本 ADR の M5〜M7 に置き換える
- 5.3「写像の将来」の pyright / typescript-language-server の記述は本 ADR の B と C-1 に合わせる

### CLAUDE.md

- 現在地の M5 の行を M5〜M7 に置き換える

### 開発環境

- M5 で flake.nix に pyright（と basedpyright）を、M6 で typescript-language-server と typescript、nodejs を足す。版は flake.lock が固定するものを `TESTED_VERSIONS` に載せる

### 仕様

- 変更なし。10 章の pyright / tsserver 系の行は、M5 / M6 で実サーバーの準拠テストを通したときに「見込み」から「確認済み」に書き換える

### 上流提案への影響

- クライアント側の採用の根拠（Serena で消える行数）が M7 で得られる
- README と仕様の英訳（A-4）は v0.2 の前提ではないが、上流 issue の前提である
