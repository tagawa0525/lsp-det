# pyrefly の readiness の実測（M16）

ADR 0019 決定 F の M16。コーパス（[readiness-vocabulary-corpus.md](readiness-vocabulary-corpus.md)）は pyrefly を「起動時の走査が無音で再チェックだけ進捗」型に置き、「再チェックの進捗だけで `ready` を言えるか」を疑問にしていた。実測とソースで、**起動時の索引（populate）はプロトコルに何も出さない**（stderr の INFO だけ）。`$/progress` "Pyrefly: Rechecking" は開いているファイルの型検査で、索引を覆わない。索引の前後で `references` は空配列 → 部分 → 完全と変わり、区別する信号がない。設定の壊れも stderr だけで health の信号もない。写像は書けず、両軸 `unknown`（仕様 8.2 の 3）が正直。変更の取り込み（`didChange`、監視対象の Changed）は同期に近く速い。

## 方法

- nixpkgs の pyrefly 1.3.0-dev.1（`flake.nix` の `servers`）。`pyrefly lsp`。2026-09-06
- 被験体: `pyproject.toml`（`[project]` だけ）、`pkg/__init__.py`、`pkg/a.py`（`def target()`）、`pkg/b.py`（`target()` を 1 回呼ぶ）。大きい被験体は `target()` を 40 回呼ぶ `m001.py` … `m300.py` を足した 303 ファイルと、1 回呼ぶ `m0001.py` … `m3000.py` を足した 3003 ファイル（`--workspace-indexing-limit` の既定値 2000 を超える）
- 道具: scratchpad の `lsp_probe.py`。`workspace/configuration` には Serena と同じ `{"pythonPath": …, "pyrefly": {"diagnosticMode": "workspace"}}` を返す
- 走行: (1) 3 ファイル、(2) 303 ファイル、(3) 3003 ファイル、0.05 秒間隔、(4) 3003 ファイルを `--indexing-mode lazy-blocking` で、(5) `pyproject.toml` の `[tool.pyrefly]` を壊す、(6) 開いている `a.py` に `didChange` で使用を足す、(7) 開いていない `b.py` をディスクで変えて Changed
- 裏付けにソース（`pyrefly/lib/lsp/non_wasm/server.rs`: `populate_workspace_files_if_necessary`、`populate_all_workspaces_files`、`LspEvent::RecheckFinished`、`LspProgressSubscriber`）を読んだ

## 結果

### 語彙

| 信号                                                         | 内容                                                                                                                                                                               |
| ------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `$/progress`（token は `"pyrefly-progress-N"`、create の後） | title "Pyrefly: Rechecking"。開いているファイルの型検査（`didOpen`、`didChange`、監視対象の変更、設定の無効化）で begin → end（message "1/20" 等）。**索引（populate）では出ない** |
| `textDocument/publishDiagnostics`                            | 開いているファイルの診断。索引が終わるたびに（`RecheckFinished`）開いているファイルを再検証して出し直す（診断 0 件でも出る）                                                       |
| エラー応答 `-32800`（RequestCancelled）                      | 変更の処理中に届いた要求を "Request textDocument/references (N) is canceled due to subsequent mutation" で拒む。空応答ではなく、正直                                               |
| `client/registerCapability`                                  | `workspace/didChangeWatchedFiles` を id "FILEWATCHER" で動的登録する                                                                                                               |

`serverInfo` は `{"name": "pyrefly-lsp", "version": "1.3.0-dev.1"}`。`capabilities.experimental` はない。索引の開始と終了（"Populating all files in the config …"、"Populated all files in the project path …"、"Populating up to 2000 files in the workspace …"、"Populated all files in the workspace …"）は **stderr の INFO だけ**で、`window/logMessage` にはならない。

### 索引の前後で答えが変わる（走行 3、3003 ファイル、既定の lazy-non-blocking-background）

| 時刻（秒） | 出来事                                                                                                                      |
| ---------- | --------------------------------------------------------------------------------------------------------------------------- |
| 0.002      | `initialize` 応答、`didOpen pkg/a.py`                                                                                       |
| 0.023      | 最初の `references` は `-32800`（`didOpen` の処理による取り消し）。"Rechecking" begin                                       |
| 0.070      | "Rechecking" end、`a.py` の診断。**`references` が空配列**（索引前。stderr: "Populating all files in the config" が 0.069） |
| 0.160      | stderr "Populated all files in the project path"。`a.py` の診断（`RecheckFinished` による再検証）                           |
| 0.198      | stderr "Populating up to 2000 files in the workspace" → "Populated all files in the workspace"                              |
| 0.226      | `references` が 6002 件（3001 ファイル × 2）。以後変わらない。0.252 に `a.py` の診断（2 つ目の `RecheckFinished`）          |

`lazy-blocking`（走行 4）では索引がメインスレッドを塞ぐので "Rechecking" の begin が索引の後（0.129）になり、その間（0.154）の `references` は **3996 件（上限 2000 の workspace の索引だけ。config の索引はまだ）** で、0.329 に 6002 件。塞いでも部分応答は消えない。

303 ファイル（走行 2）と 3 ファイル（走行 1）では、取り消しの次の答えが完全だった（索引が 0.2 秒に収まる）。窓の長さは被験体の大きさで決まり、信号はない。

### 索引の完了を示す信号はない

各 populate の完了は `RecheckFinished` → 開いているファイルの再検証 → `publishDiagnostics` として見えるが、`didOpen` 直後の型検査の診断と区別できない。populate の数は開いたファイルの config の数と workspace root の数で変わり（`skip_lsp_config_indexing`、`--indexing-mode none`、上限 0 なら 0 回）、観測者が数えても根拠がない。`$/progress` の機構（`LspProgressSubscriber`）は再検査にしか繋がれていない。

### 変更は速い（走行 6、7）

| 引き金（`ready` 後）                                 | 結果                                                                                               |
| ---------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| 開いている `a.py` に使用を足す `didChange`（走行 6） | 0.1 秒後の `references` が 3 件（2 → 3）。`publishDiagnostics` が 1 ms 後                          |
| 開いていない `b.py` に使用を足して Changed（走行 7） | 直後に "Rechecking" begin → end（1 ms）、0.2 秒後の `references` が 3 件。監視対象の変更を取り込む |

### 設定が壊れても信号はない（走行 5）

`[tool.pyrefly]` に型の違う値を書くと、stderr に "TOML parse error" を 2 回出して既定値で動く（`references` は 2 件）。`window/showMessage` も `window/logMessage` も出ない（ソースに送信箇所がない）。health の信号はない。

## 写像（設計）

- **写像を書かない**。readiness も health も観測する語彙がなく、両軸 `unknown`（仕様 8.2 の 3、8.4 の 1）。lsp-det は `serverInfo.name` "pyrefly-lsp" に写像がないので、そのまま両軸 `unknown` を宣言する。`initializing` に留めることは「何も答えられない」という嘘（8.2 の 3）で、"Rechecking" の end で `ready` にすることは索引前の空応答を通す
- `--indexing-mode` と `--workspace-indexing-limit` は起動フラグで、観測者が注入できるものではなく、注入しても部分応答は消えない（走行 4）
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: populate を "Rechecking" と同じ `$/progress` の機構に繋ぐ（`populate_all_workspaces_files` と `populate_all_project_files_in_config` の前後で begin / end）。それだけで観測者は `indexing` → `ready` を写せる。設定の壊れを `window/showMessage` に出す

## コーパスへの反映

「起動時の走査が無音で再チェックだけ進捗」はそのとおりで、疑問「再チェックの進捗だけで `ready` を言えるか」の答えは否。索引の前の空応答と、`lazy-blocking` でも残る部分応答を、再チェックの end は区別しない。
