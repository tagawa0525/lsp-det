# 上流への提出

lsp-det の最終目標は、サーバー状態プロトコルを言語サーバーとクライアントの本体に入れ、最後に LSP 本体へ提案することである。本文書はそのための外向きの提出（PR、issue、報告）の戦略と一覧。**いずれもユーザーの確認をもらってから出す**。fork の準備と受け入れ条件の回し方は [scripts/upstream/README.ja.md](../scripts/upstream/README.ja.md)。この戦略は別のセッションによる批判的レビューを経ていて、レビューの全文と採否は [research/upstream-strategy-review-2026-09.md](research/upstream-strategy-review-2026-09.md)。

## 位置づけ

- **消費者を先に立てる**。プロトコルは、クライアントが `experimental.serverState` を宣言して待つことで初めて価値が出る。サーバーの上流に「話してほしい」と頼むときの「誰が読むのか」への答えは次の 3 つで、この順に立てる
  1. **Claude Code**。[#76870](https://github.com/anthropics/claude-code/issues/76870) で作者が起動直後の不完全な結果を再現し、「サーバーの索引の信号を待つか、bounded retry か」を探している。待つべき信号として `experimental/serverState` を提案し、lsp-det を今日使える橋として添える
  2. **lsp-det**。今日から読み、宣言しないクライアントの代わりに要求を保留する
  3. **Serena**。自前の readiness 判定を捨てさせるのではなく、[#1988](https://github.com/oraios/serena/pull/1988) の言語サーバー registry に lsp-det を外部実装として載せる形で繋ぐ
- **LSP 本体への根拠は乱立の実測**。上流のどれかが取り込むのを待たない。根拠は [research/readiness-vocabulary-corpus.md](research/readiness-vocabulary-corpus.md) と仕様 10 章の対応表で、20 を超えるサーバーが同じもの（索引の完了、壊れ、編集の取り込み）を別々の語彙で作っている事実である。rust-analyzer の `experimental/serverStatus`、jdtls の `language/status`、Sorbet の `sorbet/showOperation`、Dart の `$/progress` の `ANALYZING`、clangd の `backgroundIndexProgress`。[#511](https://github.com/microsoft/language-server-protocol/issues/511) は 2021 年から止まっているので、エージェント用途と実測という新しい角度で再開する
- **上流は未試行だが、既に別の答えを持っていることがある**（ADR 0018 決定 A-6 の補足）。readiness の提案を上流が却下したのではなく、誰も出していない。一方で、上流が同じ問題に別の形で答えている場合がある。typescript-language-server は tsserver が死んだら LS 本体を落とす設計（typescript-language-server/typescript-language-server#302、typescript-language-server/typescript-language-server#305）、gopls は `references` が初期ロードを待つ（`awaitLoaded`）うえに自前の MCP サーバー（`gopls mcp`）を持つ、rust-analyzer は readiness の要望を「`serverStatus` で既にある」と閉じている（rust-lang/rust-analyzer#10888）。提出はその答えの上に載せる
- **相手の直し方を否定しない**。lsp-det の写像や別の通知は「良い方が流行る」だけのもので、既存の語彙を消させる必要はない。rust-analyzer には `serverStatus` への field 追加と別通知の両案を並べ、どちらでも受けると書く

## 順序の原則

1. **相手の痛みが先、提案は後**。相手が既に感じている不具合の修正を先に出し、プロトコルの提案はその一般化として後に出す。単発の指摘は、自分たちが直したい箇所の PR と同じ機会に出す
2. **相手の設計に沿う最短の直し方で出す**。tsls の tsserver 死後の空応答は、`RequestFailed` の新しい経路ではなく、typescript-language-server/typescript-language-server#305 が `signal` を見ていない取りこぼしの修正として出す
3. **消費者を先に立てる**。Claude Code への提案と Serena の registry への接続を、rust-analyzer と gopls への提出より前に置く
4. **小さい PR は独立に出す**。`InitializeResult.serverInfo` を返すだけの PR は他の何にも依存しないので、相手の反応を待たずに出せる。ただし同じ上流には一度に 1 本ずつ

## 規則

- **仕様を安定版にしてから出す**。提出物が指す仕様は `Status: draft` のままにしない。安定版にする前に、仕様（または提案文）に「サーバー自身が要求を待たせるのではなく通知にする理由」（health の区別、待たせない要求との共存、待つか進むかの選択権がクライアントにある）を書き、保留中の決定（nil の (a) / (b)）を閉じる。安定版の tag を提出物が指し、提出の後に仕様を変えるなら版を上げ、出したものには変更点を追記する
- **重複を閉じた issue まで確認する**。Claude Code は stale で自動クローズし再提出を促す運用なので、閉じた issue（`didClose` の anthropics/claude-code#64276 は NOT_PLANNED）も含めて探し、既存があればコメントにする
- **上流の設計意図を issue の履歴で確認する**。同じ症状に上流が既に答えを出していれば（tsls typescript-language-server/typescript-language-server#302 / typescript-language-server/typescript-language-server#305）、その答えの上に載せる
- **出す直前に上流 HEAD と最新リリースで再測定する**。測定の記録は書いた時点の版に対するもので、動きの速い上流（Serena、tsls 6.0.0）では提出時に前提が崩れていることがある。fork のパッチは上流 HEAD に rebase して受け入れ条件（`tests/upstream_dev.rs`）を通し直してから出す
- **受け入れ条件は仕様の振る舞いで書く**。上流が別の直し方（Claude Code の settle timer など）をしても判定できるよう、`tests/upstream_dev.rs` の条件は「この実装がこの通知を出す」ではなく仕様 7 章の振る舞いで書く
- **再現を最初から添える**。gopls は情報不足の issue を凍結する運用なので、fixture と実測ログを最初から添える。他の上流も同じにする
- **手続きの前提を先に済ませる**。gopls は golang/go に issue を立て、CL は Gerrit で Google CLA が要る。pyright は CONTRIBUTING が「新機能は先に enhancement request」なので issue から。Serena も小さな修正以外は issue から。rust-analyzer は `lsp-extensions.md` の hash の更新と `cargo xtask tidy`。LSP 本体は issue → `vscode-languageserver-node` の `proposed.<name>.ts`（メタモデル生成器はこの名前か JSDoc の `@proposed` で「提案中」と印を付ける。3.17 の開発時の `proposed.diagnostic.ts` / `proposed.typeHierarchy.ts` と同名の `.md` が前例）→ 仕様への PR の順
- **催促は 1 回まで**。返事がなくても 2 週間おいて 1 度だけ確認し、それ以上は追わない。却下されたら写像で吸収し、fork のブランチは閉じる
- **fork は提出の直前にだけ追従させる**。取り込まれなかったパッチを維持しない
- **文面は英語で、ユーザーの確認をもらってから出す**。出したら一覧表の状態を更新する

## 提出前の準備

1. 仕様を安定版にする（上の規則。「通知にする理由」の記述、nil の (a) / (b)、`Status`、tag）
2. tsls のパッチを typescript-language-server/typescript-language-server#305 の取りこぼし修正に作り直し、6.0.0 で再測定する。通ると tsls は SIGKILL でも落ちるので、lsp-det は上流消失として扱う（仕様 8 章）。anthropics/claude-code#82416 の Symptom 1（tsserver は生きているが応答しない）は対象外と明記する。**済（2026-09-09）**: 修正は `onExit` の `if (exitCode)` を外す形（tsserver 自身の shutdown は #585 以来 `onExit` に届かないので、条件の理由が消えていた）。fork の `tsserver-exit-by-signal`、fork の CI 3 OS で確認。再測定と経緯は [research/typescript-language-server-readiness-measurement.md](research/typescript-language-server-readiness-measurement.md)
3. gopls の提出を health（"Error loading workspace" をクライアントに見える信号にする）と go.mod 変更後の再ロード窓に縮める。`awaitLoaded` と `gopls mcp` を先に認める。**済（2026-09-09）**: 実測（[research/gopls-health-measurement.md](research/gopls-health-measurement.md)）で、失敗中の要求は明示的なエラー、信号は `window.workDoneProgress` のないクライアントに type 4（Log）の showMessage に落ちる、健康なセッションの go.mod 変更に窓はないが回復後は約 1 秒の窓がある、と分かった。提出は golang/go#78273 への severity のコメントと、窓の新規 issue の 2 件に決めた
4. rust-analyzer に `serverStatus` への field 追加案を用意する（別通知の案と並べる）。**済（2026-09-09）**: fork の `server-status-readiness`（`ServerStatusParams` に `readiness` を足し、初期の `last_reported_status` を `initializing` に。`lsp-extensions.md` の「人向け」の注記に、この field だけは答えを信じてよいかを決めるクライアント向けと書く）。`server-state` と同じ起点。実測（[research/rust-analyzer-quiescent-measurement.md](research/rust-analyzer-quiescent-measurement.md) の末尾）で、Cargo.toml のないディレクトリでは最初のロードの前に `{warning, quiescent: true}` が送られる（自明な静穏）ことと、field 版では `initializing` → `indexing` → `ready` が観測できることを確認。lsp-det の写像は field があればそれを読む
5. LSP 本体向けに `vscode-languageserver-node` の `proposed.serverState.ts` を用意する。**済（2026-09-09）**: fork `tagawa0525/vscode-languageserver-node` の `server-state` に、`protocol/src/common/proposed.serverState.ts`（型、`workspace/serverState` 要求、`workspace/serverStateChanged` 通知、`$ServerStateClientCapabilities` / `$ServerStateServerCapabilities`）、同名の `.md`（LSP 仕様の書式の本文。3.17 の `proposed.typeHierarchy.md` の型）、`api.ts` の `Proposed` 名前空間、再生成した `metaModel.json`（新しい項目は `proposed: true`）を用意。方法名は仕様 4.3 の採用後の名前、capability はクライアントが `workspace.serverState: boolean`（`workspace.configuration` 等と同じ形）、サーバーが `serverStateProvider: ServerStateOptions`。`@since 3.19.0 - proposed state`。`compile:protocol`、`lint`、`test:node`、`generate:metaModel` が通る（このリポジトリに GitHub Actions はなく、fork の CI では確かめられない）
6. Serena を上流 HEAD（oraios/serena#1978、oraios/serena#1988 の帰趨を含む）で再測定する。**済（2026-09-09）**: HEAD `701e7c84` で前回と同じ結果（[research/serena-integration-measurement.md](research/serena-integration-measurement.md) の末尾）。(a) クラッシュ検知が 2 回目以降の横断要求に効かない穴は HEAD にも #1978（まだ OPEN）にも残り、修正は fork の `tsserver-crash-on-request-path`（latch の前で `_raise_if_crashed()`。1 行とテスト 1 件。#1978 の上でも成立）。(b) の 4 件は HEAD に残る（所在の行番号は同報告）。#1988（まだ OPEN）の registry は `SolidLanguageServer` の子クラスを entry point で登録する形で、lsp-det は組み込みの adapter を継承する Python パッケージとして載る
7. README に Nix を使わない導入手順（Release のバイナリ）があるか点検する

## 段階

第 1 段（準備の 1、2、7 の後。相互に依存しない）:

1. typescript-language-server: typescript-language-server/typescript-language-server#305 の取りこぼし修正の PR（typescript-language-server/typescript-language-server#1125 として提出済み、2026-09-09。Copilot の指摘 2 件（同期の throw が exit handler の残りを飛ばす、テストの private 連鎖に実行時チェック）には fork の 6c21094 で同日対応済み。[research/typescript-language-server-readiness-measurement.md](research/typescript-language-server-readiness-measurement.md) の末尾）。続けて `server-info` の PR
2. Claude Code: anthropics/claude-code#76870 に「待つべき信号は `experimental/serverState`。lsp-det が今日の橋」を提案し、第 1〜7 回の実測を添える。anthropics/claude-code#82416 に tsls の PR へのリンクと再現。anthropics/claude-code#85225 に第 5 回の観測。`didClose` は anthropics/claude-code#64276、`workspace/configuration` は anthropics/claude-code#16360 へのコメント。新規は `shutdown` の `params: {}` の 1 件
3. pyright: `serverInfo` の enhancement request（issue）。PR は返事の後

第 2 段（準備の 3〜6 の後。第 1 段の反応を待たない）:

1. Serena: 再測定の結果が残る不具合の issue と、registry に lsp-det を載せる提案
2. rust-analyzer: issue で両案（`serverStatus` への field 追加、別通知の `experimental/serverState`）を並べる。PR は相手が選んだ方を出す
3. gopls: golang/go#78273 に fallback の severity のコメント、回復後の go.mod 変更の窓は新規 issue。fixture と実測ログ付き（2026-09-09 に提出済み: golang/go#78273 のコメントと golang/go#81400。付いた反応と返信は「gopls: 提出後の反応」）

第 3 段（第 2 段のどれかに反応があってから）:

1. 12 サーバー: `serverInfo` を返すだけの PR（Nextflow、haskell-language-server、crystalline、Gleam、haxe-language-server、Sorbet）。不具合の issue（Gleam の `references` が空になる、nixd の SIGPIPE、jdtls の status）は再現付きで。readiness の信号の提案は LSP 本体の proposal へのリンクを添えて出す
2. LSP 本体: microsoft/language-server-protocol#511 にコメントし、新規の proposal issue を立てる。根拠はコーパスと 10 章の対応表、`proposed.serverState.ts`

## 未調査

- Claude Code 以外のエージェント（Cursor、Codex CLI、Cline、OpenCode、Zed）が LSP をどう叩くか。同じ穴があれば報告先と提案の聴衆が広がる。Zed は `experimental/serverStatus` を既に読む
- Claude Code のユーザーが使う vtsls と ty（anthropics/claude-code#76870 の報告者の環境）。lsp-det に写像がない
- Claude Code の修正の方向（信号を待つか timer か）と時期
- ~~`gopls mcp` が readiness をどう扱うか~~ → `find_references` は `golang.References` を直接呼び、LSP と同じスナップショットの API（`awaitLoaded`）を通る（[research/gopls-health-measurement.md](research/gopls-health-measurement.md)）

## 却下した案

| 案                                                             | 却下理由                                                                                                                                               |
| -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| tsls に `RequestFailed` の経路を足す PR（fork に用意したもの） | メンテナは typescript-language-server/typescript-language-server#302 で「tsserver が死んだら LS 本体を落とす」と決めている。設計と違う直し方は通らない |
| Claude Code に新規 issue 3 件                                  | `didClose` と `workspace/configuration` は既存があり、重複は読まれない                                                                                 |
| Serena に自前の readiness 判定を捨てさせる提案                 | Serena の流儀は「サーバーごとの待ちを足す」で、設計思想と衝突する。registry への接続に変える                                                           |
| rust-analyzer に別通知の `experimental/serverState` だけを出す | rust-lang/rust-analyzer#10888 を「`serverStatus` で既にある」と閉じている。field 追加案を並べないと「field を足せ」で終わる                            |
| gopls にサーバー状態プロトコルを話す CL                        | `references` は初期ロードを待っていて、起動直後の無言の嘘がない。health に縮める                                                                       |
| LSP 本体は上流のどれかが取り込んでから                         | 根拠は取り込みではなく乱立の実測。待つ理由がない                                                                                                       |
| 仕様を draft のまま出す                                        | 「安定してから来い」で止まる                                                                                                                           |
| 戦略を ADR に書く                                              | 戦略は設計変更ではない。本文書の本文に決定を宣言的に書く                                                                                               |

## 一覧

| 提出先                     | 内容                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         | 根拠                                                                                                                                                                                                                                                                                                                               | 前提                                                                                                            | 状態                                                                                                                                                                                                                                                                     |
| -------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| typescript-language-server | tsserver が SIGKILL 等の signal で死ぬと `exitCode` が null で LS 本体が落ちず（`onExit` の `if (exitCode)`。typescript-language-server/typescript-language-server#305 の後、#624 で残った条件）、以後の要求に `NoServer` の空応答（`references` は `[]`）を返す取りこぼしを、条件を外して常に落とすように直す PR。通ると typescript-language-server/typescript-language-server#302 の設計どおり LS 本体が落ち、クライアントが再起動する。Node 自身の OOM も `{code: null, signal: 'SIGABRT'}` なので同じ穴。anthropics/claude-code#82416 の Symptom 1（tsserver は生きているが応答しない）は対象外                                                                                                                                                                          | [research/typescript-language-server-readiness-measurement.md](research/typescript-language-server-readiness-measurement.md)（5.3.0 と 6.0.0 で SIGKILL 後の references が `[]`。取りこぼしの経緯）、[research/serena-integration-measurement.md](research/serena-integration-measurement.md)                                      | 第 1 段。準備 2 は済                                                                                            | **提出済み**（typescript-language-server/typescript-language-server#1125、2026-09-09。fork `tsserver-exit-by-signal`。受け入れ条件と fork の CI 3 OS 通過）。応答待ち                                                                                                    |
| Claude Code                | (1) anthropics/claude-code#76870 に、待つべき信号として `experimental/serverState` を提案し、第 1〜7 回の実測（rust-analyzer で 6ms 後の空配列、tsls で宣言 1 件だけの応答）と lsp-det を今日の橋として添える。(2) anthropics/claude-code#82416 に tsls の PR へのリンクと再現。(3) anthropics/claude-code#85225 に第 5 回の観測（`didChangeWatchedFiles` を宣言も送信もせず、gopls / pyright では Bash の編集がセッション中ずっと見えない）。(4) `didClose` を送らない → anthropics/claude-code#64276（NOT_PLANNED）にコメント、`workspace/configuration` を支持しない → anthropics/claude-code#16360 にコメント。(5) 新規 1 件: `shutdown` の `params: {}` を rust-analyzer が拒み `exit` を送らない。Write の再 `didOpen`（2.1.259）は 2.1.261 で直っているので報告しない | [research/claude-code-dogfooding.ja.md](research/claude-code-dogfooding.ja.md)、[research/serena-processing-around-lsp.md](research/serena-processing-around-lsp.md)、第 5 回（2026-09-06、CC 2.1.261）の lsp-det あり／なしの比較と、第 6 回の実害の一事例（直接では tsls と gopls の両方で使われている関数を消しビルドが壊れる） | 第 1 段。準備 1、7                                                                                              | 未着手                                                                                                                                                                                                                                                                   |
| Serena                     | (a) クラッシュ検知（PR oraios/serena#1848）がリクエスト経路に効いていない不具合の PR。(b) 不具合 4 件の issue: 打ち切りが素の `TimeoutError` で再起動の経路に乗らない、書き込み失敗の握りつぶし、再 open の全文 `didChange` が `version` を増やさない、hover の予算切れをエージェントに伝えない。(c) 言語サーバー registry（oraios/serena#1988）に lsp-det を外部実装として載せる提案。自前の readiness 判定を捨てさせない                                                                                                                                                                                                                                                                                                                                                   | [research/serena-integration-measurement.md](research/serena-integration-measurement.md)、[research/serena-processing-around-lsp.md](research/serena-processing-around-lsp.md) 8 章                                                                                                                                                | 第 2 段。準備 6 は済                                                                                            | fork に (a) の修正を用意済み（`tsserver-crash-on-request-path`。受け入れ条件は `scripts/serena/probe.py` の `CRASH=1 VIA_LSP_DET=0` が `TypeScriptServerCrashedError` になること）。(b) の 4 件は HEAD `701e7c84` に残る。草案は下。提出は #1978 の帰趨を見てから rebase |
| pyright                    | (a) `InitializeResult.serverInfo` を返す enhancement request（issue）の後に PR（fork `tagawa0525/pyright` の `server-info`）。(b) ファイル列挙の再開を `window/logMessage` に出す（または本プロトコルを話す）変更。Created / Deleted の通知の直後の問い合わせが古い答えを返す窓（約 0.04 秒）を観測者が埋められるようにする（ADR 0016 で `freshness.fileChanges` が `["Changed"]` にとどまる理由）                                                                                                                                                                                                                                                                                                                                                                           | ADR 0011 決定 C                                                                                                                                                                                                                                                                                                                    | 第 1 段。CONTRIBUTING が「新機能は先に enhancement request」                                                    | fork に用意済み。受け入れ条件は通過                                                                                                                                                                                                                                      |
| typescript-language-server | (a) 同上（fork `tagawa0525/typescript-language-server` の `server-info`）。(b) `useClientFileWatcher` で Created が同期になるかを測ったうえで、Created / Deleted の取り込み完了を伝える手段の提案（ADR 0016 で `freshness.fileChanges` が `["Changed"]` にとどまる理由）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | 同上                                                                                                                                                                                                                                                                                                                               | 第 1 段。不具合修正の PR の後                                                                                   | fork に用意済み。受け入れ条件は通過                                                                                                                                                                                                                                      |
| Nextflow の言語サーバー    | (a) `InitializeResult.serverInfo`（名前と版）を返す PR。名乗りが `executeCommandProvider.commands` にしかなく、版はどこにも現れない。(b) 設定差分のない `initialized` でもサービスを初期化し、ワークスペースの走査を `$/progress` に出す提案。走査の完了を示す信号がなく、観測者はファイル集合を再現するしかない                                                                                                                                                                                                                                                                                                                                                                                                                                                             | [research/nextflow-readiness-measurement.md](research/nextflow-readiness-measurement.md)                                                                                                                                                                                                                                           | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| haskell-language-server    | (a) `InitializeResult.serverInfo` を返す PR。名乗りが pid 前置の `executeCommandProvider.commands` にしかない。(b) 索引の完了を伝える信号の提案。`$/progress` は `optProgressStartDelay` の 1 秒で抑制され、kick と索引バッチごとに作り直されるので readiness に使えず、索引中の `references` は増え続ける部分応答になる。`--test` の `kick/done` / `ghcide/reference/ready` に総数を足して通常モードでも送る、または本プロトコルを話す                                                                                                                                                                                                                                                                                                                                      | [research/haskell-language-server-readiness-measurement.md](research/haskell-language-server-readiness-measurement.md)                                                                                                                                                                                                             | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| pyrefly                    | 起動時の索引（`populate_all_workspaces_files` / `populate_all_project_files_in_config`）を "Pyrefly: Rechecking" と同じ `$/progress` の機構（`LspProgressSubscriber`）に繋ぐ提案。索引の開始と終了が stderr にしか出ず、索引の前は `[]`、途中は部分応答になる。設定の壊れを `window/showMessage` に出す                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | [research/pyrefly-readiness-measurement.md](research/pyrefly-readiness-measurement.md)                                                                                                                                                                                                                                             | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| crystalline                | (a) `InitializeResult.serverInfo` を返す PR。名乗りが起動ログ "[workspace] Found projects:" にしかなく、版はどこにも現れない。(b) コンパイルが失敗した要求をエラー応答にする提案。今は definition などが空配列で答え、コンパイルできないのか見つからないのかエージェントに区別がつかない。(c) `didChangeWatchedFiles` の登録と結果キャッシュの無効化。開いていないファイルのディスク上の変更が織り込まれない                                                                                                                                                                                                                                                                                                                                                                 | [research/crystalline-readiness-measurement.md](research/crystalline-readiness-measurement.md)                                                                                                                                                                                                                                     | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| Gleam                      | (a) `InitializeResult.serverInfo` を返す PR。名乗りが `$/progress` の依存ダウンロードのトークンにしかない。(b) `gleam.toml` の変更（`workspace/didChangeWatchedFiles`）でエンジンを作り直した後 `references` が空になる不具合の修正。(c) コンパイルの開始と終了（`compilation_started` / `compilation_finished`。現状 no-op）を `$/progress` に出す提案                                                                                                                                                                                                                                                                                                                                                                                                                      | [research/gleam-readiness-measurement.md](research/gleam-readiness-measurement.md)                                                                                                                                                                                                                                                 | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| haxe-language-server       | (a) `InitializeResult.serverInfo` を返す PR。名乗りが `workspace/didChangeConfiguration` の後の `window/logMessage`（"Haxe Path: "）にしかない。(b) 設定を送らないクライアントでもコンパイラを起動する提案。それまでは `references` 等が `-32601` のまま。(c) 開いている文書の `didChange` を他ファイルの `references` に織り込む提案（測定では `didSave` だけが反映される）                                                                                                                                                                                                                                                                                                                                                                                                 | [research/haxe-language-server-readiness-measurement.md](research/haxe-language-server-readiness-measurement.md)                                                                                                                                                                                                                   | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| Dart analysis server       | `workspace/didChangeWatchedFiles` を黙って無視する（クライアントが送るのは LSP では登録の後だが、登録しないサーバーに送るクライアントは珍しくなく、今は type 1 の `window/showMessage`（"Unknown method workspace/didChangeWatchedFiles"）がエラーとして見える。サーバー自身のファイル監視で取り込みには効いているので、副作用は表示だけ）                                                                                                                                                                                                                                                                                                                                                                                                                                   | [research/dart-readiness-measurement.md](research/dart-readiness-measurement.md)                                                                                                                                                                                                                                                   | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| Sorbet                     | (a) `InitializeResult.serverInfo` を返す PR。名乗りが `sorbet/showOperation` の通知そのものにしかなく、版はどこにも現れない。(b) `subscribe` の前に `watch-project` を発行する（または文書に「root が既に watch されていること」を明記する）提案。今は watch されていない root に `subscribe` しても watchman の `RootResolveError` が stderr に出るだけで、`lsp.md` の「`.git` か `.watchmanconfig` があればよい」という記述が成り立たない。(c) `workspace/didChangeWatchedFiles` を登録して読む提案。今は登録も読みもせず、watchman がない環境ではディスク上の変更が一切取り込まれない                                                                                                                                                                                     | [research/sorbet-readiness-measurement.md](research/sorbet-readiness-measurement.md)                                                                                                                                                                                                                                               | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| jdtls                      | ビルドの完了後にも `ProjectsManager.reportProjectsStatus` を呼ぶ提案。現状はプロジェクトの取り込み直後に一度だけ呼ばれ、ビルドが問題マーカーを付ける前なので、壊れた classpath（存在しない jar）があっても `language/status` の `ProjectStatus` は "OK" のままになる。壊れは `textDocument/publishDiagnostics`（プロジェクト自身の URI）にしか出ない（201 クラスの被験体で実測）                                                                                                                                                                                                                                                                                                                                                                                             | [research/jdtls-readiness-measurement.md](research/jdtls-readiness-measurement.md)                                                                                                                                                                                                                                                 | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| clangd                     | (a) 索引中の横断リクエストを待たせるか、部分応答であることを示す提案。現状は begin〜end の間 `references` が空から増え続ける部分応答（無言の嘘）。(b) `workspace/didChangeWatchedFiles` を登録して読み、ディスク上の変更を再索引する提案。現状は登録せず、送っても効かない。(c) `compile_commands.json` がないときにそれを伝える通知の提案。現状は信号が出ず、観測者には「データベースがない」と「まだ begin が来ていない」の区別がつかない（402 ファイルの被験体で実測。[research/clangd-readiness-measurement.md](research/clangd-readiness-measurement.md)）                                                                                                                                                                                                              |                                                                                                                                                                                                                                                                                                                                    | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| nixd                       | (a) 評価の失敗を end の `message` か `window/showMessage` で成功と区別できるようにする提案。現状は失敗しても end の message が成功時と同じ「evaluated …」で、失敗は stderr のログにしか出ない。(b) 評価 worker の死で nixd 自身が SIGPIPE で落ちないようにする提案。SIGPIPE を無視して次の RPC をエラー応答にする。(c) `workspace/didChangeWatchedFiles` を登録なしで受けても stderr に出さず黙って無視する提案（既にエラーではないので優先は低い）                                                                                                                                                                                                                                                                                                                          | [research/nixd-readiness-measurement.md](research/nixd-readiness-measurement.md)                                                                                                                                                                                                                                                   | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| nil                        | (a) flake.lock の読み込み（`load_flake_info`）にも `$/progress` を出す提案。現状は入力への `definition` が空で通る窓（約 100 ms）に信号がない。(b) flake がない workspace でその旨を `window/logMessage` か `$/progress` で示す提案。現状は信号の不在が「読み込み中」と区別できない。(c) 中断した評価の end を出す前に次の begin を出す提案。現状は end → 115 ms 後に begin で、その間だけ `ready` に見える                                                                                                                                                                                                                                                                                                                                                                  | [research/nil-readiness-measurement.md](research/nil-readiness-measurement.md)                                                                                                                                                                                                                                                     | 第 3 段                                                                                                         | 未着手                                                                                                                                                                                                                                                                   |
| rust-analyzer              | issue で `experimental/serverStatus` への field 追加（`readiness`）と別通知の `experimental/serverState`（fork `tagawa0525/rust-analyzer` の `server-state`）の両案を並べ、相手が選んだ方を PR にする。`quiescent` の飛び越えの実測を根拠にする                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | 仕様 10 章、[research/rust-analyzer-quiescent-measurement.md](research/rust-analyzer-quiescent-measurement.md)                                                                                                                                                                                                                     | 第 2 段。準備 4。rust-lang/rust-analyzer#10888 は「`serverStatus` で既にある」で閉じている                      | fork に両案を用意済み（`server-state`、`server-status-readiness`）。受け入れ条件はどちらも通過。草案は下。両ブランチとも push 済み（pre-commit の自動修正が混ぜた無関係なフェンスの差分は nixfiles #192 の後に除いた）                                                   |
| gopls                      | (a) golang/go#78273 へのコメント: critical error status（"Error loading workspace"）は `window.workDoneProgress` を宣言しないクライアントには `window/showMessage` type 4（Log）の本文だけに落ち、正常の "Finished loading packages."（type 3）と severity で区別できない。fallback の severity を title に応じて選ぶ（`WorkspaceLoadFailure` は Error）提案。(b) 新規 issue: 一度読み込み失敗から回復したセッションでは、以後の go.mod 変更のたびに要求が約 1 秒 "no package metadata" で失敗する（`unloadableFiles` が go.mod の修復で消えない）。`references` が初回ロードを待つこと（`awaitLoaded`、golang/go#76137）と `gopls mcp` を先に認める。CL（fork `tagawa0525/tools` の `server-state`）は出さない。golang/tools は Gerrit で Google CLA が要る                 | [research/gopls-health-measurement.md](research/gopls-health-measurement.md)、[research/gopls-readiness-measurement.md](research/gopls-readiness-measurement.md)                                                                                                                                                                   | 第 2 段。準備 3 は済                                                                                            | **提出済み**（2026-09-09。(a) golang/go#78273 の[コメント](https://github.com/golang/go/issues/78273#issuecomment-5595205676)、(b) golang/go#81400）。応答待ち                                                                                                           |
| LSP 本体                   | `workspace/serverState` の proposal。[microsoft/language-server-protocol#511](https://github.com/microsoft/language-server-protocol/issues/511) のスレッドに「エージェント用途からの再提案」として接続する                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | [research/readiness-vocabulary-corpus.md](research/readiness-vocabulary-corpus.md)、仕様 10 章                                                                                                                                                                                                                                     | 第 3 段。準備 5（`vscode-languageserver-node` の `proposed.serverState.ts`）。第 2 段のどれかに反応があってから | fork `tagawa0525/vscode-languageserver-node` の `server-state` に `proposed.serverState.ts` と `.md` を用意済み（準備 5）。草案は下                                                                                                                                      |

## 草案

出す前にユーザーの確認をもらう文面。確認が取れたものから出し、出したら上の表の状態を更新する。

### typescript-language-server: `fix: stop the server when tsserver is killed by a signal`

PR 先: `typescript-language-server/typescript-language-server`（master）。ブランチ: `tagawa0525/typescript-language-server` の `tsserver-exit-by-signal`（2 コミット。修正本体と Copilot の指摘への対応 6c21094、`src/lsp-server.ts` と `src/ts-client.test.ts`。fork の PR #1 で CI を通してある）。見出しは `## Summary / ## Changes / ## Tests`（PR 本文の見出しは常に英語。ユーザーの決定、2026-09-09）。文面は 2026-09-09 にユーザーの確認済み。同日 typescript-language-server/typescript-language-server#1125 として提出。

本文:

```markdown
## Summary

Since #305 the language server exits when tsserver crashes (#302), so that the client restarts it. The exit is only honoured when tsserver reports an exit code, though (`if (exitCode)` in the `onExit` handler in `lsp-server.ts`). A tsserver killed by a signal reports `exitCode: null` — SIGKILL from the OOM killer, or SIGABRT from Node's own out-of-memory abort (`FATAL ERROR: ... JavaScript heap out of memory` ends the child with `{ code: null, signal: 'SIGABRT' }` on Linux) — and the language server keeps running. Every later request is then answered with `ServerResponse.NoServer`, which the handlers turn into an empty success: `textDocument/references` returns `[]`, `textDocument/definition` returns `null`. A client cannot tell that from a real answer.

How to reproduce (6.0.0): open a file, `kill -9` the tsserver child processes, then send `textDocument/references`. The server logs `[tsserver] Exited. Code: null. Signal: SIGKILL`, answers `{"result": []}`, and is still running ten seconds later.

## Changes

The `exitCode` check dates from 507db40 (2022), a logging-only refactor made right after #536 had removed the exit altogether: at that time the `exit` listener of the tsserver process also fired for the server's own shutdown, which kills tsserver with SIGTERM (`code: null`), and the check only kept that from being logged as a crash. Since #585 the tsserver client disposes its exit handlers before killing the process, so `onExit` only runs for an exit the server did not ask for. This change stops the server on every such exit, with the same error as before.

## Tests

Two cases in `ts-client.test.ts` pin down the two facts the change rests on: a tsserver killed by a signal reaches `onExit` with a null exit code, and `shutdown()` does not reach `onExit`. `pnpm test`, `lint` and `typecheck` pass (CI on my fork: Linux, macOS, Windows × Node 22 / 24). End to end, with this change the server exits with code 1 right after the `kill -9` above.
```

### rust-analyzer: issue（両案）

issue 先: `rust-lang/rust-analyzer`。ブランチ: `tagawa0525/rust-analyzer` の `server-status-readiness`（案 A、1 コミット）と `server-state`（案 B、3 コミット）。PR は相手が選んだ方だけを出す。第 2 段（準備 5〜6 の後）。題名: `experimental/serverStatus: tell clients when answers to workspace-wide requests are complete (a readiness field, or a successor notification)`

本文:

````markdown
## Summary

`experimental/serverStatus` is documented as a status line for the end user, and `quiescent` answers "is there pending background work?". A client that needs to know whether an answer to `references`, `workspace/symbol`, `rename`, … is complete has to read `quiescent` as "ready", and that reading is wrong at the moment it matters most: before the first workspace has been loaded, the server is trivially quiescent. rust-lang/rust-analyzer#10888 asked for a readiness notification and was closed as "already exists" pointing at `serverStatus`; this issue is about the part that does not exist yet, with two implementations to choose from. I will open a PR for whichever you prefer.

## What a client sees today

Measured with rust-analyzer 2026-08-03 (nixpkgs) over stdio, the client declaring `experimental.serverStatusNotification` (script: https://github.com/tagawa0525/lsp-det/blob/main/scripts/rust-analyzer/status-probe.py):

- A directory without `Cargo.toml`: the first notification, 5 ms after `initialized`, is `{health: "warning", quiescent: true, message: "Failed to discover workspace. …"}`. Nothing has been loaded and the fetch has not started (`GlobalState::run` reports the status before `fetch_workspaces_queue.request_op("startup")`). `quiescent: true` here means "nothing in flight", not "ready". 1 ms later: `{health: "error", quiescent: true, …}`.
- A one-crate project: `{quiescent: false}` at 5 ms, `{quiescent: true}` at 1.7 s. The initial `last_reported_status` is `quiescent: true`, so a client that missed a notification (there is no request form) has to assume the server is ready.
- The doc says "this functionality is intended primarily to inform the end user … Clients are discouraged from but are allowed to use the `health` status to decide if it's worth sending a request." There is no field a client may rely on for completeness.

Coding agents are the client that needs this: they send `references` right after starting the server and take an empty answer as a fact (anthropics/claude-code#76870). Zed already reads `serverStatus` (`crates/project/src/lsp_store/rust_analyzer_ext.rs`), so there is a consumer for whichever shape is chosen.

## Option A: a `readiness` field in `ServerStatusParams`

Branch: https://github.com/tagawa0525/rust-analyzer/tree/server-status-readiness

```typescript
interface ServerStatusParams {
    health: "ok" | "warning" | "error",
    quiescent: boolean,
    /// initializing: no workspace loaded yet.
    /// indexing: workspaces are being (re)loaded or caches primed; answers may be incomplete.
    /// ready: fully loaded; answers are complete.
    readiness: "initializing" | "indexing" | "ready",
    message?: string,
}
```

Computed from the same facts as `quiescent`: `initializing` while `workspaces` is empty and no load has failed, `ready` when `is_fully_ready()`, `indexing` otherwise. The initial `last_reported_status` starts at `initializing`, so the notification traffic does not change. The doc paragraph is amended: this field, unlike the others, is meant for clients that decide whether to trust an answer. `quiescent`, the VS Code extension and every other client keep working as they are.

Measured on the branch: `initializing` (4 ms) → `indexing` (0.2 s) → `ready` (0.54 s) on the one-crate project; `initializing` → `{health: "error", readiness: "ready"}` on the directory without `Cargo.toml` (a failed load is reported on the health axis, and readiness says the failure is settled).

Smallest change. What it does not give: a request form (a client that attaches late waits for the next notification), and a declaration of what `ready` covers.

## Option B: a successor, `experimental/serverState`

Branch: https://github.com/tagawa0525/rust-analyzer/tree/server-state

A request `experimental/serverState` answering `{health, readiness, message}` at any time after `initialize`; a notification `experimental/serverStateChanged` sent when `health` or `readiness` changes, if the client declares `experimental.serverState`; and a server capability `serverStateProvider` that declares the guarantees by naming what is missing (`coverage: {scope: "workspace", incomplete: {"workspace/symbol": <workspace.symbol.search.limit>}}`, `freshness: {fileChanges: ["Created", "Changed", "Deleted"]}`). `serverStatus` is untouched. An undiscovered workspace is `health: "error"` there (nothing workspace-wide can be answered), which A leaves at `warning` for compatibility.

The vocabulary follows the server state protocol specification (https://github.com/tagawa0525/lsp-det/blob/main/docs/spec/server-state.md), written as a candidate for LSP itself; `serverStatus` is the closest existing vocabulary and the one it was modelled on.

## Either way

lsp-det, a transparent proxy that derives the same three values from `quiescent` today, reads the field when present (A) or passes the notification through untouched (B), so agents that already sit behind it are covered by both. Both branches pass `cargo xtask tidy` and the `rust-analyzer` lib tests, with `lsp-extensions.md` and its hash updated.
````

### Serena: PR（クラッシュ検知を 2 回目以降の横断要求にも効かせる）

fork の `tsserver-crash-on-request-path`。CONTRIBUTING の「small bug fixes」は issue なしで PR にできる。PR template の checklist 2 つ（scope、CHANGELOG）は満たす。

Title: `fix(typescript): surface a tsserver crash on every cross-file query, not only the first`

````markdown
## Problem

#1848 wires tsserver's `[tsserver] Exited …` log message into `_crash_message` and raises `TypeScriptServerCrashedError` from the indexing waits. But the only place on the find-references path that reaches those waits is `_wait_for_cross_file_references_if_needed`, and it returns before any wait once `_has_waited_for_cross_file_references` has been set by the first query. A crash observed after the first query is logged ("tsserver reported an abnormal exit") and never raised: tsserver's teardown sends a `$/progress` "end", nothing waits, and the query returns an empty result that cannot be told apart from a symbol with no references. `safe_delete_symbol` deletes on exactly that result.

## Reproduction (solidlsp directly, no MCP)

1. `request_references` on a two-file TypeScript fixture (`a.ts` exports `target`, `b.ts` imports and calls it) → 2 locations.
2. `kill -KILL` the two tsserver processes.
3. `request_references` on the same position → 0 locations, no exception. The log has only the WARNING from step 2.

With this change step 3 raises `TypeScriptServerCrashedError: tsserver exited abnormally: [lspserver] [tsclient] [tsserver] Exited. Code: null. Signal: SIGKILL`. Reproduced on `701e7c84` and, with the same one-line change, on top of #1978, which touches the same method (the check goes before the new active-token check).

## Change

Call `_raise_if_crashed()` at the top of `_wait_for_cross_file_references_if_needed`, before the latch. The first query's behaviour is unchanged. One test for the latched case; CHANGELOG entry.

## Note

`TypeScriptServerCrashedError` is not a `LanguageServerTerminatedException`, so this does not trigger the automatic restart; later queries keep failing with the reason until the server is restarted. typescript-language-server/typescript-language-server#1125 makes the language server exit together with tsserver, which would let the crash reach the restart path. The two changes complement each other.

## Checklist

- [x] This PR follows the guidelines in `CONTRIBUTING.md` regarding the scope of PRs.
- [x] For changes that add features or fix problems, I have added an entry to `CHANGELOG.md`, which concisely describes the change.
````

### Serena: issue 4 件

HEAD `701e7c84` の行番号。1 件ずつ別の issue にする（Serena の issue template は checklist だけで、本文の形は自由）。設定の欄は共通: Serena 1.7.1.dev0 at `701e7c84`, read from the source and reproduced with solidlsp directly where stated; Linux; Python (pyright) and TypeScript.

(b-1) Title: `A request timeout is a bare TimeoutError and bypasses the language-server restart path`

````markdown
`Request.get_result` turns `queue.Empty` into a plain `TimeoutError` (`src/solidlsp/ls_process.py:101-107`). Everything that recovers from a broken language server keys on `SolidLSPException.is_language_server_terminated()` (`src/serena/tools/tools_base.py:382-397`), so a timed-out request never reaches the restart-and-retry branch, even when the server is hung rather than slow. The agent sees "Tool execution timed out after N seconds." (`tools_base.py:430-433`) with no indication whether the server is still healthy, and the next tool call goes to the same server.

`$/cancelRequest` is defined (`lsp_protocol_handler/lsp_requests.py`) but never sent, so the timed-out request also keeps running in the server and its late response is dropped as an unknown id.

Suggestion: raise a `SolidLSPException` subclass for the timeout (carrying the language), send `$/cancelRequest`, and let the tool layer decide whether a timeout counts as "terminated" (e.g. after a health probe such as a cheap `documentSymbol`).
````

(b-2) Title: `A failed write to the language server's stdin is logged and swallowed; the request then waits for the full timeout`

````markdown
`StdioLanguageServer` catches `BrokenPipeError`, `ConnectionResetError` and `OSError` around `stdin.writelines` / `flush`, logs "Failed to write to stdin" and returns (`src/solidlsp/ls_process.py:660-667`). The `Request` that was just registered stays in the pending table, so the caller blocks in `get_result` until the request timeout (up to 235 s in Serena's default configuration) unless the stdout reader thread happens to see the process exit first and fails the pending requests itself.

The comment says "don't raise to prevent cascading failures", but the write failing means the server is gone; the honest outcome is `LanguageServerTerminatedException` for that request, which is exactly what the restart path needs.

Suggestion: on a write failure, fail the request with `LanguageServerTerminatedException` (and mark the process as not running) instead of returning silently.
````

(b-3) Title: `Full-document didChange on re-open reuses the document's version number`

````markdown
When a file is re-opened while it is already open in the language server and its mtime changed on disk, `LSPFileBuffer._open_in_ls` sends a full-text `textDocument/didChange` with `version: self.version`, unchanged (`src/solidlsp/ls.py:141-158`). The range-based `didChange` sent for edits increments the version (`ls.py:1378, 1420`).

LSP's `VersionedTextDocumentIdentifier` says the version "will increase after each change". Servers use it to drop stale requests and to order changes; a change that keeps the version can be treated as already applied (rust-analyzer's `ContentModified` retry logic, pyright's document tracking), so the server may keep analysing the old content.

Suggestion: `self.version += 1` before building the notification, as the edit path does.
````

(b-4) Title: `find_symbol with include_info silently drops info when symbol_info_budget is exceeded`

````markdown
`request_info_for_symbols` stops issuing hover requests once `symbol_info_budget` (default 10 s, `serena_config.py:944`; the docstring in `symbol.py` still says 5 s) is spent and sets `info = None` for the remaining symbols (`src/serena/symbol.py:687-694`). The only trace is a single `log.debug` and the perf summary, also at debug level (`symbol.py:715-721`). The tool result is the same as for a symbol that genuinely has no docstring or signature, so the agent cannot tell a budget cut from an absence.

Suggestion: report the cut in the tool result (a count of symbols whose info was skipped, or a note on each affected symbol), and log it at INFO or WARNING.
````

### Serena: registry に lsp-det を載せる提案（oraios/serena#1988 へのコメント）

oraios/serena#1988 がマージされてから、その形に合わせて出す（マージ前なら PR へのコメント）。Serena に自前の readiness 判定を捨てさせない（却下した案）。

````markdown
Thanks for this registry. I would like to use it for a different kind of external implementation and want to check that the shape fits before writing it.

lsp-det (https://github.com/tagawa0525/lsp-det) is a transparent proxy that sits between a client and a language server and adds one thing: a machine-readable server state (`experimental/serverState` request and `experimental/serverStateChanged` notification, with `health`, `readiness`, and what the server's index covers). It holds cross-file requests such as `references` until the server has finished indexing, and turns a crashed server into an explicit error instead of an empty result. Serena already works through it with only `ls_specific_settings.<language>.ls_base_cmd` (measured with pyright and typescript-language-server, including the tsserver-crash case where the direct path returns 0 references and the proxied path raises).

What I would register through this registry is one adapter per language that derives from the built-in one (`PyrightServer`, `TypeScriptLanguageServer`, …), prepends `lsp-det --` to the launch command, declares `experimental.serverState` in the client capabilities, and replaces the adapter's own readiness wait with a read of the server state. Everything else (dependency handling, initialize params, file matching) stays inherited.

Three questions:

1. Is deriving from a built-in adapter and registering the subclass under a new key (e.g. `python-lsp-det`) the intended use, or would you rather see `register(…, allow_override=True)` on the existing key so that `project.yml` does not change?
2. The readiness wait lives inside each adapter's `_start_server` (`PyrightServer` waits for "Found N source files", `TypeScriptLanguageServer` for `$/progress`). Would a hook such as `_wait_for_initial_readiness()` on `SolidLanguageServer`, called from `start_server`, be acceptable so that a subclass can override only that part? Without it the subclass has to copy `_start_server`.
3. Should such an adapter live in this repository (under `src/solidlsp/language_servers/`) or as a separate package that uses the entry point? I am happy to maintain it either way.
````

### LSP 本体: microsoft/language-server-protocol#511 へのコメントと proposal issue

第 3 段（第 2 段のどれかに反応があってから）。issue 先: `microsoft/language-server-protocol`。根拠: [research/readiness-vocabulary-corpus.md](research/readiness-vocabulary-corpus.md)、仕様 10 章、fork `tagawa0525/vscode-languageserver-node` の `server-state`（`proposed.serverState.ts` と `.md`）。proposal issue を先に立て、#511 にはそれへのリンクを添えてコメントする。

microsoft/language-server-protocol#511 へのコメント:

```markdown
Coming back to this thread from a different angle, coding agents as LSP clients, with measurements.

What @matklad wrote in 2021 still holds: `serverStatus` is for humans, and `ContentModified` makes the client poll. What has changed is the client. Agents (Claude Code, Serena, Zed's agent, …) send `textDocument/references` right after starting the server and take an empty or partial answer as a fact. Measured with Claude Code: rust-analyzer answers `references` with `[]` 6 ms after `initialize`, typescript-language-server answers with the declaration only, and the agent then deletes a function that is still in use. A person sees the spinner and waits; an agent does not.

Meanwhile more than twenty servers have each invented the same facts in their own vocabulary: rust-analyzer's `experimental/serverStatus`, jdtls's `language/status`, Sorbet's `sorbet/showOperation`, the Dart analysis server's `ANALYZING` token, clangd's `backgroundIndexProgress`, … (corpus: https://github.com/tagawa0525/lsp-det/blob/main/docs/research/readiness-vocabulary-corpus.md, Japanese, the table is readable without the prose). The facts are: is the index complete, is the server broken, does the answer include my edits.

I have written that up as a proposal, `workspace/serverState` / `workspace/serverStateChanged` with `{health, readiness}` and a `serverStateProvider` capability that says what `ready` guarantees, as `proposed.serverState.ts` plus specification text in the vscode-languageserver-node format (https://github.com/tagawa0525/vscode-languageserver-node/tree/server-state/protocol/src/common), together with a reference implementation: a transparent proxy that speaks it today on behalf of 18 servers by mapping their vocabularies (https://github.com/tagawa0525/lsp-det). Details and the discussion are in the proposal issue: #NNNN.
```

proposal issue の題名: `Proposal: workspace/serverState, a server says whether its index is complete and whether it is functional`

proposal issue の本文:

````markdown
## Summary

A language server indexes its workspace after `initialize` and, while the index is incomplete, answers workspace-wide requests (`textDocument/references`, `workspace/symbol`, `textDocument/rename`, …) with a complete-looking result that is missing entries. LSP has no vocabulary for "this result is incomplete": `$/progress` is a display for humans (a client cannot tell which requests a token affects), and `ServerCancelled` (#1367) makes the client poll. A client that is a person sees the spinner and waits. A client that is a coding agent sends the request 6 ms after `initialize` and acts on the answer.

This proposes one request, one notification and one capability:

- `ServerState { health: "ok" | "warning" | "error", readiness: "initializing" | "indexing" | "ready", message?: string }`. Two independent axes. A failure of indexing is expressed through `health`, never through `readiness`.
- `workspace/serverState` (client → server): the state at the moment the request is received, answered at once.
- `workspace/serverStateChanged` (server → client): sent on every change of `health` or `readiness`, if the client declared `workspace.serverState: true`.
- `serverStateProvider: { coverage?, freshness? }`: what `ready` guarantees, written by naming what is missing from the ideal. `coverage: { scope: "workspace" | "openDocuments" | "document", incomplete: { [method]: cap } }` says over which index results are computed and which methods are capped. `freshness: { fileChanges: ("Created" | "Changed" | "Deleted")[] }` says which `didChangeWatchedFiles` kinds are incorporated before `ready` is reported again (`didChange` always is). `{}` promises the notification only, and even that has value.

Implementation in the vscode-languageserver-node format, with the specification text: https://github.com/tagawa0525/vscode-languageserver-node/tree/server-state/protocol/src/common (`proposed.serverState.ts`, `proposed.serverState.md`; `metaModel.json` regenerated with the entries marked proposed).

## Why this is not new vocabulary

More than twenty servers already report these facts, each in its own words (corpus, with sources: https://github.com/tagawa0525/lsp-det/blob/main/docs/research/readiness-vocabulary-corpus.md; the mapping of each onto the proposal: https://github.com/tagawa0525/lsp-det/blob/main/docs/spec/server-state.md#10-mapping-from-existing-implementations):

| server                                                    | readiness                                                          | health                                                            |
| --------------------------------------------------------- | ------------------------------------------------------------------ | ----------------------------------------------------------------- |
| rust-analyzer                                             | `experimental/serverStatus` `quiescent`                            | `health` in the same notification                                 |
| gopls                                                     | `$/progress` "Setting up workspace" / "Loading packages"           | `$/progress` "Error loading workspace"                            |
| jdtls                                                     | `language/status` `ServiceReady`                                   | `language/status` `ProjectStatus`, diagnostics on the project URI |
| Sorbet                                                    | `sorbet/showOperation` (nested)                                    | none                                                              |
| Dart                                                      | `$/progress` token `ANALYZING`                                     | none                                                              |
| clangd                                                    | `$/progress` token `backgroundIndexProgress`                       | none                                                              |
| pyright, typescript-language-server, nixd, nil, Metals, … | `$/progress` with server-specific titles, `window/logMessage` text | `window/showMessage`, crash of a child process                    |

Clients read them one by one: Zed reads `experimental/serverStatus` for rust-analyzer; Serena has a per-server waiting rule for each of 30 servers, several of them wrong (they wait for a signal the server never sends). #511 asked for this in 2018 and stalled in 2021 on "servers can use `$/progress`" and "who is the consumer". Both answers have changed: `$/progress` does not say which requests are affected or whether the server is broken, and the consumers are now programs.

## What exists today

- A reference implementation: https://github.com/tagawa0525/lsp-det, a transparent proxy (Rust, no dependencies beyond serde) that speaks the protocol under the `experimental/` prefix on behalf of 18 servers by mapping their vocabularies, holds workspace-wide requests for clients that do not speak it, and declares only the guarantees it has verified per server version. Conformance tests for servers (chapter 7 of the specification) and for clients (chapter 9) run against fake servers in CI and against the real servers locally.
- Measurements of what each server answers before `ready`, of what happens when the server is broken, and of whether edits are incorporated, per server: https://github.com/tagawa0525/lsp-det/tree/main/docs/research.
- Two servers with the protocol applied on a fork (rust-analyzer, gopls), and for rust-analyzer an alternative that adds a `readiness` field to `experimental/serverStatus` (rust-lang/rust-analyzer#NNNN).

## Design notes

- Why a notification and not a server-side hold: a server that holds a request until its index is complete removes the empty answer but cannot say whether what is being waited for is an index or a broken server, cannot say which edits the answer includes, and does not let a client that would rather proceed with a partial answer do so. To the client a held request is indistinguishable from an unresponsive server. The proposal does not forbid holding; a server that holds reports `indexing` while it holds.
- Why `health` and `readiness` are separate: a failed index is not a state of readiness, and waiting for `ready` on a broken server is the wrong behavior. `health: "error"` tells the client to stop waiting.
- Why the guarantees name what is missing: a boolean "complete" is unverifiable. "computed over the whole workspace, `workspace/symbol` capped at 128" is, and a client can act on it (narrow the query, compare the count with the cap).
- No time: no value changes on the grounds of elapsed time. "no signal for a while" is not `ready`.
- Forward compatibility: a client ignores a field it does not know, and reads nothing from an axis whose value it does not know.

## Ask

Feedback on the shape, and whether a PR against the 3.19 specification text (`proposed.serverState.md` is already in that format) is the right next step.
````

### gopls (a): golang/go#78273 へのコメント

先: [golang/go#78273](https://github.com/golang/go/issues/78273)（"x/tools/gopls: gopls sends verbose error messages via $/progress instead of window/showMessage"。open。adonovan が "an abridged version displayed in the client UI" に同意）。2026-09-09 にユーザーの確認を得て[提出](https://github.com/golang/go/issues/78273#issuecomment-5595205676)。

本文:

```markdown
A related observation from the client side (gopls v0.23.0; the code is the same at master). The critical "Error loading workspace" status is the only signal that the workspace failed to load, and how it reaches the client depends on `window.workDoneProgress`:

- With it: a `$/progress` begin with title "Error loading workspace", held open until the problem is fixed ("Done.").
- Without it: `progress.Tracker.Start` falls back to `window/showMessage` with `type: 4` (Log), carrying only the message (the title is dropped), and the resolution arrives as `type: 3` (Info) "Done.". "Loading packages..." arrives as Log and "Finished loading packages." as Info through the same fallback.

So for a client that does not implement progress (Claude Code, for example, declares no `window` capability at all) a workspace load failure looks like any other log line, and carries a lower severity than the "Finished loading packages." that precedes it. The requests themselves are honest meanwhile: `textDocument/references` and `textDocument/definition` answer `no package metadata for file …` as errors while the workspace is broken (`workspace/symbol` answers `null`), so what is missing is only the severity and identity of the status.

Suggestion, in the spirit of the abridged message above: when `Tracker.Start` falls back to `showMessage`, let the severity carry what the title carried (`Error` for `WorkspaceLoadFailure`, `Info` for the others) and keep the full text in the server log. Reproduction (a two-file module whose go.mod has an unterminated `require (` block, a stdio client with no `window` capability) and the message logs: https://github.com/tagawa0525/lsp-det/blob/main/docs/research/gopls-health-measurement.md.
```

### gopls (b): golang/go に新規 issue

2026-09-09 にユーザーの確認を得て golang/go#81400 として提出。

タイトル: `x/tools/gopls: after a workspace load error is fixed, every later go.mod change makes requests fail with "no package metadata" for about a second`

本文（golang/go の issue template の見出しに沿う）:

```markdown
### gopls version

golang.org/x/tools/gopls v0.23.0 (go1.26.7 linux/amd64). The code involved (`MetadataForFile` and `clone` in `internal/cache/snapshot.go`) is the same at master.

### What did you do?

1. A two-file module: `go.mod` (`module fixture` / `go 1.21`), `a.go` with `func Target() {}`, `b.go` with `func Caller() { Target() }`. Open `a.go` over stdio and wait for "Finished loading packages.".
2. Break `go.mod` on disk (append an unterminated `require (` block) and send `workspace/didChangeWatchedFiles` (Changed). "Error loading workspace" appears, and requests on `a.go` fail with `no package metadata for file …/a.go` (expected).
3. Fix `go.mod` on disk and send `didChangeWatchedFiles`. "Done." arrives; a second later `textDocument/references` on `Target` works again.
4. Append a comment to `go.mod`, send `didChangeWatchedFiles`, and immediately send `textDocument/references` (or `definition`) on `a.go`, repeating every 50ms.

### What did you see happen?

For about 1.0s after each `go.mod` change, every request on `a.go` fails with `no package metadata for file file:///…/a.go`; then they succeed again. This repeats on every later `go.mod` change for the rest of the session (three changes in a row, two runs).

### What did you expect to see?

The same as in a session that never had a load error. There, step 4 answers correctly at once (16ms after the change in my runs), because `MetadataForFile` loads the file's package inline (`s.load(ctx, NoNetwork, fileLoadScope(uri))`) when its metadata was invalidated.

### Why, reading the source

While the workspace was broken, `MetadataForFile` found no package for `a.go` and added it to `snapshot.unloadableFiles`. A URI leaves that set only through a metadata-affecting change to that `.go` file itself (`clone`: "typing in a file doesn't necessarily make it loadable"); fixing `go.mod` does not remove it. After the recovery the file is loadable again, but it stays marked unloadable, so on every later `go.mod` change (`reinit` in `clone` invalidates the metadata) `MetadataForFile` skips the inline load (`(shouldLoad || len(pkgs) == 0) && !unloadable`) and returns the error until the background reload has run.

A possible fix: clear `unloadableFiles` on the `reinit` path of `clone` (go.mod / go.work / go.sum changed on disk), since a workspace-level change is exactly the kind of change that can make a file loadable again.

Logs of both kinds of session and the probe script: https://github.com/tagawa0525/lsp-det/blob/main/docs/research/gopls-health-measurement.md and https://github.com/tagawa0525/lsp-det/blob/main/scripts/gopls/health-probe.py.
```

### gopls: 提出後の反応（2026-09-09）

両方とも、付いたのは Go チームの自動ボット gabyhelp の "Related Issues" だけで、メンテナの返答はまだない。golang/go#81400 には gopherbot が `gopls` / `Tools` のラベルと milestone Unreleased を付けた。ボットが挙げた issue のうち開いているものは全部読み、こちらの主張に関わるのは次の 3 点。

- golang/go#81400 の重複はない。golang/go#36589（go.mod 変更時の metadata 無効化を差分で判定する提案）は 2026-08 に obsolete とされ、golang/go#79585 は branch 切り替え後に `shouldLoad` が残って go.mod を書き換える別の不具合、golang/go#68002 は読み込みの範囲を絞る親 issue で、回復後に `unloadableFiles` に残る件には触れていない
- [golang/go#50885](https://github.com/golang/go/issues/50885) で findleyr が 2022 年に、critical error status を `$/progress` を開いたまま保持する手法は "tricky, and may be problematic" で各クライアントへの影響を調べるべきと書いている。golang/go#78273 に出した「`window.workDoneProgress` を宣言しないクライアントには Log に落ちる」はその影響の一例
- [golang/go#76137](https://github.com/golang/go/issues/76137)（closed）で adonovan が、"Finished loading packages." は metadata の取得完了であって型検査の完了ではない、準備完了を知るにはサーバーを黒箱として扱いその状態でしか答えられない要求を投げて待て、と答えている。gopls の提出を readiness ではなく health に縮めた判断（[research/gopls-health-measurement.md](research/gopls-health-measurement.md)）と整合する

ユーザーの指示で、ボットへの返信として上を要約したコメントを 2 件に出した（[golang/go#81400 のコメント](https://github.com/golang/go/issues/81400#issuecomment-5599635641)、[golang/go#78273 のコメント](https://github.com/golang/go/issues/78273#issuecomment-5599635886)）。

golang/go#81400 への本文:

```markdown
For the record, none of the related issues above covers this one: #36589 was marked obsolete last month; #79585 is about `shouldLoad` entries surviving a branch switch (a different map, and its symptom is gopls writing to go.mod, not requests failing); #68002 is about how loads are scoped, not about a file staying in `unloadableFiles` after the workspace has recovered. The closest prior discussion is #76137, where the answer was that requests such as `references` block on the initial load — which is exactly what makes this session-long window surprising: a session that never had a load error answers at once after a go.mod change, and only a session that once recovered from one keeps failing.
```

golang/go#78273 への本文:

```markdown
Two of the related issues above bear on the fallback observation. In #50885 findleyr noted in 2022 that holding a critical status open as a hanging progress notification "is tricky, and may be problematic" and that its ramifications on various clients should be looked into; the Log-severity fallback for clients without `window.workDoneProgress` is one such ramification, and it affects exactly the clients that cannot see the hanging notification. In #76137 adonovan pointed out that "Finished loading packages." only means package metadata was obtained, not that type-checking is complete. That is why the suggestion above is limited to the severity of the failure status: it does not ask gopls for a readiness signal, only that a failure not arrive with a lower severity than the success message before it.
```
