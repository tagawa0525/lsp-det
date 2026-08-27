# Serena (solidlsp) の initialize ClientCapabilities と通知ハンドリング調査

調査対象: `reference/serena/` (oraios/serena, commit `7fcbca7e6255`, 2026-08-20)

## 要約

- solidlsp には共通の ClientCapabilities 基底実装は存在しない。`_create_base_initialize_params` は抽象メソッドで、各言語サーバークラスが capabilities を丸ごと自前定義する。共通ビルダーが足すのは `processId`/`rootPath`/`rootUri`/`clientInfo`/`workspaceFolders` と `initializationOptions` のマージのみで、capabilities には一切触れない。
- **rust-analyzer**: `window.workDoneProgress: true` と `experimental.serverStatusNotification: true` の両方を宣言済み。`experimental/serverStatus` ハンドラで `quiescent: true` を待って ready 判定している(タイムアウト 120 秒)。**lsp-det による capability 書き換えは不要**。
- **gopls**: capabilities は最小限で、`window` セクション自体が存在せず `workDoneProgress` は未宣言。`$/progress` ハンドラは登録されているが中身は no-op で、ready 待ちもしない。**lsp-det が gopls の `$/progress` を ready 信号に使うなら、initialize の `window.workDoneProgress: true` 注入が必要**。あわせて `window/workDoneProgress/create` リクエストは Serena 側に転送すると `MethodNotFound` エラー応答になるため、プロキシが自前で応答すべき。
- 未知の通知/リクエストで Serena はクラッシュしない。未登録の通知は警告ログを出して無視、未登録のサーバー発リクエストは `MethodNotFound` エラー応答を返すだけで、受信処理全体も例外を握って継続する。

## 1. initialize で送られる ClientCapabilities

### 1.1 基底実装(共通部分)

`SolidLanguageServer._create_base_initialize_params` は抽象メソッドであり、基底クラスは capabilities のデフォルトを持たない
(`reference/serena/src/solidlsp/ls.py:3218-3237`)。

最終的なパラメータは `_create_initialize_params` がビルダー経由で組み立てる
(`reference/serena/src/solidlsp/ls.py:3239-3245`)。`DefaultInitializeParamsBuilder._apply_updates` が設定するのは以下のみで、`capabilities` キーには関与しない(`reference/serena/src/solidlsp/initialize_params.py:51-82`)。

- `processId`, `rootPath`, `rootUri`(51-56 行)
- `clientInfo: {"name": "Serena"}`(57 行)
- `workspaceFolders`(59-69 行)
- ユーザー設定 `initializationOptions` のマージ(71-82 行)

つまり **capabilities は言語サーバークラスごとに完全に独立して定義される**。

### 1.2 rust-analyzer (`RustAnalyzer`)

`reference/serena/src/solidlsp/language_servers/rust_analyzer.py:218-685` の `_create_base_initialize_params` が、VSCode 相当のフル capabilities を返す。

(a) `window.workDoneProgress` — **宣言あり**(414 行):

```python
"window": {
    "showMessage": {"messageActionItem": {"additionalPropertiesSupport": True}},
    "showDocument": {"support": True},
    "workDoneProgress": True,
},
```

(b) `capabilities.experimental` — **`serverStatusNotification: true` を宣言済み**(467-484 行):

```python
"experimental": {
    "snippetTextEdit": True,
    "codeActionGroup": True,
    "hoverActions": True,
    "serverStatusNotification": True,
    "colorDiagnosticOutput": True,
    "openServerLogs": True,
    "localDocs": True,
    "commands": {...},
},
```

そのほか `workspace`(224-259 行)、`textDocument`(260-410 行)、`general.staleRequestSupport`(416-428 行)、`notebookDocument`(466 行)も宣言。`trace: "verbose"` 付き(683 行)。

### 1.3 gopls (`Gopls`)

`reference/serena/src/solidlsp/language_servers/gopls.py:112-148` の `_create_base_initialize_params` は最小限:

```python
initialize_params: dict = {
    "locale": "en",
    "capabilities": {
        "textDocument": {
            "synchronization": {"didSave": True, "dynamicRegistration": True},
            "definition": {"dynamicRegistration": True},
            "documentSymbol": {...},
        },
        "workspace": {"workspaceFolders": True, "didChangeConfiguration": {"dynamicRegistration": True}},
    },
}
```

- (a) `window` セクションが存在しないため **`window.workDoneProgress` は未宣言**。
- (b) **`experimental` も未宣言**。
- `initializationOptions` はユーザー設定 `gopls_settings` がある場合のみ付与(132-146 行)。

## 2. サーバー発通知/リクエストのハンドラ

ハンドラ登録は各クラスの `_start_server` 内で `self.server.on_notification` / `on_request` により行う。基底クラス共通では `on_any_notification` オブザーバが 1 つだけ登録され、`textDocument/publishDiagnostics` の保存にのみ使われる(`reference/serena/src/solidlsp/ls.py:581`, `655-663`)。

### 2.1 rust-analyzer のハンドラ(`rust_analyzer.py:749-756`)

| メソッド | 種別 | ハンドラ | 処理内容 |
| --- | --- | --- | --- |
| `client/registerCapability` | request | `register_capability_handler` | `workspace/executeCommand` 登録を検出してイベント set(722-727 行) |
| `workspace/executeClientCommand` | request | `execute_client_command_handler` | 空リストを返すだけ(736-737 行) |
| `language/status` | notification | `lang_status_handler` | `ServiceReady` でイベント set(729-734 行) |
| `window/logMessage` | notification | `window_log_message` | INFO ログ出力のみ(746-747 行) |
| `$/progress` | notification | `do_nothing` | **完全に無視**(パースしない、753 行) |
| `textDocument/publishDiagnostics` | notification | `do_nothing` | 無視(基底オブザーバが別途保存) |
| `language/actionableNotification` | notification | `do_nothing` | 無視 |
| `experimental/serverStatus` | notification | `check_experimental_status` | `params.get("quiescent") is True` で `server_ready` を set(742-744 行) |

ready 判定: `initialized` 送信後、`self.server_ready.wait(timeout=120.0)` で `experimental/serverStatus` の quiescent を待つ。タイムアウト時は警告を出して続行(777-785 行)。

**`window/workDoneProgress/create`(request)と `window/showMessage`(notification)のハンドラは未登録**。

### 2.2 gopls のハンドラ(`gopls.py:296-328`)

| メソッド | 種別 | ハンドラ | 処理内容 |
| --- | --- | --- | --- |
| `client/registerCapability` | request | `register_capability_handler` | 何もしない(`None` を返す = 成功応答) |
| `window/logMessage` | notification | `window_log_message` | INFO ログ出力のみ(302-303 行) |
| `$/progress` | notification | `do_nothing` | **完全に無視**(310 行) |
| `textDocument/publishDiagnostics` | notification | `do_nothing` | 無視 |

ready 判定は**行わない**。initialize 応答の capabilities を assert した後、`initialized` を送って即座に完了する(「gopls server is typically ready immediately after initialization」327-328 行)。`window/workDoneProgress/create` と `window/showMessage` のハンドラは未登録。

### 2.3 `window/workDoneProgress/create` について

rust-analyzer / gopls とも未登録。登録している言語サーバーの例は F# で、no-op で成功応答を返す(`reference/serena/src/solidlsp/language_servers/fsharp_language_server.py:393-405`)。C#、Erlang、Kotlin なども同様に登録している。

## 3. window/showMessage / window/logMessage

- `window/logMessage`: rust-analyzer(`rust_analyzer.py:751`)、gopls(`gopls.py:309`)ともログ出力するだけの ハンドラを登録。ready 判定には使っていない。
- `window/showMessage`: rust-analyzer / gopls とも**未登録** → 受信すると警告ログを出して無視(後述 4.1)。登録している言語もあるが、いずれもログ転記や no-op であり(例: `fsharp_language_server.py:366-376`, `kotlin_language_server.py:556`, `ada_language_server.py:201`)、ready 判定に使う実装は rust-analyzer / gopls には存在しない。`window/showMessageRequest`(request)は Scala(Metals)が応答内容を選択して返す実装を持つ(`scala_language_server.py:783`)。

## 4. 未知の通知/リクエストを受けたときの挙動(クラッシュ耐性)

ディスパッチは `LanguageServerInterface` 内(`reference/serena/src/solidlsp/ls_process.py`)。

### 4.1 未登録の通知 → 警告ログのみ、無視

`_notification_handler`(456-482 行):

```python
handler = self.on_notification_handlers.get(method)
if not handler:
    log.warning("Unhandled method '%s'", method)
    return
```

ハンドラ実行中の例外もログに落とすだけで raise しない(476-482 行)。

### 4.2 未登録のサーバー発リクエスト → `MethodNotFound` エラー応答

`_request_handler`(432-454 行):

```python
handler = self.on_request_handlers.get(method)
if not handler:
    self.send_error_response(
        request_id,
        LSPError(ErrorCodes.MethodNotFound, f"method '{method}' not handled on client."),
    )
    return
```

クラッシュはしないが、サーバーには JSON-RPC エラーが返る点に注意。

### 4.3 受信処理全体の例外耐性

`_handle_body` は JSON デコードエラー等を捕捉(277-288 行)、`_receive_payload` もハンドラ呼び出しを `try/except Exception` で包む(290-306 行)。したがって**どんな通知を透過しても Serena がクラッシュすることはない**。

## 5. 結論: lsp-det への示唆

| 項目 | rust-analyzer | gopls |
| --- | --- | --- |
| ready 信号 | `experimental/serverStatus`(quiescent) | `$/progress` |
| 必要 capability | `experimental.serverStatusNotification` | `window.workDoneProgress` |
| Serena の宣言 | **あり**(`rust_analyzer.py:471`) | **なし**(`gopls.py:116-130` に `window` なし) |
| capability 書き換え | **不要** | **必要**(`window.workDoneProgress: true` を注入) |

### 5.1 rust-analyzer

書き換え不要。Serena は `serverStatusNotification` と `window.workDoneProgress` の両方を宣言しており、rust-analyzer は `experimental/serverStatus` を送ってくる。プロキシはこれを盗み見るだけでよく、透過しても Serena 自身が quiescent 待ちに使うため、**この通知は必ず下流(Serena)へ透過しなければならない**(握りつぶすと Serena の起動が 120 秒タイムアウトまで待たされる。`rust_analyzer.py:780-785`)。

### 5.2 gopls

`window.workDoneProgress` が未宣言のため、素の状態では gopls はサーバー起点の work done progress(`window/workDoneProgress/create` + `$/progress`)を送らない。プロキシが `$/progress` を ready 信号に使うには initialize の capabilities への注入が必要。

注入した場合の注意点:

1. gopls は progress 開始前に `window/workDoneProgress/create`(server→client request)を送る。これを Serena に転送すると `MethodNotFound` エラー応答が返る(`ls_process.py:440-447`)。サーバーによっては create 失敗時にその token での `$/progress` 送信を取りやめるため、**プロキシが create リクエストを自前で終端し `null` 成功応答を返すのが安全**(Serena へは転送しない)。
2. `$/progress` 通知自体は Serena に透過しても無害。gopls クラスに no-op ハンドラが登録済みで(`gopls.py:310`)、仮に未登録でも警告ログが出るだけ(4.1 節)。

### 5.3 透過の一般則

- 通知はすべて透過してよい。Serena は未知の通知を警告ログ付きで無視し、クラッシュしない。
- ただし Serena が ready 判定・状態管理に使う通知(`experimental/serverStatus`、`textDocument/publishDiagnostics`、`client/registerCapability`)は透過が必須。
- server→client の「リクエスト」だけは注意が必要。Serena が未登録のメソッドはエラー応答になるため、プロキシが capability 注入によって新たに誘発したリクエスト(`window/workDoneProgress/create` 等)はプロキシ自身が応答すべき。
