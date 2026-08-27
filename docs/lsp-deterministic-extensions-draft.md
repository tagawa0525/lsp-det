# LSP Deterministic Extensions — 仕様草案 v0.1

> LSP (Language Server Protocol) の応答を、クライアントが解釈なしに機械的に扱えるようにするための最小限の拡張。
> 既存の LSP を置き換えず、**準拠すれば補正が不要になる** 3 点のみを規定する。
> 動機はコーディングエージェントだが、対象はエディタを含むすべての LSP クライアント。
> 本草案は日本語。上流提案時は英訳する。

---

## 0. 目的と非目的

### 目的

LSP は「エディタが表示するための情報」を返す前提で設計され、以下が意図的に緩い。

| 項目           | LSP の現状                              | エージェントで起きる問題                         |
| -------------- | --------------------------------------- | ------------------------------------------------ |
| シンボルの範囲 | `range` の始点・終点が実装依存          | 範囲を切り貼りすると `type type Foo` 等の破損    |
| 準備完了       | `$/progress` は任意、形式もサーバー依存 | インデックス中の空応答を「結果なし」と誤認       |
| 起動方法       | 仕様外                                  | クライアントごとに 70 言語分の起動コードを再実装 |

この緩さはエディタ時代から問題だった。準備完了の標準化は 2018 年にエディタ側から提案され（LSP issue #511）、VS Code・Neovim・Zed はそれぞれ言語サーバーごとの補正コードを持っている。ただし人間の目とタイミング感覚が最終判断を補っていたため、各エディタが個別に対処すれば済んでいた。

コーディングエージェントはこの補いを持たない。応答を文字通り信じて機械的に切り貼りするため、同じ緩さが直接の破損として現れる。また 1 クライアントで全言語を扱うため、エディタ界に分散していた補正コストが一箇所に積み上がる（Serena の言語別コード約 2.7 万行）。

つまりエージェントは問題を**新しく作った**のではなく、**可視化し、動機を強くした**。本仕様は 3 点を締めることで、エージェントとエディタの両方から補正を消すことを目的とする。

### 非目的

- 合成クエリ（影響分析、呼び出し経路など）の定義。これは本仕様の**上**に置く別レイヤー（LSAP 等）の役割
- 出力形式（Markdown 等）の規定
- MCP との接続方法の規定
- 既存 LSP メソッドの意味変更

### 設計原則

1. **後方互換**: 非準拠サーバーはそのまま動く。準拠は capability で宣言する
2. **要求最小**: サーバーに求めるのは 3 点のみ。これ以上増やさない
3. **準拠テストが本体**: 文章で書いても守られない（`documentSymbol.range` の前例）。テストで検証可能なものだけ規定する
4. **プロキシで先行**: 非準拠サーバーを準拠して見せる参照プロキシを提供し、上流の対応を待たずに使えるようにする

### 締める位置（なぜブリッジ側ではなく LSP 側か）

エージェント向けの厳密な語彙を定義する場所は 2 つある。

```text
(a) ブリッジで締める:  エージェント ── [厳密な API] ── ブリッジ ── [緩い LSP] ── 言語サーバー
(b) LSP 側で締める:    エージェント ── [任意]        ── プロキシ ── [厳密な LSP] ── 言語サーバー
```

(a) は Serena の出力を仕様化・保証した形に相当し、LSAP / LSAI が採っている方式でもある。それだけでも十分に価値があり、実装も早い。

本仕様が (b) を選ぶ理由:

- (a) では補正の責任がブリッジに永続的に集中する。言語サーバーが増えるたび、変わるたびに、ブリッジが追従し続ける。Serena の言語別 2.7 万行はこの構造の帰結
- (b) では補正をプロキシに**一時的に**置き、最終的に言語サーバー側へ押し込んで消すことを目指す。準拠サーバーが増えるほどプロキシは薄くなる
- (b) で締めた語彙は、エージェント以外（エディタ、LSIF、静的解析ツール）も同じ恩恵を受ける。(a) はエージェントに閉じる
- 最終目標を「LSP に取り込まれる」に置いている以上、LSP の型（`DocumentSymbol` 等）の拡張として書く方が提案の形にそのまま使える

(a) の層（合成クエリ、エージェント向けの出力形式）は本仕様の上に別途置く。本仕様は (a) を置き換えるものではなく、(a) の実装から言語別の補正を消すための土台。

---

## 1. 拡張 A: 宣言範囲の契約 (Declaration Range Contract)

### 1.1 問題

`textDocument/documentSymbol` の `DocumentSymbol.range` は仕様上「シンボルを囲む範囲。先頭・末尾の空白を除き、コメント等はすべて含む」とされるが、実装は従っていない。

実例:

- gopls: `type Foo struct` の範囲が `Foo` から始まり `type` を含まない。関数は `func` を含む
- 各サーバーで decorator / attribute / doc comment の扱いが異なる

### 1.2 規定

準拠サーバーは `DocumentSymbol` に以下を追加する。

```typescript
interface DocumentSymbol {
  // 既存フィールドはそのまま
  range: Range;
  selectionRange: Range;

  // 追加
  declarationRange?: DeclarationRange;
}

interface DeclarationRange {
  /**
   * 宣言全体。以下をすべて含む:
   *   - 言語のキーワード (type, func, class, def, fn, ...)
   *   - 修飾子 (pub, static, async, export, ...)
   *   - 直前に付随するアノテーション / デコレータ / 属性
   *   - 本体（存在する場合）と閉じ括弧
   * 以下を含まない:
   *   - doc comment（下の docRange で別途返す）
   *   - 前後の空行
   *   - 末尾のセミコロン以降の改行
   */
  full: Range;

  /**
   * 本体のみ。関数なら { } の内側、クラスなら class body。
   * 本体を持たないシンボル（変数宣言、インポート等）では省略。
   * 括弧自体は含まない。
   */
  body?: Range;

  /**
   * 直前の doc comment。存在しない場合は省略。
   */
  doc?: Range;

  /**
   * 「この範囲を丸ごと置換しても構文的に整合する」ことをサーバーが保証するか。
   * false の場合クライアントは full を切り貼りに使ってはならない。
   */
  replaceable: boolean;
}
```

### 1.3 Capability

```typescript
interface ServerCapabilities {
  documentSymbolProvider?: boolean | {
    // 追加
    declarationRange?: boolean;
  };
}
```

### 1.4 準拠テスト

各言語について、最低限以下のケースを含む fixture を用意し、`full` / `body` / `doc` の期待値を byte offset で固定する。

- キーワード付き宣言（関数、型、クラス）
- 修飾子付き（public、static、async、export）
- アノテーション / デコレータ / 属性付き
- doc comment 付き
- 本体なし（変数、定数、インポート）
- ネストしたシンボル
- 1 行宣言と複数行宣言

期待値の生成はテスト作者が手で行う。サーバーの出力をそのまま期待値にしない。

---

## 2. 拡張 B: 準備完了の通知 (Readiness)

### 2.1 問題

多くのサーバーは `initialize` 完了後もインデックス中で、この間の問い合わせに空配列や部分的な結果を返す。`$/progress` は任意で、進捗の意味（何が終わると何が答えられるか）はサーバー依存。エージェントは「結果なし」と「まだ答えられない」を区別できない。

### 2.2 規定

#### 2.2.1 状態の問い合わせ

```text
Request:  workspace/readiness
Params:   {}
Response: ReadinessState
```

```typescript
interface ReadinessState {
  /**
   * "initializing" : initialize 直後、まだ何も答えられない
   * "indexing"     : 一部のメソッドは答えられるが結果が不完全になりうる
   * "ready"        : すべてのメソッドが完全な結果を返せる
   */
  state: "initializing" | "indexing" | "ready";

  /**
   * indexing 中に完全な結果を返せるメソッドの一覧。
   * state が ready の場合は省略可。
   */
  completeMethods?: string[];

  /**
   * indexing 中、結果が不完全になりうるメソッドの一覧。
   */
  partialMethods?: string[];

  /**
   * 推定残り時間 (ms)。不明なら省略。
   */
  estimatedRemainingMs?: number;
}
```

#### 2.2.2 状態変化の通知

```yaml
Notification: workspace/readinessChanged
Params:       ReadinessState
```

サーバーは `state` が変わるたびに送る。`indexing` 中の細かい進捗は送らなくてよい。

#### 2.2.3 応答への注釈

準拠サーバーは、`indexing` 中に `partialMethods` に含まれるメソッドへ応答する際、応答に以下を付ける。

```typescript
// 任意の応答オブジェクトに追加可能な拡張
interface PartialResultAnnotation {
  $partial?: true;
}
```

配列応答の場合は、JSON-RPC の `result` を `{ items: [...], $partial: true }` で包む形は互換性を壊すため採用しない。代わりに `workspace/readiness` の問い合わせをクライアントに求める。

### 2.3 Capability

```typescript
interface ServerCapabilities {
  readinessProvider?: boolean;
}
```

### 2.4 準拠テスト

- 大規模 fixture（最低 1,000 ファイル）で `initialize` 直後に `workspace/readiness` を呼び、`ready` でないことを確認
- `ready` 通知後に `textDocument/references` を呼び、事前計算した完全な結果と一致することを確認
- `indexing` 中に `completeMethods` に含まれるメソッドを呼び、完全な結果が返ることを確認

---

## 3. 拡張 C: 起動の宣言 (Launch Manifest)

### 3.1 問題

言語サーバーの入手方法、起動コマンド、初期化オプション、プロジェクトルートの判定は仕様外で、各クライアントが言語ごとに実装している。既存の事実上の標準（Claude Code の `.lsp.json`、nvim-lspconfig、Mason registry）は互いに非互換。

### 3.2 規定

言語サーバーの配布物は `lsp-manifest.json` を同梱する、またはレジストリから取得可能にする。

```typescript
interface LaunchManifest {
  /** マニフェスト形式のバージョン */
  manifestVersion: "1";

  /** サーバー識別子。逆 FQDN 推奨 (例: "org.golang.gopls") */
  id: string;

  /** 対応する languageId と拡張子 */
  languages: {
    languageId: string;
    extensions: string[];
    filenames?: string[];   // "Makefile" 等
  }[];

  /** 起動方法 */
  launch: {
    command: string;
    args?: string[];
    env?: Record<string, string>;
    transport: "stdio" | "socket" | "pipe";
    /** socket の場合のポート指定方法 */
    socket?: { arg: string };
  };

  /** 入手方法。省略時は PATH 上にあることを期待する */
  install?: {
    /** 検出コマンド。exit 0 なら利用可能 */
    detect: { command: string; args?: string[] };
    /** バージョン取得。stdout をそのまま返す */
    version?: { command: string; args?: string[] };
    /** プラットフォーム別の入手方法。クライアントは実行前にユーザー承認を得る */
    sources?: {
      platform?: ("linux" | "darwin" | "windows")[];
      arch?: ("x64" | "arm64")[];
      /** package manager 経由 */
      package?: { manager: "npm" | "pip" | "cargo" | "go" | "brew" | "apt"; name: string };
      /** アーカイブ直接取得 */
      archive?: { url: string; sha256: string; binaryPath: string };
    }[];
  };

  /** プロジェクトルートの判定。上から順に評価し最初に見つかったものを使う */
  rootMarkers: string[];   // ["go.work", "go.mod", ".git"]

  /** initialize の initializationOptions として渡す既定値 */
  initializationOptions?: Record<string, unknown>;

  /** 本仕様の準拠状況 */
  conformance?: {
    declarationRange?: boolean;
    readiness?: boolean;
  };

  /**
   * 準拠していない場合の補正ルール。プロキシが適用する。
   * ここに書けない補正（言語固有の複雑なロジック）はプロキシのコードで持つ。
   */
  shims?: Shim[];
}

interface Shim {
  /** どのシンボル種別に適用するか */
  symbolKinds: number[];   // SymbolKind の値
  /** range の始点を、指定パターンまで前方に拡張する */
  extendStartToPattern?: string;   // 正規表現。例: "^\\s*(pub\\s+)?type\\s+"
  /** decorator/attribute を含める */
  includeLeadingAttributes?: boolean;
}
```

### 3.3 準拠テスト

- マニフェストの JSON Schema 検証
- `detect` → `launch` → `initialize` → `shutdown` が成功すること
- `rootMarkers` に従って正しいルートが選ばれること（モノレポ fixture）

---

## 4. 参照プロキシ (Reference Proxy)

上流サーバーの準拠を待たずに本仕様を使えるようにするため、以下の動作をするプロキシを参照実装として提供する。

### 4.1 動作

```text
Agent ──[LSP + 拡張 A/B]── Proxy ──[LSP]── Language Server
```

- クライアントからは準拠サーバーに見える
- 上流が拡張 A に準拠していれば透過。していなければマニフェストの `shims` と言語別補正コードで `declarationRange` を合成する
- 上流が拡張 B に準拠していれば透過。していなければ `$/progress` の監視、既知の初期化完了パターン、タイムアウトから `ReadinessState` を推定する
- マニフェストに従って上流を起動する

### 4.2 設計制約

- 単一バイナリ、外部ランタイム依存なし（Rust を想定）
- 言語追加はマニフェスト追加のみで可能にする。言語別コードは `shims` で表現できない補正に限る
- プロセス寿命は親に従う。親が死んだら自分も死ぬ（`PR_SET_PDEATHSIG` / Job Object / kqueue）
- キャッシュは上限付き。無制限に持たない
- ダッシュボード等の付随機能は持たない

### 4.3 初期対応言語

gopls, rust-analyzer, typescript-language-server, pyright, clangd の 5 つ。
それぞれに準拠テストの fixture を用意する。

---

## 5. 上流への提案経路

1. 5 言語のプロキシと準拠テストを公開
2. 各上流に「`declarationRange` と `workspace/readiness` を実装すれば、プロキシの補正コードが N 行消える」という issue を実測付きで立てる。gopls と rust-analyzer を先に
3. 上流が 1 つでも取り込んだら、LSP 本体（microsoft/language-server-protocol）に proposal を出す。拡張 B は既存 issue #511 のスレッドに「エージェント用途からの再提案」として接続し、拡張 A は新規 issue とする。`proposed` 状態の拡張として `3.18` 以降のサイクルに載せることを目標にする。拡張 C は LSP 本体の対象外なので別仕様として出す
4. LSAP 等の上位レイヤーには、本仕様を前提とすることで自前の補正を消せることを提示し、依存してもらう

---

## 6. 先行調査 (Prior Art)

仕様を書く前に、各実装が同じ問題をどう解いているかを調べる。以下は初期調査の結果と、追って確認すべき項目。

### 6.1 LSP 本体

| 項目                                                       | 内容                                                 | 本仕様との関係                                                |
| ---------------------------------------------------------- | ---------------------------------------------------- | ------------------------------------------------------------- |
| `DocumentSymbol.range` の仕様文                            | 「先頭・末尾の空白を除き、コメント等を含む囲む範囲」 | 拡張 A が締める対象。準拠テストがないため守られていない       |
| `selectionRange`                                           | 名前のみの範囲。3.10 で `range` と分離された         | 拡張 A の `body` / `full` は同じ発想の延長                    |
| `$/progress` / `window/workDoneProgress`                   | 3.15 で追加。任意。トークンとタイトルは自由          | 拡張 B の前身。「何が終わると何が答えられるか」を表現できない |
| `workspace/didChangeWatchedFiles` 等の capability 宣言方式 | 任意機能は capability で宣言                         | 拡張 A/B の宣言方式はこれに合わせる                           |

#### 3.18 の新機能（競合確認済み・2026-08-27 時点）

3.18 で `@since 3.18.0` が付いた項目は以下。**本仕様の 3 拡張と重なるものはない。**

- `textDocument/inlineCompletion`（インライン補完）
- `workspace/textDocumentContent`（仮想ドキュメントの内容提供）
- `workspace/foldingRange/refresh`
- `SnippetTextEdit`（スニペット形式の編集）
- `DocumentFilter` の相対パターン対応
- `Diagnostic.message` の `MarkupContent` 対応
- `WorkspaceEditMetadata`
- `RegularExpressionEngineKind`
- languageId の追加（D, Pascal 等）
- `Command.tooltip`

方向性としては「エディタ UI の充実」が中心で、エージェント向けの決定性・ライフサイクルに関する項目は入っていない。

#### 関連する既存 issue（microsoft/language-server-protocol）

| Issue                                                        | 内容                                                                                                                                                                | 状態              | 本仕様との関係                                                                                                                                                                                                                                                                                                                                             |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| #511 "Discussion: LSP-server readiness indicator"（2018-06） | Java 拡張・OmniSharp が各自でステータスバーに準備状態を出している。標準化を提案し、`window/showStatus`（type / actions / message / shortMessage）を実装例として提示 | **Open、Backlog** | 拡張 B に最も近い先行提案。ただし**人間向け UI（ステータスバー表示）**を目的としており、機械可読な「どのメソッドが完全な結果を返せるか」は含まない。拡張 B は #511 を引用しつつ「表示ではなく判定のための状態」として差別化する。8 年放置されているのは、UI 目的なら各エディタが独自にやれば済んだからで、エージェント用途という新しい動機を示す必要がある |
| #54 "Clarification for the Indexing workflow"（2016-08）     | インデックス構築中のクライアント・サーバー間の通信が仕様にない                                                                                                      | 古い              | 拡張 B の問題意識と同一。最初期から認識されていた穴                                                                                                                                                                                                                                                                                                        |
| #312 "filtering documentSymbol operation"（2017）            | 範囲指定で documentSymbol を絞る提案                                                                                                                                | —                 | 拡張 A とは別の話。無関係                                                                                                                                                                                                                                                                                                                                  |

`declarationRange` に相当する提案（キーワード・デコレータを含む完全な宣言範囲の規定）は、追加検索でも**見つからなかった**。関連して見つかったのは #613（`document/extendSelection`、後の `selectionRange` の起源）と #1270（`selectionRange` の null 応答の扱い）で、いずれも「範囲の意味を厳密化する」提案ではない。`DocumentSymbol.range` の仕様文（"not including leading/trailing whitespace but everything else like comments"）は `LocationLink.targetRange` からの流用で、3.14 から一度も改訂されていない。**拡張 A は新規提案として出せる。**

起動マニフェスト（拡張 C）に相当する提案は LSP 本体には存在しない。LSP は「サーバーの起動はクライアントの責任」と明記しており、仕様の対象外という立場。拡張 C は LSP 本体への提案ではなく、別仕様（レジストリ）として出すのが筋。

#### Base Protocol 0.9 (Upcoming) — 確認済み

LSP から「言語サーバーに依存しない共通部分」を切り出した仕様。capability 交換、initialize / initialized / shutdown / exit、request / notification の構造、cancel、progress、window/showMessage 等を含む。目的は、LSP 以外のプロトコル（デバッグ等）でも同じ土台を使えるようにすること。

拡張との関係:

- 拡張 A（宣言範囲）: 無関係。LSP 側の `DocumentSymbol` の話
- 拡張 B（準備完了）: **Base Protocol 側に置く方が筋が良い可能性がある**。「サーバーが要求に完全に答えられる状態か」は言語サーバーに限らない概念で、Base Protocol のライフサイクル（initialize 〜 shutdown）の一部として提案できる。ただし `completeMethods` / `partialMethods` はメソッド名に依存するので、Base Protocol には `state` の 3 値のみを置き、メソッド一覧は LSP 側の拡張とする二層構成も検討する
- 拡張 C（起動）: Base Protocol も起動方法は対象外。変わらず別仕様

Base Protocol はまだ 0.9 で、既存 issue から切り出された capability 名の予約リストを持っている。拡張 B の capability 名（`readinessProvider`）が将来の予約と衝突しないか、提案時に確認する。

### 6.2 各言語サーバーの「準備完了」の実態

solidlsp の言語別クラスが実際に何を待っているかを読んだ結果。拡張 B の必要性を示す**一次証拠**であり、上流提案時にこの表をそのまま使う。

| サーバー                   | 準備完了の判定方法（solidlsp の実装）                                                                                                                                                                                                                                         | 評価                                                                                                                                           |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| rust-analyzer              | 独自拡張 `experimental/serverStatus` の `quiescent: true` を待つ。タイムアウト付き                                                                                                                                                                                            | 最も機械可読。拡張 B に最も近い既存実装                                                                                                        |
| jdtls                      | 独自通知 `language/status` の `type: "ServiceReady"` と `type: "ProjectStatus", message: "OK"` の 2 段階を待つ                                                                                                                                                                | 機械可読だが完全に独自形式。「サービス準備」と「プロジェクト準備」を区別している点は拡張 B の `initializing` / `indexing` / `ready` と対応する |
| pyright                    | `window/logMessage` の**本文を正規表現** `Found \d+ source files?` でマッチして判定。コード内コメントに「pyright は信頼できず、これより良い方法がない」とある。`experimental/serverStatus` も補助的に監視                                                                     | **人間向けログを機械が読んでいる**。仕様がないことの典型例                                                                                     |
| typescript-language-server | `$/progress` の `end` を待つが、tsserver がクラッシュした時も同じ `end` が来て区別できないため、`window/logMessage` の異常終了パターンを別途監視。「インデックスを始めるまでの猶予」（5 秒）、「インデックス完了」（30 秒）、「準備完了」（10 秒）の 3 つのタイムアウトを持つ | `$/progress` が「完了」と「中断」を区別できない実例                                                                                            |
| clangd                     | 判定なし。コード内コメントに「clangd は準備完了時に意味のある通知を送らない」「これはイベントの目的を無にしている」とある                                                                                                                                                     | **判定不能**                                                                                                                                   |
| gopls                      | 判定なし。「通常 initialize 直後に準備完了」とコメント                                                                                                                                                                                                                        | 実際は大規模モジュールで空応答が返る（Serena issue #890 等）が、判定手段がないので無視している                                                 |

**まとめ**: 5+1 サーバーで、機械可読な準備完了通知を持つのは rust-analyzer と jdtls の 2 つのみで、しかも形式が異なる。残りは `$/progress` の曖昧さ、ログの正規表現、または諦め。これが「LSP に準備完了の標準がない」ことの実害であり、拡張 B の根拠になる。

**上流提案の優先順**: rust-analyzer（既に概念を持つ）→ jdtls（同上、形式変更のみ）→ pyright（Microsoft 製。ログ依存の現状を示せば動機は明確）→ gopls → clangd → tsserver。

### 6.3 rust-analyzer: `experimental/serverStatus`

rust-analyzer は独自拡張として `experimental/serverStatus` 通知を送り、`quiescent: true` でインデックス完了を伝える。

- 拡張 B の**最も近い既存実装**。`health` / `quiescent` / `message` の 3 フィールド
- pyright と clangd も solidlsp 側で同じ通知名を監視している（rust-analyzer に倣った実装が他サーバーにも広がりつつある可能性。**要調査**: pyright / clangd が実際に `experimental/serverStatus` を送っているか、それとも solidlsp が念のため監視しているだけか）
- 拡張 B は `experimental/serverStatus` の後継として、`quiescent` を `state` の 3 値に拡張し `completeMethods` を足した形と位置づけると、rust-analyzer 側の移行コストが最小になる

### 6.4 エディタ側の実装

#### VS Code

- `vscode-languageclient` が LSP クライアント。起動方法は各拡張が `ServerOptions` で指定
- 言語サーバーは拡張が**バンドル**するのが慣行。バージョン固定と引き換えに、外部からは使えない
- 範囲の補正は各拡張の中に閉じている
- **要調査**: vscode-languageclient に範囲や準備完了の共通処理があるか

#### Neovim: nvim-lspconfig + Mason

- nvim-lspconfig: 言語ごとに `cmd`, `filetypes`, `root_markers`, `settings` を Lua テーブルで宣言。起動と初期化オプションの**事実上の標準スキーマ**
- Mason registry: `package.yaml` に `source.id` を purl 形式（`pkg:npm/...`, `pkg:github/...`, `pkg:golang/...`）で記述し、`bin` で実行ファイルを指定。入手方法の**事実上の標準**。extra_packages、build ステップ、schemas.lsp（設定スキーマの URL）も持つ
- 拡張 C のマニフェストは、この 2 つの合成に近い。独自形式を作るより **purl と nvim-lspconfig の語彙を流用**する方が採用されやすい
- **要調査**: Neovim 0.11 の `vim.lsp.config` で lspconfig の形式がどう変わったか

#### Zed

- `LspAdapter` トレイト（初期化パラメータ、環境変数、補完ラベル）と `LspInstaller` トレイト（バイナリの検出・取得・キャッシュ）を分離。`check_if_user_installed()` で PATH と toolchain 内を先に探し、なければ自前で取得。SHA-256 で検証
- 拡張 C の `install.detect` → `sources` の順序は Zed と同じ設計
- Rust 実装なので、参照プロキシの設計上参考にできる部分が多い
- 拡張機能は WASM で書き、同じトレイトを実装する
- **要調査**: Zed の rust-analyzer アダプタが `serverStatus` をどう扱っているか。範囲補正の有無

#### Helix

- `languages.toml` で `command`, `args`, `roots`, `config` を宣言。nvim-lspconfig と同型
- 自動インストールなし。「ユーザーが入れる」方針で、Claude Code の LSP プラグインと同じ割り切り
- **要調査**: なし（設計が単純なので参照のみ）

#### Emacs: lsp-mode / eglot

- lsp-mode は自動インストール機構あり、eglot はなし。両極の実装が同じエディタにある
- **要調査**: lsp-mode の `lsp-dependency` 機構の宣言形式

#### JetBrains

- ネイティブは LSP 非依存。2023 以降 LSP API を持つが補助的
- Serena は JetBrains をバックエンドとして使う経路も持つ（LSP ではない）
- 本仕様の対象外だが、「LSP より厳密な IDE 内部モデル」として範囲定義の参考になる

### 6.5 Serena / solidlsp

コードベース: oraios/serena `src/solidlsp/`（約 4.2 万行、うち言語別 74 ファイル約 2.7 万行）

**構造**

- `SolidLanguageServer` 基底クラスを言語ごとに継承。multilspy の設計を継承
- 言語別クラスが上書きしているフック（頻度順）:
  - `__init__` (78), `_start_server` (75), `_create_base_initialize_params` (75) — 起動と初期化。拡張 C が吸収する部分
  - `is_ignored_dirname` (56) — 無視ディレクトリ。マニフェストに入れるべき項目として**拡張 C に追加検討**
  - `_create_dependency_provider` (44), `_setup_runtime_dependencies` (16) — 入手。拡張 C
  - `_get_wait_time_for_cross_file_referencing` (14) — 準備完了の代替として**固定秒数の待ち**を言語ごとに持っている。拡張 B が消す対象
  - `_document_symbols_cache_fingerprint` (10), `_normalize_symbol_name` (9), `request_document_symbols` (8) — 範囲・シンボルの補正。拡張 A が消す対象
  - `request_text_document_diagnostics` (8) — pull/push 診断の違いの吸収
- `dependency_provider.py` — 入手方法の抽象化。Mason の purl に相当するが独自形式

**本仕様にとっての価値**

- 74 言語分の「どこが標準から外れているか」の**実測記録**。各クラスの上書き内容を読めば、準拠テストの fixture に入れるべきケースが分かる
- 特に `_get_wait_time_for_cross_file_referencing` の値と `request_document_symbols` の上書きは、拡張 A/B の必要性を示す証拠として上流提案に使える
- **要調査**: 74 クラスの上書き内容を分類し、「マニフェストで表現可能」「shims で表現可能」「コードが必要」の 3 つに振り分ける。これが拡張 C の `Shim` の設計根拠になる

**引き継がない点**

- 同期 API、Python 依存、ダッシュボード、無制限キャッシュ、プロセス寿命管理（Issues #944, #1277, #1281, #1367, #1387, #1488）

### 6.6 先行するエージェント向けプロトコル

#### LSAP (lsp-client/LSAP)

- 38 スター、v0.2.0（2026-01）、MIT、Python 主体
- 「LSP は原子的操作、LSAP は認知的能力」と位置づけ、合成クエリ（locate + references + context 抽出を 1 リクエスト）を JSON Schema で定義。Markdown の描画テンプレートまで標準化
- rename は preview → execute の 2 段階
- 本仕様の**上位レイヤー**に相当。競合ではなく、本仕様に依存してもらう相手
- **要調査**: LSAP が内部で範囲・準備完了をどう扱っているか。自前補正があれば本仕様で消せることを示す

#### LSAI (LadislavSopko/lsai-protocol)

- 3 スター、2026-05、**CC BY-NC 4.0**（商用実装は別ライセンス）
- 14 の意味的ツール（`impact`, `context` 等の合成含む）。上流 LSP が機能を欠く場合の**フォールバック戦略を仕様に含める**点は本仕様に近い
- 参照実装 Zerox.Lsai は 10 言語、E2E 検証済み
- ライセンスのため標準にはなり得ないが、フォールバック戦略の分類は参考になる
- **要調査**: spec/LSAI-v1.4.md のフォールバック定義を読み、拡張 C の `Shim` に流用できる分類がないか

#### その他の MCP ブリッジ

- claude-code-lsps (Piebald-AI)、boostvolt/claude-code-lsps: Claude Code の `.lsp.json` 形式のプラグイン集。宣言形式の実例
- cclsp (ktnyt)、mcpls (bug-ops): LSP → MCP の薄いブリッジ
- code-yeongyu/codex-lsp: Codex 向け。編集後フックで診断を返す設計
- **要調査**: 各ブリッジが範囲・準備完了をどう扱っているか（おそらく未対応。未対応であること自体が本仕様の根拠）

### 6.7 調査タスク一覧

優先順:

1. [x] LSP 3.18 に競合する proposal がないか確認 → なし（6.1 参照）。補足で issue #511 を引用対象に追加
1b. [x] Base Protocol 0.9 の内容確認 → 拡張 B を Base Protocol 側に置く案を追加（6.1）
1c. [x] declarationRange 相当の先行提案 → なし。新規提案可（6.1）
2. [x] gopls / tsserver / pyright / clangd / jdtls の準備完了通知の有無を確認 → 6.2 に表として記載
2b. [ ] pyright / clangd が実際に `experimental/serverStatus` を送るか、各リポジトリのソースで確認
3. [ ] solidlsp の 74 クラスの上書き内容を分類（マニフェスト / shims / コード）
4. [ ] Neovim 0.11 `vim.lsp.config` と Mason `package.yaml` の語彙を拡張 C に取り込む
5. [ ] Zed の `LspInstaller` の設計を参照プロキシに取り込む
6. [ ] LSAP の内部実装で範囲・準備完了の補正がどこにあるか確認
7. [ ] LSAI v1.4 のフォールバック分類を読む
8. [ ] 5 言語について `documentSymbol.range` の実際の返り値を fixture で採取し、仕様文との乖離を表にする（上流提案の証拠）

## 7. 未決事項

- `declarationRange.full` に末尾のコメント（同一行の trailing comment）を含めるか
- `readiness` を workspace 単位でなくファイル単位で返す必要があるか（大規模モノレポ）
- マニフェストのレジストリを誰がホストするか。当面は Git リポジトリで十分
- LSAP との役割分担の明文化
- 名前は「Deterministic Extensions」に変更済み。エディタ側の実装者にも自分事として読まれるか、提案前に数人に読んでもらって確認する

---

## 付録: 既存仕様との対応

| 本仕様              | 関連する既存 LSP 項目                     | 関係                             |
| ------------------- | ----------------------------------------- | -------------------------------- |
| A. declarationRange | `DocumentSymbol.range` / `selectionRange` | 追加。既存は変更しない           |
| B. readiness        | `$/progress`, `window/workDoneProgress`   | 追加。既存は補助情報として併用可 |
| C. manifest         | なし（仕様外）                            | 新規                             |
