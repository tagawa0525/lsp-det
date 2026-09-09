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

## 一般化してはならない点

- 窓の長さ（約 1 秒）は 2 ファイルの fixture と本機での値。再ロードの所要時間に依存する
- 壊し方は go.mod の構文エラーのみ。go.work、`GOFLAGS`、vendor の不整合で `unloadableFiles` に入る経路は測っていない
- 依存の取得失敗は `GOPROXY=off` の 1 通り。ネットワーク断でのタイムアウトの見え方は測っていない
