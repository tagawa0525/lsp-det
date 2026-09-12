# gopls の health の実測（2026-09-09、提出前の準備 3）

対外戦略（[../upstream-submissions.md](../upstream-submissions.md)）の準備 3「gopls の提出を health に縮める」の材料。ワークスペースの読み込みが失敗したとき gopls がクライアントに何を伝え、その間の要求に何を返すか、go.mod の変更後に窓があるかを実 gopls で測った。readiness は出さない: `references` 等は初回ロードを待つ（`awaitLoaded`）し、golang/go#76137（closed）で adonovan が「ready を知るには、その状態でしか答えられない質問（`package` 節への `definition`）を投げてブロックさせよ」と答えている。

## 結論

| 項目                                                             | 実測                                                                                                                                                                                                                                                                                                                                                                              |
| ---------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 失敗中の横断要求                                                 | `textDocument/references` と `definition` は **エラー**（code 0、"no package metadata for file …"）。空応答の成功ではない。`workspace/symbol` だけは `null` を成功として返す                                                                                                                                                                                                      |
| 失敗の信号                                                       | `$/progress` の begin（title "Error loading workspace"、message にエラー本文）が開いたままになり、直ると end "Done."。開いているファイルには診断 "initialization failed: …"、go.mod には構文エラーの診断                                                                                                                                                                          |
| `window.workDoneProgress` を宣言しないクライアント               | `progress.Tracker.Start` の fallback で `window/showMessage` **type 4（Log）** に落ち、title は落ちて message だけ。"Done." は type 3（Info）。"Loading packages..." も type 4、"Finished loading packages." は type 3 で、失敗と正常が同じ severity の列に並ぶ。Claude Code は `window` を一切宣言しない（[claude-code-dogfooding.ja.md](claude-code-dogfooding.ja.md) 第 4 回） |
| 一度も失敗していないセッションの go.mod 変更                     | 窓なし。変更の直後（1 ms）の `references` も正しく答える。`MetadataForFile` が要求の中で `s.load(ctx, NoNetwork, fileLoadScope(uri))` を同期に走らせるため                                                                                                                                                                                                                        |
| 失敗から回復したセッションの go.mod 変更                         | **約 1.0 秒の窓**。変更のたびに `references` / `definition` が "no package metadata for file" で失敗し、約 1 秒後に戻る。3 回連続、2 走行で再現                                                                                                                                                                                                                                   |
| 依存の取得失敗（`GOPROXY=off` で存在しないモジュールを require） | critical error ではない。"Error loading workspace" は出ず、`could not import …` の診断だけ。`references` / `definition` / `workspace/symbol` は正しく答える                                                                                                                                                                                                                       |
| `gopls mcp` の `find_references`                                 | `golang.References` を直接呼ぶ（`internal/mcp/references.go`）。LSP の経路と同じスナップショットの API を通るので、上の挙動はそのまま当てはまる                                                                                                                                                                                                                                   |

上流の該当箇所は v0.23.0 と HEAD（`9427efe18`、2026-09）で同じ（`server/diagnostics.go` の差分は無関係の 1 行、`progress/progress.go` と `cache/snapshot.go` は差分なし）。

## 回復後の窓の理由（ソースの読み）

`cache/snapshot.go` の `MetadataForFile` は、ファイルの package が見つからず（`len(pkgs) == 0`）かつそのファイルが `unloadableFiles` に**入っていない**ときだけ、要求の中で同期にロードする。ワークスペースが壊れている間に要求を受けたファイルは、ロードしても package が見つからないので `unloadableFiles.Add(uri)` される。この集合から URI が外れるのは、その `.go` ファイル自身にメタデータに影響する変更があったとき（`clone` の "typing in a file doesn't necessarily make it loadable"）だけで、go.mod を直しても外れない。回復後はバックグラウンドの再ロードでメタデータが戻るので要求は通るが、次に go.mod が変わって `reinit` でメタデータが無効になると、`unloadable` のままなので同期ロードが飛ばされ、バックグラウンドの再ロード（約 1 秒後）まで "no package metadata" になる。

直し方の候補: `clone` の `reinit` の経路（go.mod / go.work / go.sum のディスク上の変更）で `unloadableFiles` を空にする。ワークスペース単位の変更はまさに「ファイルが再びロード可能になりうる」変更である。

## 測定環境

- gopls v0.23.0（nixpkgs。最新 release も v0.23.0）、go1.26.7 linux/amd64
- 経路: 偽クライアント（`scripts/gopls/health-probe.py`）→ `gopls serve` 直結。lsp-det は挟んでいない
- fixture: `go.mod`（`module fixture` / `go 1.21`）、`a.go` の `func Target() {}`、`b.go` の `func Caller() { Target() }`。壊し方は go.mod の末尾に閉じない `require (`。`initialize` は `rootUri` と `workspaceFolders` を渡し、`window.workDoneProgress: true`（`nowdp` シナリオでは `window` なし）

## 時系列（抜粋）

`broken-start`（最初から壊れている。`workDoneProgress` あり）:

```text
[ 0.024s] <- $/progress begin "Setting up workspace" "Loading packages..."
[ 0.030s] <- $/progress end "Finished loading packages."
[ 0.031s] <- publishDiagnostics go.mod n=1 ["syntax error (unterminated block started at …/go.mod:5:1)"]
[ 1.037s] <- $/progress begin "Error loading workspace" "…/go.mod:6: syntax error (unterminated block …)"
[ 1.037s] <- publishDiagnostics a.go n=1 ["initialization failed: …"]
[ 6.025s] -> textDocument/references (a.go Target)
[ 6.025s] <- error {"code": 0, "message": "no package metadata for file file:///…/a.go"}
[ 6.026s] -> textDocument/definition
[ 6.026s] <- error {"code": 0, "message": "no package metadata for file file:///…/a.go"}
[ 6.026s] -> workspace/symbol "Target"
[ 6.026s] <- result null
```

`nowdp`（同じ fixture。`window` を宣言しない）:

```text
[ 0.030s] <- window/showMessage {"type": 4, "message": "Loading packages..."}
[ 0.036s] <- window/showMessage {"type": 3, "message": "Finished loading packages."}
[ 1.044s] <- window/showMessage {"type": 4, "message": "…/go.mod:6: syntax error (unterminated block started at …/go.mod:5:1)"}
```

`break-later`（正常 → 壊す → 直す）:

```text
[ 5.048s] -> references（go.mod を壊して didChangeWatchedFiles の直後）
[ 5.055s] <- error "no package metadata for file …/a.go"
[ 6.055s] <- $/progress begin "Error loading workspace" …
[10.056s] -> references / workspace/symbol → error / null
[10.056s] -> references（go.mod を直して didChangeWatchedFiles の直後）
[10.057s] <- error "no package metadata for file …/a.go"
[11.066s] <- $/progress end "Done."
[15.058s] -> references → 1 件（b.go の呼び出し）
```

`recover-window`（回復後に go.mod を変え、応答を待って 50 ms 置いて要求を繰り返す。2 走行とも同じ）:

```text
=== go.mod change (comment after recovery) → references repeated
   t+0.001s ERR: no package metadata for file …
   t+1.017s OK n=1
=== second comment → t+0.001s ERR … t+1.014s OK
=== third change, definition → t+0.001s ERR … t+1.013s OK
```

`reload-window`（一度も壊していない。同じ変更の後、同じ繰り返し）:

```text
=== go.mod change (comment) → t+0.016s OK n=1（以後ずっと OK）
=== go.mod change (require) → t+0.016s OK n=1（以後ずっと OK）
```

## 提出への含意

- readiness の提案は出さない。要求は初回ロードを待ち、失敗中は明示的なエラーで返る。健康なセッションの go.mod 変更にも窓はない
- health は「信号がない」のではなく、「severity が信号を運んでいない」。`workDoneProgress` を宣言しないクライアントへの fallback は、失敗を type 4（Log）で、正常終了を type 3（Info）で流す。golang/go#78273（`$/progress` 経由の冗長なエラー表示。adonovan が「abridged version displayed in the client UI」に同意）の上に、fallback の severity を title に応じて選ぶ（`WorkspaceLoadFailure` は Error）提案を載せる
- 回復後の go.mod 変更の窓は gopls の不具合として新規 issue。fixture、ソースの読み、直し方の候補を添える
- `workspace/symbol` が失敗中に `null` を返す点は、要求ごとの正直さの例外として記録にとどめる（別 issue にするかは上の 2 つの反応を見てから）

## 提出後: 上流の修正 CL の検証（2026-09-12）

golang/go#81400 に対し、Go チームの Hana Kim が [CL 830924](https://go.dev/cl/830924)（`clone` の `reinit` の経路で `unloadableFiles` を空にする。上の「直し方の候補」と同じ）を出した。レビューで Peter Weinberger が、別の [CL 830844](https://go.dev/cl/830844)（`workspace/didChangeWatchedFiles` を 50 ms デバウンスし、編集操作 `didOpen` / `didChange` / `didSave` / `didClose` が来たら保留分を同期に flush する）が入ると、CL 830924 の回帰テスト（通知の直後に `References`）は変更前の snapshot に対して空振りで通る、と指摘した。

4 つの `gopls` をビルドして同じ probe を当てた（go1.27.0 linux/amd64）: CL 830924 patch set 2（`a373bba32`）、その親（`249605012`、master 上）、および両方に CL 830844 patch set 2（`dacbdbd80`）を cherry-pick したもの。probe には要求のタイミングを変える選択肢を足した（`--after-go-mod-diagnostics`、`--did-change-before-request`、`--toggle-b-before-request`、`--diagnostics-delay`）。

### 結果（`recover-window`、回復後の go.mod 変更 3 回 × 2 走行。時刻は変更からの経過）

| 要求のタイミング                                   | 親                                      | 親 + デバウンス                                                                    | CL 830924             | CL 830924 + デバウンス |
| -------------------------------------------------- | --------------------------------------- | ---------------------------------------------------------------------------------- | --------------------- | ---------------------- |
| 通知の直後から 50 ms ごと                          | 0.001 s でエラー、1.015〜1.018 s で成功 | 0.001 s は**成功（変更前の snapshot）**、0.052 s でエラー、1.065〜1.072 s で成功   | 0.009〜0.018 s で成功 | 0.001 s で成功         |
| go.mod の `publishDiagnostics` を待ってから        | 1.013 s（診断の到着）で成功             | 1.063 s で成功                                                                     | 1.011〜1.014 s で成功 | 1.062〜1.064 s で成功  |
| 通知の後に a.go の `didChange`（内容不変）を送って | 0.010 s で成功（**窓が出ない**）        | 0.001 s でエラー、1.014〜1.017 s で成功                                            | 0.009〜0.011 s で成功 | 0.008〜0.016 s で成功  |
| 通知の後に b.go の `didOpen` / `didClose` を送って | 0.010 s で成功（**窓が出ない**）        | 0.001 s でエラー。成功は `didOpen` 後 0.052〜0.059 s、`didClose` 後 1.016〜1.018 s | 0.009〜0.011 s で成功 | 0.009〜0.018 s で成功  |

読み取り:

- 修正はデバウンスの有無に関わらず効く。修正のない 2 つは、flush の後（デバウンスなしは通知の直後、ありは 50 ms 後）に窓が始まる
- 窓は「再ロードの所要時間」ではない。`--diagnostics-delay` を 300 ms にすると窓は 0.356 s、2 s にすると 2.03 s（親、デバウンスなし。親 + デバウンスの 300 ms は 0.052〜0.407 s）。fixture の再ロード自体は遅延を差し引いて 15〜56 ms
- go.mod の `publishDiagnostics` は再ロードの後に来るので、それを待ってから要求するテストは（修正の有無に関わらず）空振りで通る
- 空振りしないテストの形: 通知の後、変更後の go.mod の診断が届くまで要求を繰り返し、その間に一度でも "no package metadata" が返れば失敗。診断が窓の後に届くことは 4 つのビルドすべてで確認した

### 窓の機構と、編集操作が master で窓を消す理由（trace で確認）

親のビルドに `MetadataForFile` の判定、`clone` の変更ごとの `invalidateMetadata`、`load` と `reloadWorkspace` の呼び元を書き出す trace を仕込み（`GOPLS_TRACE_FILE`。lsp-det には入れていない）、`recover-window` を通知直後の要求と `--did-change-before-request` の 2 通りで走らせた。

- go.mod のディスク上の変更は `clone` で `reinit` になり、新しい snapshot は `initialized = false` になる。a.go の metadata はなくなる（`MetadataForFile` で `pkgs=0`）
- `references` の経路は `MetadataForFile`（`golang.NarrowestPackageForFile`）を `awaitLoaded` より先に呼ぶ。a.go が `unloadableFiles` に入っていると要求内のロードを飛ばし、`pkgs=0` のまま "no package metadata" になる。健全なセッションでは `unloadable=false` なので要求内で `s.load(fileLoadScope)` が走り 16 ms で答える
- 窓を閉じるのは `reloadWorkspace`（`shouldLoad` は空で `scopes=0`）ではなく、`AwaitInitialized` → `Snapshot.initialize` によるワークスペース全体のロード（trace の `load #6`、`initialize:704` から）。それを最初に呼ぶのは通常、`DiagnosticsDelay` の後に走る診断パス（`server.diagnose` → `WorkspaceMetadata` → `awaitLoaded`）。窓の長さが `diagnosticsDelay` に追従するのはこのため
- 編集操作（`didChange` / `didOpen` / `didClose`）が来ると `Session.DidModifyFiles` → `invalidateViewLocked`（`view.go`）が clone の前に `prevSnapshot.AwaitInitialized(ctx)` を呼ぶ（"Do not clone a snapshot until its view has finished initializing"）。`reinit` 直後の snapshot は未初期化なので、ここで同期にロードされ（trace では `DidChange` のハンドラの中で `load #6`、約 10 ms）、直後の要求は答えられる。デバウンスなしの master で編集操作が窓を消すのはこれ
- デバウンス版で同じ編集操作が窓を消さないのは、flush された go.mod の変更と編集操作が**1 つの** `DidModifyFiles` にまとまるから。`AwaitInitialized` が待つのは変更前の（初期化済みの）snapshot で、`reinit` で未初期化になるのはその clone の結果。以後、診断パスまで誰も `AwaitInitialized` を呼ばない

テストへの含意: デバウンス後は「通知 → 編集操作 → 要求」が空振りせず（親 + デバウンスで 1 ms 後の要求がエラー、修正 + デバウンスで成功）、デバウンス前の今は「通知 → 要求」が空振りしない（Hana Kim のテストの形）。同じ手順を両方で使うことはできない。手順に依らない形は、通知の直後から変更後の診断が届くまで要求を繰り返す形。

### `break-later` と `reload-window`（CL 830924、デバウンスなし）

壊れている間の要求は従来どおり明示的なエラー（偽の成功にならない）。修復直後の要求は 16 ms で成功（親は再ロードまでエラー）。健全なセッションの go.mod 変更は窓なし（変わらず）。

CL 830844 を重ねたときの freshness（通知から取り込みまでの窓）は次の節。

### デバウンス CL 830844 は通知の直後の要求に古い答えを返す（2026-09-12）

上の検証で「親 + デバウンス」の通知 1 ms 後の要求が変更前の snapshot から答えていたので、答えが変わる fixture で測った（probe の `stale-after-watched-change`。b.go は開いていない）。

| 手順                                                                                                          | 親（デバウンスなし）             | 親 + CL 830844                                                     |
| ------------------------------------------------------------------------------------------------------------- | -------------------------------- | ------------------------------------------------------------------ |
| b.go の `Target()` の呼び出しをディスク上で消し、`didChangeWatchedFiles` の直後から 10 ms ごとに `references` | 1 ms 後の要求から 0 件（正しい） | 1 ms 後から **1 件（消した呼び出し）**、52 ms 後に 0 件            |
| 呼び出しを戻し、`didChangeWatchedFiles` を 30 ms 間隔で 20 回送りながら 10 ms ごとに `references`             | 1 ms 後の要求から 1 件（正しい） | **0 件のまま 507〜511 ms**（`debounceMax` の 500 ms）、その後 1 件 |

2 走行とも同じ。今の gopls は通知を受けた順に処理するので、`didChangeWatchedFiles` の後の要求は必ずその変更を織り込む。CL 830844 はこの順序を要求に対しては守らない（編集通知 `didOpen` / `didChange` / `didSave` / `didClose` が来たときだけ保留分を同期に flush する）。変更を受け取っておきながら最長 50 ms、連続する変更では 500 ms、古い状態で答え、その間クライアントに信号はない（flush の時点で出るものはなく、`publishDiagnostics` は再ロードの後）。仕様 6 章 2 項の freshness（受け取った `didChangeWatchedFiles` を以後の要求が織り込む）が成り立たない。

lsp-det への影響: 準拠テスト `gopls_spec_7_3_2_watched_file_changes_through_lsp_det_with_real_gopls` と `stand_in_spec_7_3_2` は親で通り、親 + CL 830844 で "did not return the call added on disk while declaring ready (freshness violation)" で落ちる（`cargo build --release --examples` の後に `cargo test --release --test conformance gopls -- --ignored --test-threads=1`。テスト名で絞った実行は偽上流の example をビルドしないため先に作る。`gopls_spec_7_1` と `spec_7_2_2` は開発版が `TESTED_VERSIONS` にないための宣言不在で両方落ち、この件とは無関係）。lsp-det は ADR 0014 でクライアントの代わりに `didChangeWatchedFiles` を送ってから要求を転送するので、この CL を含む版には `freshness` の `fileChanges` を宣言できない。CL が入った版が出たら 7.3.2 を当て直し、`TESTED_VERSIONS` を動かさない。

CL 自身がこの問題を一箇所だけ避けている: `gopls mcp` の `fileOf` は `DidChangeWatchedFiles` の呼び出しを `session.DidModifyFiles` の直接呼び出しに置き換え、debounce を迂回する（コミットメッセージ: "Update MCP's fileOf to modify the session snapshot directly so snapshot queries are not delayed by the debounce timer"）。変更の直後の問い合わせを遅らせてはならないことは認めたうえで、内蔵クライアントだけを救った形で、LSP のクライアントに同じ経路はない。

直し方の候補（gopls 側）: 要求の処理の入口でも保留分を drain する（編集通知と同じ扱い）。burst の途中に要求が来なければ今のまとめ方が保たれ、来たときだけ 1 回 flush するので、#81408 の目的（重複する `go list`）は損なわない。または、保留があることと適用された時点をクライアントに示す信号を出す。

## 一般化してはならない点

- 窓の長さ（約 1 秒）は 2 ファイルの fixture と本機での値。再ロードの所要時間に依存する
- 壊し方は go.mod の構文エラーのみ。go.work、`GOFLAGS`、vendor の不整合で `unloadableFiles` に入る経路は測っていない
- 依存の取得失敗は `GOPROXY=off` の 1 通り。ネットワーク断でのタイムアウトの見え方は測っていない
