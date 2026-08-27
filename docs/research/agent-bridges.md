# エージェント向け LSP ブリッジ実装調査

> lsp-det（readiness ゲート付き透過 LSP プロキシ）の設計検証のため、`reference/` 配下の
> LSAP・lsai-protocol・cclsp・mcpls・codex-lsp・claude-code-lsps（piebald / boostvolt）を調査した。
> 引用は `reference/` からの相対パスと行番号で示す（浅い clone の HEAD 時点、2026-08 取得）。

## 要約

- **調査した範囲に、言語サーバーの「準備完了」を正しく待つブリッジは存在しない。**
  cclsp は固定スリープ（初期化 3 秒上限 + didOpen 後 200ms）による近似のみ、
  mcpls は `$/progress` を受信するが**破棄**しており、LSAP は待ち・リトライを一切持たない。
- **空応答はどのブリッジでも素通しでエージェントに返る。** インデックス未完了の `[]` を
  「結果なし」と区別する仕組みはどこにもない。lsp-det が対象とする問題は全ブリッジで未解決。
- lsai-protocol のフォールバック分類は**機能欠落（capability の不在）への代替**であり、
  **時間軸（準備完了前）の空応答**は扱っていない。lsp-det とは補完関係にあり重複しない。
- 範囲（range）の補正はどこにもない。mcpls の position encoding 変換（UTF-8/16/32）が
  唯一の「補正インフラ」だが、対象は文字単位の変換であり range の広さ・準備完了とは無関係。
- Claude Code の `.lsp.json` は「単一 `command` + `args`」形式で、シェルスクリプトを
  `command` に指定する実例（piebald の vue-volar）もある。**lsp-det を `command` に挟む
  v0.1 設計の書き方はそのまま通る。**

## 調査対象

| リポジトリ | 種別 | 実装言語 | 役割 |
| --- | --- | --- | --- |
| LSAP | オーケストレーション層 SDK | Python | LSP を「認知的」API に合成 |
| lsai-protocol | プロトコル仕様書 | (Markdown) | AI 向け 14 ツールの契約とフォールバック定義 |
| cclsp | MCP-LSP ブリッジ | TypeScript (Bun) | エージェントに LSP ツールを提供 |
| mcpls | MCP-LSP ブリッジ | Rust | 同上 |
| codex-lsp | Codex プラグイン | TypeScript | PostToolUse 診断フック + MCP ツール |
| claude-code-lsps ×2 | CC プラグイン集 | (.lsp.json) | 言語サーバー起動定義のマーケットプレイス |

## 1. LSAP: 準備完了・空応答の扱い

### アーキテクチャ

Python SDK。`definition` / `inspect` / `locate` / `outline` / `reference` / `rename` / `search` の
capability クラス群が LSP の原子操作を合成し、Markdown レポートを返す
（`LSAP/src/lsap/capability/`）。**LSP クライアント本体（サーバー起動・initialize・通信）は
外部依存 `lsp-client` に委譲しており、本リポジトリにサーバー起動コードは存在しない**
（`LSAP/pyproject.toml:20-21` に `lsp-client>=0.3.6`, `lsprotocol>=2025.0.0`。
`src/` に `subprocess` / `Popen` / spawn 系の記述は 0 件）。

### 準備完了の待ち・リトライ: なし

`src/lsap/` 全体を `sleep` / `retry` / `ready` / `indexing` / `progress` / `timeout` で grep して
**0 件**。空応答はそのまま `None` / `[]` として返す。

- `LSAP/src/lsap/capability/definition.py:68` — `if not locations: return None`
- `LSAP/src/lsap/capability/reference.py:64` — `if not locations: return []`
- `LSAP/src/lsap/capability/search.py:42` — `if result is None:`（同様に打ち切り）

つまり、サーバーがインデックス未完了で `[]` を返せば LSAP はそれを「結果なし」として
整形して返す。lsp-det が問題視する誤認がそのまま上位へ伝播する構造である。

### 補正コードの有無

- **range の補正: なし。** `inspect.resolve` は `documentSymbol` の `range` を信頼して
  そのままスニペット切り出しに使う（`LSAP/src/lsap/capability/inspect.py:95-124`）。
- **位置の決定は独自機構がある**（応答の補正ではなく要求側の位置解決）。
  `locate` capability は `<|>` マーカー入りテキストアンカーを柔軟な空白許容の正規表現に
  変換して位置を特定する（`LSAP/src/lsap/utils/locate.py:15-49` のマーカー検出、
  `src/lsap/capability/locate.py:30-56` の `_to_regex`、`:110-149` の `_find_position`）。
  「行・桁を LLM に指定させると壊れる」問題への対処であり、lsp-det の宣言範囲拡張とは
  別軸のクライアント側ワークアラウンド。
- **capability 欠落時の対応はエージェントへの助言文言**。プログラム的フォールバックではなく、
  「declaration が無ければ definition モードを使え」等のエラーメッセージを返す
  （`LSAP/src/lsap/capability/definition.py:52-58`, `reference.py:53-59`）。

## 2. lsai-protocol: フォールバック戦略の分類

`lsai-protocol/spec/LSAI-v1.4.md` は「Fallback Resilience」を設計原則 #9 として明文化する
（`spec/LSAI-v1.4.md:40`）。分類は以下の通り（`spec/LSAI-v1.4.md:545-554` の表と、
各ツール定義の規範文）。

| ツール | 欠けている LSP メソッド | 代替手段 | 規範箇所 |
| --- | --- | --- | --- |
| `callers` | `callHierarchy/incomingCalls` | `textDocument/references`（宣言除外）+ 各参照位置を `documentSymbol` で包含関数に解決 | LSAI-v1.4.md:185 |
| `callees` | `callHierarchy/outgoingCalls` | `documentSymbol` でメソッド本体範囲を取得 → 正規表現で呼び出し識別子を抽出 → `workspace/symbol` で解決（C++ の `Calc::compute` は修飾子を剥がして照合） | LSAI-v1.4.md:199 |
| `hierarchy` | `prepareTypeHierarchy` | エラーにせず最小ノード（名前 + kind、関係は空）を返す | LSAI-v1.4.md:226 |
| `impact` | （内部で callers を使用） | callers 側の例外を捕捉し usages のみの縮退結果を返す（MUST） | LSAI-v1.4.md:255 |
| `rename` | `rename` 非対応（intelephense 無償版等） | クラッシュせず `ToolNotSupported` エラーを返す | LSAI-v1.4.md:280 |

補足:

- Tier 制度（`spec/LSAI-v1.4.md:575-583`）: Tier 1 は任意の LSP サーバーで動く 9 ツール、
  Tier 2 は callHierarchy 系（フォールバック込み）、Tier 3 は合成ツール。
- 既知のサーバー限界 10 項目を列挙（`spec/LSAI-v1.4.md:556-572`。clangd の
  outgoingCalls 未実装、jdtls の外部依存未インデックス等）。
- **準備完了の扱いは仕様化されていない。** `workspace_list` が「readiness status」を
  返すと一言あるのみで（`spec/LSAI-v1.4.md:473`）、ゲート・待ち合わせの意味論はない。
  Live Editing 節（`spec/LSAI-v1.4.md:617-629`）も「didChange 後にサーバーが再インデックスし
  次のツール呼び出しに反映される」と述べるだけで、再インデックス完了を待つ契約はない。

**評価**: LSAI のフォールバックはすべて「機能が無い」場合の空間軸の代替であり、
「機能はあるがまだ準備できていない」時間軸の問題（lsp-det の対象）は分類自体に存在しない。

## 3. cclsp / mcpls / codex-lsp の比較

### 3.1 cclsp

**(a) 起動設定** — `cclsp.json`（環境変数 `CCLSP_CONFIG_PATH` 可）。`servers[]` に
拡張子とコマンドを **文字列配列** で書く（`cclsp/cclsp.json:1-27`。
`npx -- typescript-language-server --stdio` のようにランチャ込みで指定可能）。
フィールド定義は `cclsp/src/types.ts:3-6`:

```typescript
command: string[];
rootDir?: string;
restartInterval?: number; // in minutes, optional auto-restart interval
initializationOptions?: unknown; // LSP initialization options
```

**(b) 準備待ち** — **固定時間の近似のみ**。2 段構え:

1. initialize 応答後、サーバーからの `initialized` **通知**を最大 3 秒待ち、タイムアウトしたら
   「initialized 扱い」で先に進む（`cclsp/src/lsp/server-manager.ts:277` の
   `INITIALIZATION_TIMEOUT = 3000`、`:279-294` の `Promise.race` とタイムアウト時の続行、
   `:339-347` の `initialized` 受信ハンドラ）。
   なお LSP 仕様に「サーバー→クライアントの `initialized` 通知」は存在しないため、
   標準準拠サーバーに対しては実質**常に 3 秒待って進む**コードである。
2. ファイルを初めて didOpen した直後に **200ms 固定スリープ**
   （`cclsp/src/lsp/operations.ts:204-206` の findDefinition、`:264-266` の findReferences。
   ログ文言は "waiting for server to index project"）。

その他の時間対策: pyright 用アダプタが definition 45 秒 / references 60 秒へタイムアウトを
延長（`cclsp/src/lsp/adapters/pyright.ts:34-44`）。`restartInterval` による分単位の
定期再起動（`cclsp/src/lsp/server-manager.ts:365-380`）。

**(c) 空応答** — **素通し**。definition が空なら `return []`
（`cclsp/src/lsp/operations.ts:246`、references は `:300`）。リトライなし。
別レイヤーのフォールバックは存在する: シンボル名検索で kind 不一致時に全 kind へ
再検索して警告を付す（`operations.ts:512-553`）、`documentSymbol` の range 内を
テキスト走査してシンボル名の実位置を求める（`operations.ts:137-183`）。
いずれも「インデックス未完了の空応答」への対処ではない。

### 3.2 mcpls

**(a) 起動設定** — TOML（`~/.config/mcpls/mcpls.toml`、`--config` 指定可。プロジェクト直下の
`mcpls.toml` は既定で無視・要 `--trust-project-config`）。`[[lsp_servers]]` に
`language_id` / `command`（単一文字列）/ `args` / `env` / `file_patterns` /
`initialization_options` / `timeout_seconds` / `request_timeout_seconds` 等
（`mcpls/crates/mcpls-core/src/config/server.rs:150-214`、実例は
`mcpls/examples/mcpls.toml:67-72`）。

**(b) 準備待ち** — **なし**。`$/progress` 通知は型としてパースするが
（`mcpls/crates/mcpls-core/src/lsp/types.rs:175-179`）、受信側で**明示的に捨てている**:

```rust
// mcpls/crates/mcpls-core/src/lib.rs:235
LspNotification::Progress { .. } | LspNotification::Other { .. } => {}
```

存在する「ゲート」は capability ゲートのみ（`prepare_gated_document`、
`mcpls/crates/mcpls-core/src/bridge/translator/routing.rs:216`。サーバーが
`definitionProvider` 等を宣言していなければ `CapabilityNotSupported` エラー）。
これは lsp-det の readiness ゲートとは別物（宣言の有無であって準備完了ではない）。
クラッシュ時はバックオフ付き respawn を持つ
（`mcpls/crates/mcpls-core/src/bridge/translator/respawn.rs:193-263`）。

**(c) 空応答** — **素通し**。hover の `None` は "No hover information available" に整形
（`mcpls/crates/mcpls-core/src/bridge/translator/navigation.rs:113-116`）、definition の
空応答は空の locations リストとして返す（`navigation.rs:160-167`）。リトライなし。
なお position encoding（UTF-8/16/32）の相互変換という本物の補正層を持つが
（`mcpls/crates/mcpls-core/src/bridge/encoding.rs:95,174-235`）、これは文字オフセットの
正規化であり、range の広さや準備完了の補正ではない。

### 3.3 codex-lsp

**(a) 起動設定** — `.codex/lsp-client.json`（プロジェクト）/ `~/.codex/lsp-client.json`（ユーザー）。
言語名をキーに `command`（文字列配列）と `extensions` を書く（`codex-lsp/README.md:52-66`、
`codex-lsp/skills/lsp/SKILL.md:22-33`）:

```json
{
  "lsp": {
    "typescript": {
      "command": ["typescript-language-server", "--stdio"],
      "extensions": [".ts", ".tsx", ".js", ".jsx"]
    }
  }
}
```

未設定言語には組み込みサーバー定義を使う（`codex-lsp/README.md:67`）。

**(b)(c) 準備待ち・空応答** — **本リポジトリ内では検証不能**。LSP ランタイム本体
（クライアント・サーバー管理・ツール実装）は git サブモジュール
`packages/lsp-tools-mcp`（gitlink `e7c65b0`）に分離されており、この clone では
**中身が空**（`codex-lsp/README.md:11` に明記。`src/` に残るのは CLI ルーティング
`src/cli.ts` と PostToolUse フック `src/codex-hook.ts` のみ）。
リポジトリ内に見える範囲では待ち・リトライのコードは存在しない。

見える範囲の挙動として重要なのは診断フック: 編集系ツールの成功後に対象ファイルの
`severity: "error"` 診断を取り、空文字列・"No diagnostics found"・"No LSP server configured"
をすべて「問題なし＝フック出力なし」として扱う（`codex-lsp/src/codex-hook.ts:27-34` の定数、
`:98-104` の `isCleanDiagnostics`）。サーバーが解析未完了で診断ゼロを返した場合も
「クリーン」と区別できない構造であり（推定）、lsp-det が対象とする誤認と同型の問題を含む。

### 3.4 比較表

| 項目 | cclsp | mcpls | codex-lsp |
| --- | --- | --- | --- |
| 設定形式 | JSON（`cclsp.json`）、`command: string[]` | TOML、`command: 文字列` + `args` | JSON、`command: string[]` |
| initialize 後の準備待ち | 擬似 `initialized` 通知を最大 3 秒 + didOpen 後 200ms 固定 | なし（`$/progress` は破棄） | リポジトリ内では確認不能（ランタイムはサブモジュール） |
| 空応答の扱い | 素通し（`[]` 返却） | 素通し（整形のみ） | 同上（診断ゼロ＝クリーン扱い） |
| リトライ | なし | なし（クラッシュ respawn のみ） | 確認不能 |
| range/位置の補正 | documentSymbol range 内のテキスト走査で位置決定 | position encoding 変換のみ | 確認不能 |
| lsp-det の挟み込み | `command` 配列の先頭を差し替えれば可 | `command`+`args` 差し替えで可 | `command` 配列差し替えで可 |

## 4. claude-code-lsps ×2: `.lsp.json` の実例

### フィールドの実態（全 63 定義の集計）

両リポジトリの `.lsp.json` 計 63 個に現れるフィールドの出現数:

| フィールド | 出現数 | 備考 |
| --- | --- | --- |
| `command` | 63 | 必須。単一実行ファイル名（PATH 解決）またはシェル |
| `extensionToLanguage` | 63 | 必須。拡張子 → languageId |
| `args` | 54 | 典型は `["--stdio"]` |
| `settings` / `initializationOptions` | 42 / 41 | piebald は空でも常置、boostvolt は省略 |
| `transport` | 41 | piebald のみ。常に `"stdio"` |
| `maxRestarts` | 41 | piebald のみ。既定 3 |
| `startupTimeout` | 12 | piebald の重量級サーバーのみ |
| `shutdownTimeout` | 3 | 同上 |

必須フィールドは `command` と `extensionToLanguage` の 2 つだけ
（`claude-code-lsps-boostvolt/CLAUDE.md:26-30`）。フィールドの定義一覧は
`claude-code-lsps-piebald/CLAUDE.md:71-79`。boostvolt は最小形:

```json
{
  "rust": {
    "command": "rust-analyzer",
    "extensionToLanguage": { ".rs": "rust" }
  }
}
```

（`claude-code-lsps-boostvolt/rust-analyzer/.lsp.json:1-8`）

### `initializationOptions` の使い方

大半は空 `{}`。実質的に使っているのは 4 件のみ:

- jdtls: Maven のソース/Javadoc ダウンロード指定 + `startupTimeout: 300000`（5 分）
  （`claude-code-lsps-piebald/jdtls/.lsp.json:9-22`）
- metals: `isExitOnShutdown` / `statusBarProvider` + `startupTimeout: 90000`
  （`claude-code-lsps-piebald/metals/.lsp.json:11-17`）
- php-lsp: PHP バージョンと診断の有効化（`claude-code-lsps-piebald/php-lsp/.lsp.json:14-27`）
- vue-volar: `tsdk` パスと `hybridMode`（`claude-code-lsps-piebald/vue-volar/.lsp.json:12-19`）

### lsp-det を `command` に挟めるか: 挟める

- `command` は任意の実行ファイルでよい。piebald の vue-volar は `command: "sh"` +
  `args: ["-c", "<バージョン探索スクリプト>; exec ... --stdio"]` という**ラッパー実例**が
  既に存在する（`claude-code-lsps-piebald/vue-volar/.lsp.json:3-6`）。
  v0.1 設計の `"command": "lsp-det", "args": ["--adapter", "rust-analyzer", "--", "rust-analyzer"]`
  は形式上まったく問題ない。
- `initializationOptions` / `settings` はクライアント（CC）が initialize リクエストに
  乗せるだけなので、透過プロキシはそのまま転送すればよい。lsp-det 側の関与は不要。
- 注意点は 2 つ:
  1. `startupTimeout` は「サーバー起動の待ち時間上限」であって readiness ゲートではない
     （`claude-code-lsps-piebald/CLAUDE.md:77`）。ゲートで保留が長引くサーバー
     （jdtls 等）では lsp-det の `--gate-timeout` と CC 側 `startupTimeout` の関係を
     ドキュメント化する必要がある。
  2. `maxRestarts` により CC がプロキシごと再起動する。lsp-det はクラッシュ時に
     上流サーバーを道連れに終了する（クリーン exit する）方が再起動セマンティクスと整合する。
- なお CC 本体にも初期化競合のレース既知バグがあった
  （v2.0.69–v2.0.x、`claude-code-lsps-boostvolt/README.md:10`。v2.1.0 で修正）。
  クライアント側ですら init タイミングが壊れていた事実は、タイミング問題の根深さの傍証。

## 5. まとめ: lsp-det と組み合わせて誰が恩恵を受けるか

調査結果を一言で言えば、**エージェント向けブリッジのどれも「機能の欠落」への対策
（LSAI のフォールバック、mcpls の capability ゲート、cclsp の kind フォールバック）は
持っているが、「準備の未完了」への対策は固定スリープ以上のものを持っていない**。
lsp-det の readiness ゲートは、この全ブリッジに共通する空白を、各ブリッジを改造せずに
`command` の差し替えだけで埋められる位置にある。

| 利用者 | 現状の問題 | lsp-det 挟み込みの効果 |
| --- | --- | --- |
| Claude Code + LSP プラグイン利用者（両マーケットプレイス） | initialize 直後の references/definition が空になり得る。`startupTimeout` は起動待ちのみ | `.lsp.json` の `command` を lsp-det に替えるだけ。プラグイン・CC 本体は無改造 |
| cclsp 利用者 | 3 秒 + 200ms の固定スリープは大規模リポジトリで不足、小規模では無駄待ち | `command: ["lsp-det", "--adapter", ...]` に差し替え。固定スリープは残っても無害（ゲート通過後は即応答） |
| mcpls 利用者 | `$/progress` を捨てており準備完了前の空応答を検出不能 | TOML の `command`/`args` 差し替えで対応。capability ゲートと readiness ゲートが直交補完 |
| codex-lsp 利用者 | 編集直後の診断フックが「解析未完了の診断ゼロ」を「クリーン」と誤認し得る | `lsp-client.json` の `command` 差し替え。診断系をゲート対象に含めるかは要検討 |
| LSAP / LSAI 系オーケストレーション層 | 合成クエリ（impact 等）の途中結果が黙って過少になる。フォールバック分類に時間軸がない | 下に lsp-det を敷けば「未準備の空」が消え、合成結果の過少カウントを構造的に排除 |

設計検証としての結論:

1. **v0.1 の存在意義は裏付けられた。** readiness ゲートは既存ブリッジの未実装領域であり、
   cclsp の固定スリープ（3s/200ms）はまさに「各クライアントが個別に近似で対処している」
   vision.md の問題認識の実例になっている。
2. **`.lsp.json` への挟み込みは実例レベルで互換。** シェルラッパー `command` の前例
   （vue-volar）があり、形式上の障害はない。`startupTimeout` / `maxRestarts` との
   相互作用だけ設計ノートに明記すべき。
3. **範囲補正（拡張 A）を v0.1 から外した判断も整合的。** range 補正を実装している
   ブリッジは皆無で、需要の実証がまだない一方、readiness の欠落は全ブリッジで実証された。
