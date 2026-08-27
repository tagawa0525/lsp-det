# コーディングエージェントから見た LSP の不満・障害の体系的調査

調査日: 2026-08-28。
調査対象: oraios/serena の issue トラッカー(Web)、`reference/` 配下の LSAP・lsai-protocol の設計文書(ローカル)、cclsp / mcpls / claude-code-lsps(Piebald-AI, boostvolt)の issue トラッカーと README、anthropics/claude-code および claude-plugins-official の LSP 関連 issue、コミュニティ記事。
目的: lsp-det の 3 拡張(A: 宣言範囲の契約、B: 準備完了の通知、C: 起動マニフェスト)がエージェント側の不満の全体像のどこをカバーし、何を残すかの照合。

## 要約

- **最多の不満はカテゴリ B(準備完了の不在)に集中する。** Serena では「インデックス未完了時の空応答・部分応答を完了として返す」issue が最新期でも繰り返し報告され(#1937, #1858, #1923, #1871)、cclsp も初回 `find_references` の欠落(#27, #30)を経験済み。Claude Code 本体もクライアント側で「starting のまま永久に固まる」(#78099)を抱える。B は全実装が固定 sleep・文字列マッチ等の近似で凌いでおり、標準化要求が最も強い。
- **カテゴリ C(起動の仕様外)は「LSP 統合系プロジェクトの issue の主成分」である。** claude-code-lsps 系(Piebald/boostvolt)の不具合 issue はほぼ全てが起動系(`--stdio` フラグ欠落、Windows の `.cmd`/URI 問題、opam/JRE 等の実行環境、起動タイムアウト)。Claude Code 本体の LSP issue も登録・起動失敗(「No LSP server available」系)が最大群。
- **カテゴリ A(範囲の実装依存)は件数こそ少ないが、破損の実害が直接的。** Serena #1529(Go 宣言キーワード重複)、#1484(C# attribute の無言欠落)、#1498(範囲が行外にはみ出す)が代表で、Serena は 12 言語でシンボル補正コードを保持している。
- 3 拡張でカバーされない決定性問題として **診断の完了通知(stale diagnostics)**、**サーバー死活の可視化(OOM 死の無言化)**、**無視ディレクトリの解釈差**、**position encoding / URI 形式**、**クライアント側のプロトコル準拠(server→client 要求の未応答)** が明確に残る。特に「サーバーが死んだ・部分故障した事実が応答から判別できない」は B と地続きの頻出パターン。
- プロトコル外の不満(性能・メモリ・トークン効率・合成クエリの不在)は LSAP / LSAI が主戦場としており、lsp-det の非目的と整合する。

## 1. Serena issue トラッカーの分類

oraios/serena の issue 総数は 847 件(open + closed、2026-08-28 時点、PR 除く)。キーワード検索による概数を示す(検索は重複・ノイズを含む。特に「memory」は Serena 自身のメモリー機能の issue が大半)。

| カテゴリ | 概数 | ノイズ・備考 |
| --- | --- | --- |
| indexing / index | 226 | インデックス関連全般。機能要望も含む |
| 起動失敗・startup / launch | 183 | ダッシュボード起動等のノイズあり |
| ignored / gitignore | 171 | 無視パス処理は独立した頻出領域 |
| timeout | 128 | MCP タイムアウトと LS タイムアウトが混在 |
| 空応答・シンボル不検出 | 124 | — |
| restart | 107 | 再起動要望・再起動バグ両方 |
| 参照の欠落・部分結果 | 78 | B に直結する群 |
| crash | 77 | — |
| hang / stuck | 63 | — |
| diagnostics | 34 | — |
| encoding / offset | 29 | — |
| range 不正 | 28 | 件数は少ないが実害が破損 |

### 1.1 準備完了の不在(B 直撃)

- [#1937](https://github.com/oraios/serena/issues/1937): TypeScript の `find_referencing_symbols` がプロジェクトグラフ読込中に**無言で部分結果**を返す。Serena は「最初のクロスファイルクエリ前」しか待たない
- [#1858](https://github.com/oraios/serena/issues/1858): Scala/Metals でセッション初回の参照検索が部分結果。固定 5 秒 sleep で indexing 完了を待っていない
- [#1923](https://github.com/oraios/serena/issues/1923): Vue のコンパニオン TS サーバーが全ファイルの open に失敗しても「indexing complete」と報告
- [#1871](https://github.com/oraios/serena/issues/1871): Nextflow LS で workspace scan の flush が両方失敗しても「flushed」と記録
- [#937](https://github.com/oraios/serena/issues/937) / [#1390](https://github.com/oraios/serena/issues/1390): JDTLS 自体は初期化完了しているのに Serena 側の ready 判定が完了しない
- [#1789](https://github.com/oraios/serena/issues/1789): JDTLS の起動待ちが無制限で、CI でどのフェーズで止まったか特定できない
- [#634](https://github.com/oraios/serena/issues/634): LSP の再初期化が繰り返され MCP ツールタイムアウトに至る
- [#1771](https://github.com/oraios/serena/issues/1771): 起動コスト回避のため lazy initialization をオプトインで要望

ローカル調査([serena-solidlsp.md](serena-solidlsp.md))と併せると、solidlsp は readiness を 6 類型(サーバー固有通知 / `$/progress` drain / logMessage 文字列マッチ / 初回 diagnostics / 固定 sleep / 即時 ready)の言語別実装で近似しており、ほぼ全てが「タイムアウト後は proceed anyway」で B の欠如をそのまま体現している。

### 1.2 起動・プロセス管理(C およびその周辺)

- [#1469](https://github.com/oraios/serena/issues/1469): JDT LS が起動不能(同梱 JRE 21 と Java 24+ 要求の非互換)
- [#1838](https://github.com/oraios/serena/issues/1838): Windows で TypeScript LS が initialize ハンドシェイク中に無言クラッシュ(exit 1)
- [#1798](https://github.com/oraios/serena/issues/1798): 空プロジェクトでの言語サーバー自動検出結果(空)が恒久化し、以後シンボルツールが全滅
- [#1802](https://github.com/oraios/serena/issues/1802): nixd が `textDocument/diagnostic` 中に終了
- [#1081](https://github.com/oraios/serena/issues/1081) / [#1087](https://github.com/oraios/serena/issues/1087): Kotlin LS のゾンビプロセス・stale 一時ディレクトリ
- [#1944](https://github.com/oraios/serena/issues/1944): 同一プロジェクトへの並行 stdio インスタンスが JDTLS の同じ `-data` ワークスペースを共有し、CPU/RAM 暴走とインデックス破損
- [#1818](https://github.com/oraios/serena/issues/1818): 独立 LSP プロセス群のクリーンアップにプロセステーブル列挙が必要

起動前に CLI 実行を要する言語が 20 超という事実([serena-solidlsp.md](serena-solidlsp.md))も、起動手順が仕様外であることのコスト。

### 1.3 範囲・編集の破損(A 直撃)

- [#1529](https://github.com/oraios/serena/issues/1529): `replace_symbol_body` が Go の type/var/const 宣言で先頭キーワードを重複させ破損
- [#1484](https://github.com/oraios/serena/issues/1484): C# の attribute(`[return: NotNullIfNotNull(...)]`)をメソッド差し替え時に無言で欠落
- [#1498](https://github.com/oraios/serena/issues/1498): GDScript/Godot LSP のシンボル範囲がファイル外にはみ出し、clamp 補正が必要
- [#1697](https://github.com/oraios/serena/issues/1697): `insert_after_symbol` 後の余分な空行
- [#799](https://github.com/oraios/serena/issues/799): zls が rename の編集を返さない
- [#1593](https://github.com/oraios/serena/issues/1593): `find_symbol` が stale な情報を返す(Clojure/LSP)

### 1.4 死活・部分故障の無言化(B と地続き、未カバー)

- [#1814](https://github.com/oraios/serena/issues/1814): tsserver が OOM で死んだ後、`find_referencing_symbols` が空 `{}` を「cross-file indexing complete」「isError: false」として返す
- [#1770](https://github.com/oraios/serena/issues/1770): pull-diagnostics フォールバックが LS 終了例外を握りつぶし、再起動をバイパス
- [#1835](https://github.com/oraios/serena/issues/1835) / [#1833](https://github.com/oraios/serena/issues/1833): ツールが引数ログ以前にハングし追跡不能
- [#1717](https://github.com/oraios/serena/issues/1717): リクエストタイムアウトなしで wedged な ccls が CI ジョブ上限までハング
- [#1940](https://github.com/oraios/serena/issues/1940) / [#1826](https://github.com/oraios/serena/issues/1826): health-check が失敗しても exit 0 / PASS を返す

### 1.5 無視パス・パス解釈(未カバー)

- [#1806](https://github.com/oraios/serena/issues/1806): ディレクトリ名が gitignore パターンに未エスケープで挿入され、0 ファイルを無言でインデックス
- [#1729](https://github.com/oraios/serena/issues/1729): gitignore の否定パターン順序が壊れ `is_ignored_path` が非決定的に
- [#1624](https://github.com/oraios/serena/issues/1624): 無視対象パスなのに scandir の PermissionError でクラッシュ
- [#805](https://github.com/oraios/serena/issues/805): git worktree 構成での利用問題
- [#1891](https://github.com/oraios/serena/issues/1891): 無視パスへのシンボルツールが生の ValueError トレースバックを返す

### 1.6 プロトコル外(性能・メモリ・MCP 設計)

- [#944](https://github.com/oraios/serena/issues/944): Serena MCP が約 30GB のメモリを消費し Claude Code がフリーズ
- [#1890](https://github.com/oraios/serena/issues/1890): 重い・コールドなシンボル呼び出し中に HTTP リスナーが停止
- [#529](https://github.com/oraios/serena/issues/529): インデックスが 82% でクラッシュ(WSL)
- [#1052](https://github.com/oraios/serena/issues/1052): ツール別タイムアウト・最大トークンの設定要望
- コミュニティ側の運用回避策として「事前 `serena project index`」「`MCP_TIMEOUT=60000`」が定着している([解説記事](https://smartscope.blog/en/ai-development/serena-mcp-project-indexing-optimization/))

その他、シンボルの命名規約自体の問題([#1797](https://github.com/oraios/serena/issues/1797): Erlang の `foo/1` が name_path 区切りと衝突)、monorepo での参照過少報告([#1939](https://github.com/oraios/serena/issues/1939))など、シンボル同定・ワークスペース構成の層にも不満がある。

## 2. LSAP / lsai-protocol が挙げる LSP の不足点

### 2.1 LSAP(`reference/LSAP/`)

出典: `reference/LSAP/README.md`、`reference/LSAP/docs/locate_design.md`。

| 不足点 | 内容 |
| --- | --- |
| 原子的すぎる操作 | 「参照を全部知りたい」だけで open → offset 計算 → definition → URI 解析 → 読取 → 抽出の十数往復が必要 |
| 位置指定の困難 | LSP は正確な `{line, character}` を要求するが、LLM は列番号を正確に計算できず、軽微な編集で位置が無効化する(locate_design.md「Problems with Traditional Approaches」) |
| シンボルパスの限界 | シンボルパス方式(Serena 等)は宣言位置しか指せず、シンボル内部・非シンボル位置(文字列・コメント)を指せない |
| 出力がエディタ向け | raw JSON スパンではなくコンテキスト付き Markdown が必要 |
| 合成クエリの不在 | 呼び出し経路・影響分析は raw LSP では複雑なオーケストレーションが必要 |

LSAP は README で Claude Code のネイティブ LSP を「あるが機能していない」と評し(Reddit スレッドを引用)、代替として自層(ブリッジで締める方式)を正当化している。なお [agent-bridges.md](agent-bridges.md) の調査どおり、LSAP 自身は準備完了の待ち・リトライを一切持たない。

### 2.2 lsai-protocol(`reference/lsai-protocol/`)

出典: `reference/lsai-protocol/README.md`、`spec/LSAI-v1.4.md`、`ROADMAP.md`。

| 不足点 | 内容 | lsp-det との関係 |
| --- | --- | --- |
| 往復回数 | 「誰が X を呼び、どのテストがカバーするか」に 5〜8 回の呼び出し | プロトコル外(上位層) |
| 絶対 URI | `file:///...` はトークン浪費。相対パスが必要 | プロトコル外(出力形式) |
| capability 欠落 | callHierarchy 等が無いサーバーでは references + documentSymbol や正規表現で代替(spec の Fallback Strategies) | 機能欠落への代替であり、時間軸の空応答(B)は扱わない |
| 言語ごとの手動セットアップ | 「One server per language, manual setup」を明示的に欠点として列挙 | **C の動機と一致** |
| readiness | ROADMAP に「Deterministic workspace readiness: AsyncReady config per language」— 言語別設定で readiness を自前実装 | **B の欠如の傍証** |
| ビルド前提 | 「Parasitic Architecture」= ビルド済み前提。ビルド・インデックスの状態管理を放棄 | B の回避策の一形態 |

## 3. cclsp / mcpls / claude-code-lsps の苦労点

### 3.1 cclsp(ktnyt/cclsp)

- [#27](https://github.com/ktnyt/cclsp/issues/27) / [#30](https://github.com/ktnyt/cclsp/issues/30): 初回 `find_references` が不完全 — `didOpen` 欠落とインデックス待ちを後追いで修正(**B**)
- [#26](https://github.com/ktnyt/cclsp/issues/26): 「LS の warmup/indexing 時間の期待値」をドキュメント化する要望(**B**)
- [#52](https://github.com/ktnyt/cclsp/issues/52): server→client 要求への未応答、Content-Length のバイト長フレーミング不備で TypeScript 7 native LSP が動かない(プロトコル準拠)
- [#47](https://github.com/ktnyt/cclsp/issues/47) / [#53](https://github.com/ktnyt/cclsp/issues/53): MCP stdio 切断時に LSP 子プロセスが orphan 化(プロセス寿命)
- [#43](https://github.com/ktnyt/cclsp/issues/43): `find_workspace_symbols` が「No Project」エラー — `ensureFileOpen` 欠落
- [#42](https://github.com/ktnyt/cclsp/issues/42): キャッシュ済みの stale diagnostics(診断完了の不在)
- [#35](https://github.com/ktnyt/cclsp/issues/35): Windows で「Connected」表示のまま「Connection closed」(死活可視化)
- [#24](https://github.com/ktnyt/cclsp/issues/24): rename が単一ファイルに限定される
- ツールとして `restart_server` を公開しており(README)、「サーバーが不調になったらエージェントが再起動を試みる」運用が前提化している
- [#40](https://github.com/ktnyt/cclsp/issues/40): Claude Code プラグインの登場を受けて開発中止を宣言

### 3.2 mcpls(bug-ops/mcpls)

issue の大半は dependabot と内部品質だが、LSP 統合の負担を示すものとして:

- [#327](https://github.com/bug-ops/mcpls/issues/327): diagnostics キャッシュの切り詰めが重要度を考慮しない(診断のサイズ管理)
- [#313](https://github.com/bug-ops/mcpls/issues/313): LSP エラーメッセージを無制限に MCP へ転送
- [#315](https://github.com/bug-ops/mcpls/issues/315) / [#324](https://github.com/bug-ops/mcpls/issues/324): DocumentTracker のリソース上限管理
- [#321](https://github.com/bug-ops/mcpls/issues/321) / [#318](https://github.com/bug-ops/mcpls/issues/318) / [#329](https://github.com/bug-ops/mcpls/issues/329): stdin ブロッキングやシグナル処理によるシャットダウン不全(プロセス寿命)
- [agent-bridges.md](agent-bridges.md) の調査どおり、mcpls は `$/progress` を受信するが破棄し、position encoding(UTF-8/16/32)変換だけを唯一の補正インフラとして持つ

### 3.3 claude-code-lsps(Piebald-AI / boostvolt)

両マーケットプレースの不具合 issue はほぼ全部が**起動系(C)と Claude Code クライアント側の欠陥**に分類できる。

Piebald-AI:

- [#75](https://github.com/Piebald-AI/claude-code-lsps/issues/75): `workspaceSymbol` ツールのスキーマに `query` パラメータがなく使用不能(クライアント実装)
- [#74](https://github.com/Piebald-AI/claude-code-lsps/issues/74): diagnostics 専用サーバーが拡張子を独占し、本命の LS を遮蔽(ルーティング)
- [#72](https://github.com/Piebald-AI/claude-code-lsps/issues/72): Windows で `pyright-langserver` の spawn が ENOENT(`.cmd` 拡張子が必要)(**C**)
- [#69](https://github.com/Piebald-AI/claude-code-lsps/issues/69): Windows ネイティブで `.java` 拡張子が LSP ルーターに未登録(**C**)
- [#73](https://github.com/Piebald-AI/claude-code-lsps/issues/73): jdtls の `startupTimeout` 引き上げ・`maxRestarts` 追加(**B/C**)
- [#67](https://github.com/Piebald-AI/claude-code-lsps/issues/67) / [#62](https://github.com/Piebald-AI/claude-code-lsps/issues/62): インストール手順・配布物の変化への追従(**C**: 導入手順が仕様外)
- README は Claude Code の組み込み LSP を「使える状態にするには tweakcc でパッチが必要」と明記

boostvolt:

- [#32](https://github.com/boostvolt/claude-code-lsps/issues/32) / [#33](https://github.com/boostvolt/claude-code-lsps/issues/33): kotlin-lsp が `--stdio` フラグ欠落で「starting」のままハング(**C**、かつ B の欠如でハングとして表面化)
- [#29](https://github.com/boostvolt/claude-code-lsps/issues/29): Claude Code が不正な Windows file URI を送り ZLS が InvalidParams(URI/encoding)
- [#28](https://github.com/boostvolt/claude-code-lsps/issues/28): Windows で LSP プラグイン全滅
- [#30](https://github.com/boostvolt/claude-code-lsps/issues/30): OCaml は opam 経由で起動する必要(**C**: 起動環境の宣言不在)
- [#14](https://github.com/boostvolt/claude-code-lsps/issues/14): 「Java LSP が動いているとどう分かるのか」(**B/health**: 可視化要求そのもの)
- [#13](https://github.com/boostvolt/claude-code-lsps/issues/13): dart lsp が動かない

## 4. Claude Code 本体の LSP へのコミュニティ不満

anthropics/claude-code にはタイトルに LSP を含む issue が 279 件(2026-08-28、is:issue)。主要な群:

### 4.1 登録・起動の失敗(C の欠如がクライアント側にも波及)

- [#15168](https://github.com/anthropics/claude-code/issues/15168) / [#14803](https://github.com/anthropics/claude-code/issues/14803) / [#16214](https://github.com/anthropics/claude-code/issues/16214): 設定が正しくても常に「No LSP server available」
- [#84857](https://github.com/anthropics/claude-code/issues/84857) / [#90114](https://github.com/anthropics/claude-code/issues/90114): バンドル LSP プラグインに `lspServers` 設定が欠落し、ツールが一切登録されない
- [#53399](https://github.com/anthropics/claude-code/issues/53399) / [#71800](https://github.com/anthropics/claude-code/issues/71800): `lspServers` のスキーマ的に正しいフィールド(`restartOnCrash` 等)を宣言するとサーバーごと無言で登録されない
- [#20050](https://github.com/anthropics/claude-code/issues/20050): ネイティブバイナリ版で LSP プラグインが動かない
- [#78188](https://github.com/anthropics/claude-code/issues/78188): Windows で jdtls が「unsafe location」判定
- [#79690](https://github.com/anthropics/claude-code/issues/79690): jdtls に VM 引数(Lombok javaagent)を渡せず誤診断
- [#53837](https://github.com/anthropics/claude-code/issues/53837): LSP サブプロセスの stdin が即 EOF になりサーバーがメッセージ受信前に終了

起動コマンド・引数・環境・設定の受け渡しが標準化されていないため、クライアント(Claude Code)とプラグイン作者の双方が言語ごとに手探りしている構図。

### 4.2 readiness・ハング(B)

- [#78099](https://github.com/anthropics/claude-code/issues/78099): サーバーが `initialize` に応答済みでも、クライアントが「server is starting」状態から永久に抜けない。ツール呼び出しは無期限ハング
- [#52693](https://github.com/anthropics/claude-code/issues/52693): dynamic capability registration を使うサーバーで組み込みクライアントが永久ハング

### 4.3 プロトコル準拠の欠落(server→client 方向)

- [claude-plugins-official#1359](https://github.com/anthropics/claude-plugins-official/issues/1359) / [#16360](https://github.com/anthropics/claude-code/issues/16360): `workspace/configuration` 等 3 つの server→client 要求に `-32601` を返し、csharp-ls がソリューション読込を中断。C# の code intelligence が一切得られない

### 4.4 診断の鮮度・スコープ(未カバーの決定性問題)

- [#80267](https://github.com/anthropics/claude-code/issues/80267): 外部(Bash 等)のファイル書換後に stale diagnostics を表示
- [#57840](https://github.com/anthropics/claude-code/issues/57840) / [#64239](https://github.com/anthropics/claude-code/issues/64239): サーバー別の stale diagnostics(SourceKit-LSP、TS project references)
- [#50024](https://github.com/anthropics/claude-code/issues/50024): 遅い非同期 `publishDiagnostics` を `<new-diagnostics>` が取りこぼす(**診断完了通知の不在**そのもの)
- [#50224](https://github.com/anthropics/claude-code/issues/50224): 診断が兄弟 git worktree 間でリークする
- [#72594](https://github.com/anthropics/claude-code/issues/72594): goToDefinition が `.venv` 内のファイルを落とす(無視ディレクトリの解釈)

### 4.5 可視化・運用

- [#89473](https://github.com/anthropics/claude-code/issues/89473): LSP サーバーの状態を見る `/lsp` コマンド要望(health)
- [#80733](https://github.com/anthropics/claude-code/issues/80733): サブエージェントから LSP ツールが無言で剥がれる
- [#40282](https://github.com/anthropics/claude-code/issues/40282): diagnostics / codeAction / rename の公開要望

## 5. lsp-det の 3 拡張との照合

### 5.1 カバーされる不満

| 拡張 | カバーされる不満(代表出典) |
| --- | --- |
| A: 宣言範囲 | Serena #1529 / #1484 / #1498 / #1697、Serena の 12 言語分シンボル補正コード([serena-solidlsp.md](serena-solidlsp.md))、gopls の `type` キーワード欠落([vision.md](../vision.md) 1.1) |
| B: 準備完了 | Serena #1937 / #1858 / #1923 / #1871 / #937 / #1390 / #1789 / #634、cclsp #27 / #30 / #26、boostvolt #14、LSAI の言語別 AsyncReady 設定、solidlsp の 6 類型 readiness 近似。空応答と「結果なし」の区別不能が根本 |
| C: 起動 | Serena #1469 / #1798 / #1838、boostvolt #32 / #30、Piebald #72 / #69 / #67 / #62、claude-code #78188 / #79690、20 超言語の起動前 CLI 実行、LSAI の「manual setup per language」批判 |

補足: Claude Code の登録失敗群(4.1 の「No LSP server available」系)はクライアント実装バグだが、起動宣言が仕様外であるがゆえに各クライアントが独自の設定スキーマ(`lspServers`)を発明し、その解釈差で壊れているという意味で C の間接的な帰結でもある。

### 5.2 決定性に関わるが 3 拡張でカバーされない不満

| 領域 | 内容 | 代表出典 |
| --- | --- | --- |
| 診断の完了通知 | `publishDiagnostics` はいつ「揃った」かを示さない。stale 診断・取りこぼしが多発 | claude-code #50024 / #80267 / #57840 / #64239、cclsp #42、Serena #1770 |
| サーバー死活・部分故障 | OOM 死・クラッシュが空応答と区別できず「成功」として報告される | Serena #1814 / #1770 / #1940、cclsp #35、boostvolt #14 |
| 無視ディレクトリ | どのパスを解析対象とするかがサーバー・ブリッジ毎に非決定的 | Serena #1806 / #1729 / #1624、claude-code #72594 / #50224 |
| position encoding / URI | UTF-16 既定と URI 形式差(特に Windows)による InvalidParams | boostvolt #29、mcpls の変換インフラ、[agent-bridges.md](agent-bridges.md) |
| クライアント側準拠 | server→client 要求(`workspace/configuration` 等)・dynamic registration・バイトフレーミングの未実装 | claude-plugins-official #1359、claude-code #52693、cclsp #52 |
| シンボル同定 | name_path 等シンボルの一意な指し方が未標準(Erlang `foo/1` 衝突、位置指定の困難) | Serena #1797、LSAP locate_design.md |
| ワークスペース構成 | monorepo / worktree / 複数ルートでの参照過少・状態リーク | Serena #1939 / #1260 / #805、claude-code #50224 |
| 並行アクセス | 同一プロジェクトへの複数クライアントでインデックス破損 | Serena #1944 / #1864 |

このうち「診断の完了通知」と「死活の可視化」は B の自然な隣接領域であり、issue 上も B と同じ「無言の嘘(silently wrong)」として現れる。v0.1 スコープ外としても、拡張 B の語彙設計時に将来の拡張余地(例: readiness の対象に diagnostics フェーズを含められる形)を意識する価値がある。

### 5.3 プロトコル外(lsp-det の非目的と整合)

| 領域 | 内容 | 主な担い手 |
| --- | --- | --- |
| 往復回数・トークン効率 | 合成クエリ(impact / context / callers)、Markdown 出力、相対パス | LSAP、LSAI |
| 位置指定の人間工学 | `find` パターン等の semantic locate | LSAP locate モジュール |
| 性能・メモリ | 30GB 消費、インデックス時間、ゾンビプロセス | Serena #944 / #529 / #1081 |
| MCP 層の設計 | ツールタイムアウト、stdio 切断時の子プロセス管理、read_only の広告 | Serena #1052、cclsp #47、mcpls #321 |
| capability 欠落への代替 | callHierarchy 等が無いサーバーでのフォールバック | LSAI Fallback Strategies |

## 6. 考察

1. **不満の最頻パターンは「無言の嘘」である。** 空応答・部分応答・完了偽装・死んだサーバーの成功報告 — いずれも「応答はプロトコル上正当だが、真実ではない」。エージェントは応答を文字通り信じるため、この種の欠陥だけがエディタ時代より深刻化している。B はこのパターンの最大の発生源(インデックス未完了)を潰すが、隣接する診断・死活にも同型の問題が残る。
2. **C の不在は 2 段階でコストを生んでいる。** 第一にブリッジ実装者の言語別起動コード(Serena の 20 超言語の CLI 前処理、マーケットプレースの言語別 issue)、第二にクライアント独自スキーマの発明とその解釈バグ(Claude Code の `lspServers` 群)。起動宣言の標準はこの両方に効く。
3. **A は件数こそ少ないが、唯一「コード破損」に直結するカテゴリ。** 発生した issue の深刻度(無言のキーワード重複・attribute 欠落)は高く、Serena が 12 言語で補正を持つ事実自体が恒常コストの証拠。
4. LSAP / LSAI の不満リスト(往復・トークン・合成クエリ)はブリッジ層(a 方式)で解決すべきもので、lsp-det の 3 拡張とは競合せず補完する。両プロジェクトとも readiness を自前の近似で埋めている点([agent-bridges.md](agent-bridges.md))は、(b) 方式で B を締める価値の傍証になっている。

## 付記: 調査方法の限界

- Serena のカテゴリ別件数は GitHub search API のキーワード検索によるもので、重複・偽陽性を含む概数。ラベル体系による厳密な分類は行っていない
- Claude Code 本体の issue は LSP 関連 279 件のうちタイトル・検索上位の代表例のみ精査した
- Reddit 等フォーラムの一次発言は直接取得せず、LSAP README・Piebald README・検索結果経由の言及([Serena インデックス最適化記事](https://smartscope.blog/en/ai-development/serena-mcp-project-indexing-optimization/) 等)にとどまる
