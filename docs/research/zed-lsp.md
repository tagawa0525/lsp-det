# Zed の LSP 統合 調査報告

調査対象: `reference/zed/` (zed-industries/zed の浅い clone、2026-08 時点)。
以下のパスはすべて `reference/zed/` からの相対パスで表記する。

## 要約

- **experimental/serverStatus**: Zed は initialize の experimental capability で
  `serverStatusNotification: true` を宣言し、rust-analyzer からの通知を購読する。ただし
  デシリアライズするのは `health` と `message` のみで、**`quiescent` フィールドは読んでいない**
  (`quiescent` という語はコードベースに一切出現しない)。用途はログ出力とステータスバー
  (activity indicator / LSP ボタン) の表示のみで、**リクエスト抑制には使っていない**。
- **$/progress**: サーバー共通の汎用処理。`window/workDoneProgress/create` で登録された
  トークンのみ受理し、`pending_work` マップに保持して activity indicator にスピナー表示する。
  進捗はリクエスト送信のゲートには使わず、副作用は (1) rust-analyzer の
  `rust-analyzer/flycheck` トークンを disk-based diagnostics の開始/終了イベントに変換、
  (2) work 終了時の inlay hints 再取得、の 2 点。
- **LspAdapter / LspInstaller**: 「ユーザーインストール検出 (`which` 等) → メモリキャッシュ →
  ディスクキャッシュ → ダウンロード (失敗時は旧バイナリへフォールバック)」の多段解決。
  ユーザーインストール品は worktree ごとに異なりうるため意図的にキャッシュしない。
- **クライアント実装**: `Content-Length` ヘッダによる標準フレーミング + 容量 128 の受信
  キューで背圧。リクエストタイムアウトはデフォルト 120 秒 (設定で変更可)。送信リクエストは
  Future の drop で `$/cancelRequest` を自動送信、受信側の `$/cancelRequest` はタスク drop で
  キャンセルする。
- **documentSymbol の range**: 言語別の補正は**存在しない**。補正はすべて言語非依存
  (逆転 range のスワップ、利用側での clip)。言語別処理はシンボルの表示ラベル生成
  (`label_for_symbol`) と diagnostics 側に限られる。

---

## 1. experimental/serverStatus (rust-analyzer)

### 購読の宣言

initialize リクエストの `capabilities.experimental` で宣言する
(`crates/lsp/src/lsp.rs:1049-1052`)。

```rust
experimental: Some(json!({
    "serverStatusNotification": true,
    "localDocs": true,
})),
```

### 通知ハンドラ

`crates/project/src/lsp_store/rust_analyzer_ext.rs` に rust-analyzer 専用拡張としてまとまって
いる。通知型の定義は同ファイル 19-29 行:

```rust
#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ServerStatusParams {
    pub health: ServerHealth,
    pub message: Option<String>,
}

impl lsp::notification::Notification for ServerStatus {
    type Params = ServerStatusParams;
    const METHOD: &'static str = "experimental/serverStatus";
}
```

注目点: rust-analyzer のプロトコル定義には `quiescent: bool` (アイドル状態か) が含まれるが、
**Zed の `ServerStatusParams` には `health` / `message` しかなく、`quiescent` は捨てている**。
`grep -r quiescent crates/` は 0 件。つまり Zed は「サーバーがアイドルになったか」を
serverStatus からは一切利用していない。

ハンドラ登録は `register_notifications` (`rust_analyzer_ext.rs:31-82`)。処理内容は:

1. `health` に応じて `log::info!` / `warn!` / `error!` に振り分け (44-60 行)
2. `LspStoreEvent::LanguageServerUpdate { message: Variant::StatusUpdate(..) }` を emit
   (63-78 行)

登録箇所はサーバー起動時の共通セットアップ `crates/project/src/lsp_store.rs:1313`
(`rust_analyzer_ext::register_notifications(...)`)。rust-analyzer 以外のサーバーにも
ハンドラ自体は付くが、この通知を送るのは実質 rust-analyzer のみ。

### UI への反映

| 消費側 | 箇所 | 内容 |
| --- | --- | --- |
| activity indicator | `crates/activity_indicator/src/activity_indicator.rs:182-192` | proto の `StatusUpdate` を `LanguageServerStatusUpdate::Health` に復元 |
| activity indicator | 同 `:503-539` | サーバーごとの health メッセージを収集し Error > Warning > Ok でソート |
| activity indicator | 同 `:630-654` | `(server_name) Error: ...` 形式でステータスバーに表示 (長文は切り詰め) |
| LSP ボタン | `crates/language_tools/src/lsp_button.rs:366-368` | `Ok → "Running"(緑) / Warning(黄) / Error(赤)` のインジケータ |

### リクエスト抑制への利用

**なし**。health はイベント→UI 表示のみで、`language_server_statuses` の health を見て
リクエストを止める・遅延するコードは存在しない。リクエスト可否は後述の
`LanguageServerState::Starting / Running` だけで決まる。

---

## 2. $/progress の処理 (gopls 等)

### トークン登録と受理

- サーバーからの `window/workDoneProgress/create` に応答し、トークンを
  `language_server_statuses[id].progress_tokens` に登録する
  (`crates/project/src/lsp_store.rs:1000-1017`)。コメントに「**gopls は初期化時にこの
  リクエストの応答を待つのでレスポンスを返す**」と明記されている (1000-1002 行)。
- `$/progress` 受信時、**登録されていないトークンは黙って無視**
  (`lsp_store.rs:10925-10927`)。

### WorkDone progress 本体

入口は `on_lsp_progress` (`lsp_store.rs:10865-10908`)。`WorkDone` と
`WorkspaceDiagnostic` (workspace diagnostics ストリーミング) に分岐し、前者は
`handle_work_done_progress` (`lsp_store.rs:10910-10978`) で処理:

| イベント | 処理 |
| --- | --- |
| Begin | `pending_work` に `LanguageServerProgress { title, message, percentage, is_cancellable, .. }` を挿入し `WorkStart` を emit (`lsp_store.rs:10938-10956`, `10980-11004`) |
| Report | 100ms スロットリング (`SERVER_PROGRESS_THROTTLE_TIMEOUT`, `lsp_store.rs:176`) 付きで更新 (`lsp_store.rs:11006-11057`) |
| End | トークンを破棄し `WorkEnd` を emit。**disk-based でない work の終了時は inlay hints を再取得** (`lsp_store.rs:10970-10976`, `11059-11072` の `refresh_inlay_hints_on_work_end`) |

`pending_work` は `LanguageServerStatus` 構造体のフィールド
(`lsp_store.rs:4496-4498`) で、`BTreeMap<ProgressToken, LanguageServerProgress>`。

### 「準備中」のユーザーへの見せ方

activity indicator が `pending_work` を直接表示する
(`crates/activity_indicator/src/activity_indicator.rs:339-359` で全サーバーの pending_work を
更新時刻順に列挙、`:421-450` で先頭を「`title (percentage%): message + N more`」形式の
スピナー付きメッセージとして描画)。gopls の "Setting up workspace"、rust-analyzer の
"Indexing" などはこの経路で見える。

### リクエスト制御への反映

- **progress ではリクエストをゲートしない**。gopls がまだ indexing 中でも、サーバーが
  `Running` になっていればリクエストは送られる (準備完了待ちはサーバー側の応答遅延に委ねる)。
- リクエスト可否を決めるのは `LanguageServerState` (`lsp_store.rs:14486-14499`):

```rust
pub enum LanguageServerState {
    Starting {
        startup: Task<Option<Arc<LanguageServer>>>,
        pending_workspace_folders: Arc<Mutex<BTreeSet<Uri>>>,
    },
    Running { adapter: .., server: Arc<LanguageServer>, .. },
}
```

  `running_language_server_for_id` は Starting のサーバーを返さない
  (`lsp_store.rs:355-367`) ため、起動完了までリクエスト対象にならない。
  workspace folder の追加は Starting 中はバッファされ、Running 遷移後に反映される
  (`lsp_store.rs:14501-14514`)。シャットダウン時は起動完了を最大 5 秒だけ待つ
  (`SERVER_LAUNCHING_BEFORE_SHUTDOWN_TIMEOUT`, `lsp_store.rs:175`, `11726-11745`)。

- 逆方向の連携として、Zed は**自分が発行する時間のかかる LSP リクエストを擬似 progress として
  `pending_work` に注入**する (`lsp_store.rs:5679-5717`: リクエスト開始時に
  `on_lsp_work_start(ProgressToken::Number(request_id), ..)`、完了時に defer で
  `on_lsp_work_end`)。つまり progress 機構は「サーバー→UI」だけでなく
  「クライアント自身の待ち状態→UI」にも使われる。

### rust-analyzer 固有: disk-based diagnostics

アダプタが宣言するトークン接頭辞 (`crates/languages/src/rust.rs:315-317` の
`"rust-analyzer/flycheck"`) と一致する progress は cargo check の実行と見なし、
Begin/End を `disk_based_diagnostics_started/finished` イベントに変換する
(`lsp_store.rs:10929-10936`, `10940-10942`, `10973-10975`)。diagnostics UI の
「Checking...」表示や diagnostics の一括反映タイミング制御に使われる。

---

## 3. LspAdapter / LspInstaller (インストール機構の要点)

定義は `crates/language/src/language.rs`。

### トレイト構成

| トレイト | 箇所 | 役割 |
| --- | --- | --- |
| `LspAdapter` | `language.rs:510-694` | サーバー名、diagnostics 後処理、補完/シンボルのラベル生成、`initialization_options` / `workspace_configuration`、disk-based diagnostics トークン、`language_ids` など言語サーバー固有の振る舞い |
| `LspInstaller` | `language.rs:696-735` | バイナリの検出・取得。`type BinaryVersion` を持つジェネリックトレイト |
| `DynLspInstaller` | `language.rs:737-755` | `LspInstaller` を dyn 化するブランケット実装 (`language.rs:757-905`)。解決フローの本体 |
| `LspAdapterDelegate` | `language.rs:489-507` | アダプタに渡される環境アクセス面 (`which`、`shell_env`、HTTP クライアント、`update_status`、ダウンロード先ディレクトリ等) |

`LspInstaller` の主要メソッド:

```rust
pub trait LspInstaller {
    type BinaryVersion;
    fn check_if_user_installed(..) -> impl Future<Output = Option<LanguageServerBinary>>; // 既定は None
    fn fetch_latest_server_version(..) -> impl Future<Output = Result<Self::BinaryVersion>>;
    fn check_if_version_installed(..) -> impl Future<Output = Option<LanguageServerBinary>>;
    fn fetch_server_binary(..) -> impl Future<Output = Result<LanguageServerBinary>>;
    fn cached_server_binary(..) -> impl Future<Output = Option<LanguageServerBinary>>;
}
```

### 検出→取得→起動の解決順序

`get_language_server_command` (`language.rs:799-905`) が唯一の入口で、順序は:

1. **ユーザーインストール検出**: `binary_options.allow_path_lookup` が真なら
   `check_if_user_installed` を呼ぶ。見つかれば**キャッシュせず**即返す。
   理由コメント (809-819 行): worktree ごとに PATH 上のバイナリが異なりうるため
   (worktree 1 は `.bin/gopls`、worktree 2 は `~/bin/gopls`、worktree 3 はフォールバック)。
2. **メモリキャッシュ**: 前回解決済みバイナリがあり pre-release フラグが一致すれば返す
   (834-838 行)。
3. **ダウンロード禁止チェック**: `allow_binary_download` が偽ならエラー (840-845 行)。
4. **ディスクキャッシュ + バックグラウンド更新**: `cached_server_binary` の結果を即座に
   返しつつ、`try_fetch_server_binary` (最新版確認→未取得ならダウンロード) を別 Future と
   して返す (855-901 行)。ダウンロード失敗時は既存バイナリにフォールバックし、それも
   なければ `BinaryStatus::Failed` を通知 (871-892 行)。

進捗は `delegate.update_status()` 経由で `BinaryStatus`
(`CheckingForUpdate` → `Downloading` → `None` / `Failed`; `language.rs:773-796`) として
activity indicator に流れる (`activity_indicator.rs:107`, `507-510`)。

`check_if_user_installed` の典型実装 (gopls, `crates/languages/src/go.rs:107-119`):

```rust
async fn check_if_user_installed(&self, delegate: .., _: Option<Toolchain>, _: &AsyncApp)
    -> Option<LanguageServerBinary> {
    let path = delegate.which(Self::SERVER_NAME.as_ref()).await?;
    Some(LanguageServerBinary { path, arguments: server_binary_arguments(), env: None })
}
```

`delegate.which` は worktree のシェル環境を考慮した PATH 検索。実装者は
`crates/languages/src/*.rs` に多数ある (rust.rs:748, python.rs:472 ほか)。

**lsp-det への示唆**: 「ユーザーインストールを最優先し、それはキャッシュしない」
「ダウンロード品は versioned なパス (`gopls_{version}_go_{go_version}` 等、
`go.rs:138-153`) に置き、古いものは `remove_matching` で掃除」「取得失敗時は既存
バイナリに劣化運転」の 3 点が構造の核。

---

## 4. LSP クライアント実装 (crates/lsp)

### フレーミング

- **受信**: `crates/lsp/src/input_handler.rs`。`read_headers` が `\r\n\r\n` 終端まで
  ヘッダを読み (35-50 行)、`Content-Length:` 行をパースして `read_exact` で本文を読む
  (90-99 行)。`Content-Type` ヘッダの共存も許容 (テスト 197-211 行)。パース後、
  通知/リクエストは `serde_json::from_slice::<NotificationOrRequest>`、レスポンスは
  `AnyResponse` として response handler にディスパッチ (108-128 行)。
- **背圧**: 受信メッセージは容量 128 の bounded channel に積む
  (`INCOMING_MESSAGE_QUEUE_CAPACITY`, `input_handler.rs:22-27`)。フォアグラウンドが
  詰まったら読み取りを止め、OS のパイプバッファでサーバー側に背圧をかける設計。
- **送信**: `handle_outgoing_messages` (`lsp.rs:742-776`) が
  `Content-Length: {len}\r\n\r\n{body}` を書いて flush。

### リクエストタイムアウト

- 既定 120 秒: `crates/lsp/src/lsp.rs:49-55`
  (`DEFAULT_LSP_REQUEST_TIMEOUT_SECS: u64 = 120`)。
- ユーザー設定 `global_lsp_settings.request_timeout` で変更可能
  (`crates/project/src/project_settings.rs:132-163`)。実際のリクエスト発行時に
  `ProjectSettings::get_global(cx).global_lsp_settings.get_request_timeout()` を渡す
  (`crates/project/src/lsp_store.rs:5674-5680`)。
- タイムアウト時は response handler をテーブルから除去し
  `ConnectionResult::Timeout` を返す (`lsp.rs:1544-1556`)。戻り値は
  `Result / ConnectionReset / Timeout` の 3 値 (`lsp.rs:1530-1557`) で、接続リセットと
  タイムアウトを区別できる。

### キャンセル

- **送信リクエスト**: リクエスト Future の drop 時に defer で `$/cancelRequest` を自動送信
  (`lsp.rs:1517-1527`)。正常応答時は defer を abort (`lsp.rs:1534`)。つまり呼び出し側が
  Future を捨てるだけでプロトコル上のキャンセルが飛ぶ。
- **受信リクエスト** (サーバー→クライアント): `$/cancelRequest` を受けたら
  `pending_respond_tasks` から該当 ID のタスクを remove = drop してキャンセル
  (`lsp.rs:674-685`; 設計コメントは `lsp.rs:133`)。
- initialize では `general.cancel` 系として `server_cancel_support: Some(true)` を宣言
  (`lsp.rs:972`)。

---

## 5. documentSymbol の range に対する言語別補正

**結論: 言語別の range 補正は存在しない。** 補正はすべて言語非依存の防御的処理。

- documentSymbol の変換 (`crates/project/src/lsp_command.rs:2019-2061`) は Flat/Nested
  どちらも `range_from_lsp` を通すだけで、言語分岐はない。
- `range_from_lsp` (`crates/language/src/language.rs:1617-1631`) は逆転した range
  (start > end) をスワップするのみ。返り値は `Range<Unclipped<PointUtf16>>` で、
  **clip は変換時ではなく利用側で行う** (例: 編集適用時
  `lsp_store.rs:3445-3446` の `snapshot.clip_point_utf16(..)`)。
- 言語別に見えるコードはシンボルの**表示ラベル**生成であり、バッファ上の range ではない:
  `label_for_symbol` が `(text, filter_range, display_range)` を組み立てる
  (`crates/languages/src/c.rs:302`, `go.rs:396`, `python.rs:222`)。これは
  「`fn name` のようにキーワードを付けた整形文字列内のハイライト範囲」の話。
- range への実質的な補正が入るのは diagnostics 側のみ (これも言語非依存):
  disk-based diagnostics の range を保存後の編集 (`edits_since_save`) で座標変換し、
  空 range を 1 コードポイント分広げる (`lsp_store.rs:2854-2884`)。

**lsp-det への示唆**: Zed は「サーバーが返す range は信用しない (逆転・範囲外がありうる)
が、補正は言語共通の swap + clip で足りる」という立場。言語別の range ハックは
持っていない。
