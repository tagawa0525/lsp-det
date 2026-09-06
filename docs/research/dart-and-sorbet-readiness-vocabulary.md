# Dart analysis server と Sorbet の readiness の語彙

外部レビュー（[external-review-2026-09.md](external-review-2026-09.md) §2）が挙げた 2 つを一次資料で確かめ、仕様 10 章の対応表に「見込み」の行として足すための記録（ADR 0018 決定 A-3）。どちらも rust-analyzer の `experimental/serverStatus` の `quiescent` と同型で、「初回の解析が終わったか」を通知で伝える。保証（7.2 / 7.3）は測っていない。

## Dart analysis server（実測、2026-09-04、Dart SDK 3.13.0）

`dart language-server`（`serverInfo.name` は "Dart SDK LSP Analysis Server"）に、小さな fixture（2 ファイル）で `initialize` → `initialized` → `textDocument/references` を送り、通知を記録した。道具は scratchpad の `dart/probe.py`。

| クライアントの宣言             | 信号                                                                                                                                           |
| ------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `window.workDoneProgress` なし | `$/analyzerStatus` `{"isAnalyzing": true}` → `{"isAnalyzing": false}`。解析のたびに対で繰り返す（references を送るたびに true → false が来た） |
| `window.workDoneProgress` あり | `window/workDoneProgress/create`（token `ANALYZING`）→ `$/progress` begin（title "Analyzing…"）→ end。同じく解析のたびに繰り返す               |

- SDK の `pkg/analysis_server/tool/lsp_spec/README.md` は `$/analyzerStatus` を非推奨（Deprecated）とし、クライアントが `window.workDoneProgress` を宣言すると `$/progress` に置き換わると書いている。lsp-det は `window.workDoneProgress` を無条件に注入するので、写像は `$/progress` の経路を読むことになる
- `serverInfo` を返す。`capabilities.experimental` に `experimental/serverStatus` はない
- health の信号はない。写像は `unknown` にとどまる
- この fixture では `initialized` 直後の references（解析の終了前）でも完全な結果が返った。小規模なので 7.1 の前提（インデックスに観測可能な時間を要する規模）を満たしていない。保証を測るには大きな fixture が要る
- 4 サーバーと同じく stdin の EOF で終了する（[language-server-exit-on-stdin-eof.md](language-server-exit-on-stdin-eof.md) と同じ手順で確認済み）

写像（見込み）: begin（または `isAnalyzing: true`）→ `indexing`、end（または `false`）→ `ready`。rust-analyzer の写像と同じ形で、再インデックス（仕様 6 章 3 項）も同じ対で表れる。

## Sorbet（文書の確認、2026-09-06）

一次資料: `sorbet/sorbet` の `website/docs/server-status.md` と `website/docs/lsp.md`（`sorbet/showOperation` の節）。実サーバーでは測っていない。

- 通知 `sorbet/showOperation`。params は `{operationName: string, description: string, status: "start" | "end"}`。`operationName` は安定した識別子で、"Indexing"、"SlowPathBlocking"、"SlowPathNonBlocking"、"FastPath"、"References"、"SymbolSearch"、"Rename"、"MoveMethod"。`description` は人間向けで変わりうる
- クライアントが `initializationOptions.supportsOperationNotifications: true` を渡したときだけ送る。lsp-det が `window.workDoneProgress` を注入するのと同じ位置で注入できる
- 操作は重なる（"References" が "SlowPathNonBlocking" と重なる等）。`status` の start / end で追う
- 文書の表: "Idle" は待機中で、IDE の機能に応答し、エラー一覧も完全。"Indexing files..." と "Typechecking..."（SlowPathBlocking）は応答せず不完全。"Typechecking in background..."（SlowPathNonBlocking）は hover 等に応答するが一覧は不完全。**Find All References（と Rename）は Idle のときしか使えない**と明記されている
- nixpkgs にはない。rubygems の `sorbet-static`（x86_64-linux / aarch64-linux / universal-darwin の prebuilt。0.6.13485）を取り、NixOS で glibc 以外に依存せず起動することを確認した（`sorbet --lsp`。プロジェクトには `sorbet/config` が要る）

写像（見込み）: `Indexing` / `SlowPathBlocking` / `SlowPathNonBlocking` の start で `indexing`、未完了の操作がなくなった end で `ready`。`References` 等の要求に伴う操作は状態にしない（横断リクエストそのものの処理）。health の信号は文書にない。

## 含意

- 「初回の解析が終わったか」を通知で伝える語彙は rust-analyzer、Dart、Sorbet の 3 つに独立に存在し、いずれも本プロトコルの `readiness` に 1 対 1 で写る。上流提案の「既存の語彙の一般化」の根拠になる
- Sorbet は「横断リクエストは Idle でしか答えない」をサーバー側の文書として書いている唯一の例で、9 章の推奨挙動（`ready` を待つ）をサーバーが自ら求めている
- どちらも 0.5.0（ADR 0019 決定 A-2）で写像を書き、7.1〜7.3 を当てる
