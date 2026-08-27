# LSP への批判・限界の指摘 — 実装者コミュニティ調査

調査日: 2026-08-28 / 調査方法: Web 検索および一次ソース(実装者ブログ・GitHub issue/discussion・仕様リポジトリ)の直接確認

## 要約

言語サーバー・エディタ実装者による LSP 批判は、(a) 決定性・機械可読性の欠如、(b) プロトコル設計(状態同期・因果性・拡張性)、(c) パフォーマンス、(d) ガバナンス(Microsoft/VS Code 主導)の 4 系統に整理できる。最も体系的な批判は matklad(rust-analyzer 元リード)の「LSP could have been better」と Michael Peyton Jones(Haskell Language Server メンテナ)の「LSP: the good, the bad, and the ugly」であり、両者とも「仕様の曖昧さ(実装依存の解釈余地)」「状態同期のアドホックさ(結果が最新の文書状態を反映しているか確認する術がない)」を中核的欠陥として挙げる。lsp-det が対象とする 3 点(A: シンボル範囲の実装依存、B: 準備完了の不在、C: 起動の仕様外)は、いずれも実装者コミュニティで独立に繰り返し指摘されてきた実在の欠陥である。特に B(readiness)は LSP 仕様リポジトリに 2018 年から未解決 issue が存在し、rust-analyzer は独自拡張(`rust-analyzer/status`)で回避している。C(起動)は nvim-lspconfig・Helix `languages.toml`・Zed extension といった「クライアント側の起動規約レイヤー」が各エディタで再発明されている事実そのものが証拠となる。A(範囲)は仕様 issue と、位置・範囲ベースの同一性の脆さを理由に人間可読なシンボル文字列へ移行した Sourcegraph SCIP の設計判断が裏付ける。エージェント(機械消費者)の登場により、従来「人間が status bar を見て補完すればよい」とされた a 系統の欠陥(readiness・決定的な結果・再現可能な環境)の重要度が最も大きく上昇している。

---

## 1. matklad(Alex Kladov、rust-analyzer 元リード)

### 1.1 「Why LSP?」(2022-04-25)

出典: [Why LSP?](https://matklad.github.io/2022/04/25/why-lsp.html)

- LSP の功績は認める(IDE 機能が「あって当然」になった)が、その成功の通説である「M×N 問題の解決」は誤りだと主張。真の問題は「不適切な均衡」(言語サーバーを作る動機も、受け入れるエディタ側 API もなかった)であり、LSP は市場の両側を同時に立ち上げた点が本質。
- 技術的実装は「rather bad」だが「good enough」と評価。
- VS Code 自身は LSP を第一級概念にしておらず(拡張ポイントを提供するだけ)、プロトコルの語彙は「意味論 API」ではなく「プレゼンテーション API」に寄っている。
- 言語サーバーは LSP を「内部データモデルではなくシリアライゼーション形式」として扱うべき、より良いインターフェースが将来現れうる、と締める。

### 1.2 「LSP could have been better」(2023-10-12)

出典: [LSP could have been better](https://matklad.github.io/2023/10/12/lsp-could-have-been-better.html)

具体的批判(lsp-det の分類を付記):

- **RPC と状態同期の混同(最大の構造的欠陥)** — LSP はエッジトリガの request/response だが、IDE 機能の本質はレベルトリガの状態同期。診断やハイライトを request で実装すると「古い結果を返すか、毎変更ごとに再問い合わせで浪費するか」の二択になる。Dart Analysis Protocol の購読モデル(ファイル単位で機能を購読し増分更新を受ける)の方が優れていると明言。→ (b)、および結果の鮮度が不定という意味で (a) に接続
- **通知(notification)はアンチパターン** — 一方向でエラーを返せず、順序の曖昧さを生む。「クライアントは、サーバー発の編集が最新の変更を織り込んでいるかどうかを知る方法がない」。→ (a)(b)
- **UTF-16 コード単位の列座標** — UTF-8 バイトオフセットか Unicode コードポイントにすべきだった。→ (a) 機械可読性・エンコーディング
- **フレーミングと JSON-RPC** — `Content-Length` ヘッダの独自フレーミングは不要な複雑さ。`"jsonrpc": "2.0"` はノイズ、XML-RPC 由来のエラーコード(`-32601` 等)は不明瞭。→ (b)(c)
- **dynamic registration の複雑さ** — 概念的コストに見合う正当化がなく、rust-analyzer でもほぼ使っていない。→ (b)
- **対話的リファクタリングの欠如** — change signature 等の複数ステップ操作はネイティブ表現がなく、サーバーごとの独自拡張になる。→ (b) 拡張性

なお rust-analyzer は実運用上、`rust-analyzer/status` という**独自の readiness 通知拡張**を持ち、YouCompleteMe 等のクライアントは起動完了判定にこれへ依存している(→ 3.3 節、lsp-det B と直結)。

## 2. Michael Peyton Jones(Haskell Language Server / `lsp` ライブラリメンテナ)

出典: [LSP: the good, the bad, and the ugly](https://www.michaelpj.com/blog/2024/09/03/lsp-good-bad-ugly.html)(2024-09-03)

サーバー実装者視点で最も網羅的な批判。主要論点:

- **仕様の曖昧さ・過少仕様(underspecification)** — code lens の表示位置、`InlayHint.paddingLeft` の挙動、`CompletionItem` の `detail` / `documentation` / `labelDetails` の使い分けなどが未定義で、**VS Code の挙動が事実上の仕様**になっている。configuration モデルは「特に大きな混乱(a particularly big mess)」。→ (a) 実装依存の温床
- **因果性・順序の不定** — クライアントが didChange を送った直後に code action を要求しても、**応答が変更後の状態を反映しているか確認する仕組みがない**。並行性は事実上必須(キャンセル・progress)なのに仕様は「並行にやれ、変なことが起きたら自己責任」程度しか言わない。→ (a)(b)
- **状態同期の非一貫性** — 実質「状態同期」である機能が 14 個あるのに、増分更新・フィルタリング・無効化・バージョン管理の流儀がバラバラ。文書バージョン番号があるのはテキスト文書だけで例外的。汎用の状態同期プロトコルに統一すべきと提案。→ (b)
- **UTF-16** — 「Windows に引きずられた悪い選択」。→ (a)
- **型の多義性** — `WorkspaceFolder[] | null` + フィールド省略で「空」の表現が 3 通りあり区別が未定義。→ (a) 機械可読性
- **仕様の肥大** — 90 メソッド・407 型・印刷で 285 ページ。実装ミスを誘発。→ (b)(c)
- **相互作用モデルの貧しさ** — カスタム機能は実質 code action のみで、多段階操作・実行前確認ができない。カスタムメソッドは pre-LSP の断片化を再導入する。→ (b)
- **ガバナンス** — 仕様のコミッタは実質 Microsoft VS Code チームの 1 名のみ。**機能はまず VS Code に実装され、事後承諾(fait accompli)として仕様化される**。追加前の公開議論はゼロで、実装者コミュニティのフォーラムも存在しない。LSP 2.0 での作り直しよりも、標準化委員会型のオープンガバナンス移行を主張。→ (d)

## 3. エディタ実装者(Zed / Helix / Neovim)

### 3.1 Zed

出典: [Making Python in Zed Fun](https://zed.dev/blog/making-python-in-zed-fun)、[LSP Improvements メタ issue #26916](https://github.com/zed-industries/zed/issues/26916)、[LSP 拡張サポート issue #21133](https://github.com/zed-industries/zed/issues/21133)

- 「LSP は重要な機能を可能にするが、現状(status quo)を我々は好まない」と明言。**言語サーバーは自分がどのインタープリタ・venv・ツールチェーンで動くべきか知らず、LSP はそれを標準化しない**。結果としてユーザーが設定ファイル・パス・venv 検出と格闘する。→ (a) 起動・環境の仕様外(lsp-det C と直結)
- 複数サーバー(Python では pyright/ruff/ty 等)の協調は LSP の範囲外で、パッチ的対応では「複雑さが爆発する」。→ (b)
- サーバー固有の LSP 拡張(Java の jdtls 拡張等)をクライアント側で受ける仕組みが常に問題になる。→ (b) 拡張性

### 3.2 Helix

出典: [Discussion #11730「Refusal to support LSP extensions leads to basic unusability」](https://github.com/helix-editor/helix/discussions/11730)、[Discussion #7427(保存が LSP 待ちで 20 秒以上遅延)](https://github.com/helix-editor/helix/discussions/7427)

- **素の LSP にはサーバーの健全性・進捗を伝える十分な情報がない**。「サーバーがビルド失敗でクラッシュしている」ことをユーザーに伝えるには LSP 拡張が必要だが、Helix は原則拡張非対応のため、LSP フィードバックが実質死んでいても静かに沈黙する、という批判。→ (a) readiness/状態可視性(lsp-det B と直結)
- サーバー処理中にファイル書き込みがブロックされる等、ライフサイクル・進捗の標準の弱さに起因する UX 問題。→ (a)(c)
- Helix も起動設定(コマンド・引数・ルート判定)を `languages.toml` という独自規約で持つ。→ C の傍証

### 3.3 Neovim / rust-analyzer クライアント側

出典: [nvim-lspconfig](https://github.com/neovim/nvim-lspconfig)、[rust-analyzer #10888「Add notification for when workspace is ready」](https://github.com/rust-lang/rust-analyzer/issues/10888)、[rust-analyzer #15837「How to determine whether a file is indexed」](https://github.com/rust-lang/rust-analyzer/issues/15837)、[LSP & Neovim; A Retrospective!](https://vikasraj.dev/blog/lsp-neovim-retrospective)

- nvim-lspconfig の存在自体が批判の物証: **起動コマンド・filetype 対応・ルートディレクトリ判定・初期化オプションはすべて仕様外**であり、コミュニティがサーバーごとの起動規約データベースを手作業で維持している。→ C
- rust-analyzer ではワークスペース読み込み完了前のリクエストが「waiting for cargo metadata」等のエラーになり、**完了通知の標準がない**ため `rust-analyzer/workspaceReady` のような独自通知の追加が議論された(#10888)。ファイルがインデックス済みかを問い合わせる標準手段もない(#15837)。→ B
- Neovim 組み込み LSP の初期は、仕様に厳密に従わないサーバーへの対応(off-spec 挙動の吸収)がクライアント実装の主要コストだったことが retrospective 等で語られる。→ (a)

## 4. LSP 仕様リポジトリ上の未解決論点(readiness・範囲)

- [microsoft/language-server-protocol #511「LSP-server readiness indicator」](https://github.com/microsoft/language-server-protocol/issues/511)(2018 年提起、Backlog のまま未解決) — 「サーバーがまだロード中なのか死んでいるのか分からず、ユーザーは補完が壊れたと報告し続ける」。`window/showStatus` の標準化提案。→ B の一次証拠
- [#904「Progress reporting during initialization」](https://github.com/microsoft/language-server-protocol/issues/904)、[#786「Progress support in LSP」](https://github.com/microsoft/language-server-protocol/issues/786) — `$/progress` は 3.15 で入ったが**汎用の進捗表示手段であり、「準備完了」の機械判定可能なセマンティクスは定義されない**(`quiescent` 相当の概念は無い)。→ B
- [#1778「DefinitionRequest Change」](https://github.com/microsoft/language-server-protocol/issues/1778)、[#1270(selectionRange の空応答が表現不能)](https://github.com/microsoft/language-server-protocol/issues/1270) — definition の返す範囲・`selectionRange` の解釈がサーバーごとに揺れる、応答型が縮退していて区別を表現できない、という報告。`DocumentSymbol.range` と `selectionRange` の切り分け(宣言全体か名前だけか)も慣習依存。→ A の一次証拠
- 位置エンコーディングは批判(clangd の UTF-8 拡張、Neovim 側の要望等)を受けて 3.17 でようやく `positionEncoding` ネゴシエーションが追加されたが、UTF-16 が既定のまま。→ (a)

## 5. Sourcegraph SCIP / LSIF

出典: [Announcing SCIP](https://sourcegraph.com/blog/announcing-scip)、[SCIP DESIGN.md](https://github.com/sourcegraph/scip/blob/main/docs/DESIGN.md)

LSIF(LSP のデータモデルを永続化した索引形式)の運用経験から、Sourcegraph は 2022 年に後継 SCIP を設計した。LSP/LSIF のどこを不十分としたか:

- **不透明な数値 ID によるグラフ符号化** — 「インデクサのオフバイワンのバグが 1 つあるだけでリポジトリ全体のコードナビゲーションが壊れた」。SCIP は**人間可読な文字列シンボル ID**に移行し、誤りの影響範囲(blast radius)を局所化しデバッグ可能にした。→ (a) 決定性・機械可読性。位置・範囲ベースの参照ではなく安定したシンボル同一性を一次概念にした点で、lsp-det A の問題意識(範囲の実装依存はシンボル同一性を壊す)と同型
- **グラフ隣接リスト形式はモノリシックなインデクサを強制し、メモリを浪費** — SCIP は文書と配列の平坦な形式でストリーミング・並列処理・ファイル単位の増分に対応。→ (c)
- **プロデューサ(インデクサ実装者)最適化** — LSIF は消費側最適化だったが、実装者の数はプロデューサの方が多い。書きやすさ・デバッグ容易性を優先。→ (b)(c)

## 6. tsserver と LSP

出典: [TypeScript #39459「tsserver should implement LSP」](https://github.com/microsoft/TypeScript/issues/39459)、[#11274(2016 年の同旨 issue)](https://github.com/microsoft/TypeScript/issues/11274)、[typescript-language-server](https://github.com/typescript-language-server/typescript-language-server)、[A 10x Faster TypeScript(native port 発表)](https://devblogs.microsoft.com/typescript/typescript-native-port/)

- tsserver のプロトコルは **LSP より古く(2013 年頃〜)、VS Code 本体が直接この独自 API を使う**構造だったため、LSP 準拠は長年「必要性が薄い」まま放置された。他エディタはコミュニティ製ラッパー(typescript-language-server)で変換して使うしかなく、#11274 では Sourcegraph のエンジニアが「公式サーバーが LSP を話せば存在しないはずの問題が山ほどある」と証言している。
- つまり **LSP の最大級の言語(TypeScript)ですら、参照実装元の Microsoft 自身が LSP を採用していなかった**という逆説が、(d) ガバナンス批判(VS Code ファースト)の代表例として頻繁に引かれる。
- 2025 年発表の Go 移植(TypeScript 7 / typescript-go)で初めて**ネイティブ LSP 実装**が公式化される。移行理由の一つが「エディタ中立な標準への追従」であり、独自プロトコル維持のコストを Microsoft 自身が認めた形。

## 7. エージェント(機械消費者)時代の再評価

出典: [Lanser-CLI / RLCSF(arXiv:2510.22907)](https://arxiv.org/abs/2510.22907)、[LSP vs the full JetBrains IDE stack(Explyt)](https://medium.com/@explyt.ai/lsp-vs-the-full-jetbrains-ide-stack-what-an-ai-agent-misses-without-the-ide-platform-273a2fd1874c)、[Beyond Prompt Guessing(DEV Community)](https://dev.to/tamizuddin/beyond-prompt-guessing-why-lsp-integration-is-the-missing-protocol-for-reliable-ai-coding-agents-i7)

- 学術側(Lanser-CLI)は「言語サーバーは人間駆動の IDE 向けに設計されており、学習・自動化ループ向けではない」と明言し、エージェント規模での運用要件として**決定性(結果の正規化・スナップショット固定)、`file:line:col` を超える頑健なセレクタ(範囲・位置依存性の克服)、セッションの再現可能性(環境メタデータの固定)、プレビュー優先の安全な変更適用**を定式化している。これは lsp-det の A(範囲非依存の同一性)・B(状態が確定した時点の定義)・C(環境・起動の決定性)とほぼ一対一に対応する。
- 実務側の記事群は「grep は確率的、LSP は決定的」と LSP を持ち上げる一方で、エージェントが実際に使うと (1) いつ問い合わせてよいか分からない(readiness)、(2) サーバーごとに応答形状・範囲が揺れる、(3) 起動・環境構築が自動化の最難関、という 3 点で躓くことを一致して報告しており、人間なら status bar と再試行で吸収していた欠陥が機械消費では顕在化する。

---

## 8. 論点の分類と lsp-det 3 拡張との照合

凡例 — A: シンボル範囲の実装依存 / B: 準備完了の不在 / C: 起動の仕様外 / agent: 機械消費者の登場で重要度が上がるか

| 論点 | 分類 | 指摘者(出典) | lsp-det との重なり | agent |
| --- | --- | --- | --- | --- |
| definition/documentSymbol の範囲・selectionRange の解釈揺れ | (a) | LSP #1778, #1270; michaelpj(過少仕様) | **A に直結** | 上昇 |
| 位置・範囲ベース同一性の脆さ(→文字列シンボル ID へ) | (a) | Sourcegraph SCIP | **A と同型の問題意識** | 上昇 |
| UTF-16 座標 / positionEncoding が後付け | (a) | matklad; michaelpj; clangd/Neovim | A の周辺(座標の決定性) | 上昇 |
| サーバー readiness の標準不在 | (a) | LSP #511; rust-analyzer #10888, #15837; Helix #11730 | **B に直結** | 大幅上昇 |
| `$/progress` に完了セマンティクスがない | (a) | LSP #904, #786 | **B に直結** | 大幅上昇 |
| 応答が最新の文書状態を反映する保証がない(因果性) | (a)(b) | michaelpj; matklad(通知の順序曖昧性) | B の一般化(状態確定性) | 大幅上昇 |
| 起動コマンド・ルート判定・環境(venv 等)が仕様外 | (a) | Zed blog; nvim-lspconfig; Helix `languages.toml` | **C に直結** | 大幅上昇 |
| 「空」の多重表現・型の多義性 | (a) | michaelpj | 周辺(機械可読性) | 上昇 |
| RPC と状態同期の混同・同期方式の非一貫性 | (b) | matklad(最大の欠陥); michaelpj(14 機能バラバラ) | B/A の根本原因層 | 上昇 |
| 拡張性の貧しさ(拡張は断片化を再導入) | (b) | michaelpj; Zed #21133; Helix #11730 | lsp-det が「拡張」として設計する際の前提制約 | 中立 |
| 対話的リファクタリング・多段階操作の欠如 | (b) | matklad; michaelpj | 対象外 | 中立 |
| dynamic registration の複雑さ | (b) | matklad; michaelpj | 対象外 | 中立 |
| フレーミング・JSON-RPC・仕様肥大 | (b)(c) | matklad; michaelpj | 対象外 | 中立 |
| LSIF のグラフ形式・メモリ・ストリーミング不能 | (c) | Sourcegraph | 対象外 | 中立 |
| 保存ブロック等ライフサイクル起因の遅延 | (c) | Helix #7427 | B の周辺 | 上昇 |
| Microsoft 単独ガバナンス・VS Code ファースト | (d) | michaelpj; tsserver の逆説 | 拡張を仕様外で設計する動機そのもの | 中立 |

### 照合の結論

1. **lsp-det の 3 点はいずれも実装者コミュニティで独立に指摘済みの実在欠陥**であり、B(readiness)は仕様リポジトリで 8 年放置(#511)、C(起動)は全エディタでの規約レイヤー再発明、A(範囲)は仕様 issue と SCIP の設計転換が一次証拠になる。
2. ただしコミュニティの批判の重心は歴史的に (b) 状態同期・(d) ガバナンスにあり、A/B/C は「人間ユーザーなら UI と再試行で吸収できる」ため優先度が低く扱われてきた。**エージェントの登場でこの優先順位が反転しつつある**(7 節)ことが、lsp-det の 3 点への絞り込みを外部的に正当化する。
3. 見落としリスクとして、A/B/C の上流には「応答の鮮度保証の不在(因果性)」という共通根(matklad・michaelpj の中核批判)がある。lsp-det B の設計が「初回インデックス完了」だけでなく「特定の didChange までを織り込んだ状態での応答か」を判別可能にするかは、両者の批判と照合して検討する価値がある。

## 主な出典一覧

- matklad: [Why LSP?](https://matklad.github.io/2022/04/25/why-lsp.html) / [LSP could have been better](https://matklad.github.io/2023/10/12/lsp-could-have-been-better.html)
- Michael Peyton Jones: [LSP: the good, the bad, and the ugly](https://www.michaelpj.com/blog/2024/09/03/lsp-good-bad-ugly.html)
- Zed: [Making Python in Zed Fun](https://zed.dev/blog/making-python-in-zed-fun) / [zed#26916](https://github.com/zed-industries/zed/issues/26916) / [zed#21133](https://github.com/zed-industries/zed/issues/21133)
- Helix: [helix#11730](https://github.com/helix-editor/helix/discussions/11730) / [helix#7427](https://github.com/helix-editor/helix/discussions/7427)
- Neovim: [nvim-lspconfig](https://github.com/neovim/nvim-lspconfig) / [LSP & Neovim; A Retrospective!](https://vikasraj.dev/blog/lsp-neovim-retrospective)
- LSP 仕様リポジトリ: [#511](https://github.com/microsoft/language-server-protocol/issues/511) / [#904](https://github.com/microsoft/language-server-protocol/issues/904) / [#786](https://github.com/microsoft/language-server-protocol/issues/786) / [#1778](https://github.com/microsoft/language-server-protocol/issues/1778) / [#1270](https://github.com/microsoft/language-server-protocol/issues/1270)
- rust-analyzer: [#10888](https://github.com/rust-lang/rust-analyzer/issues/10888) / [#15837](https://github.com/rust-lang/rust-analyzer/issues/15837)
- Sourcegraph: [Announcing SCIP](https://sourcegraph.com/blog/announcing-scip) / [SCIP DESIGN.md](https://github.com/sourcegraph/scip/blob/main/docs/DESIGN.md)
- TypeScript: [#39459](https://github.com/microsoft/TypeScript/issues/39459) / [#11274](https://github.com/microsoft/TypeScript/issues/11274) / [typescript-language-server](https://github.com/typescript-language-server/typescript-language-server) / [TypeScript native port](https://devblogs.microsoft.com/typescript/typescript-native-port/)
- エージェント関連: [Lanser-CLI / RLCSF (arXiv:2510.22907)](https://arxiv.org/abs/2510.22907) / [LSP vs the full JetBrains IDE stack](https://medium.com/@explyt.ai/lsp-vs-the-full-jetbrains-ide-stack-what-an-ai-agent-misses-without-the-ide-platform-273a2fd1874c) / [Beyond Prompt Guessing](https://dev.to/tamizuddin/beyond-prompt-guessing-why-lsp-integration-is-the-missing-protocol-for-reliable-ai-coding-agents-i7)
