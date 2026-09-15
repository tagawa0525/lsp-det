# Serena への提出

[../upstream-submissions.md](../upstream-submissions.md) の戦略に沿って Serena に出した文面と、提出後の反応・再測定の記録。出した文面は一字も変えずに残す。現在地（何を出し、何が返ってきたか）は同文書の一覧を見る。

## PR（クラッシュ検知を 2 回目以降の横断要求にも効かせる）

fork の `tsserver-crash-on-request-path`。CONTRIBUTING の「small bug fixes」は issue なしで PR にできる。PR template の checklist 2 つ（scope、CHANGELOG）は満たす。

Title: `fix(typescript): surface a tsserver crash on every cross-file query, not only the first`

````markdown
## Problem

#1848 wires tsserver's `[tsserver] Exited …` log message into `_crash_message` and raises `TypeScriptServerCrashedError` from the indexing waits. But the only place on the find-references path that reaches those waits is `_wait_for_cross_file_references_if_needed`, and it returns before any wait once `_has_waited_for_cross_file_references` has been set by the first query. A crash observed after the first query is logged ("tsserver reported an abnormal exit") and never raised: tsserver's teardown sends a `$/progress` "end", nothing waits, and the query returns an empty result that cannot be told apart from a symbol with no references. `safe_delete_symbol` deletes on exactly that result.

## Reproduction (solidlsp directly, no MCP)

1. `request_references` on a two-file TypeScript fixture (`a.ts` exports `target`, `b.ts` imports and calls it) → 2 locations.
2. `kill -KILL` the two tsserver processes.
3. `request_references` on the same position → 0 locations, no exception. The log has only the WARNING from step 2.

With this change step 3 raises `TypeScriptServerCrashedError: tsserver exited abnormally: [lspserver] [tsclient] [tsserver] Exited. Code: null. Signal: SIGKILL`. Reproduced on `701e7c84` and, with the same one-line change, on top of #1978, which touches the same method (the check goes before the new active-token check).

## Change

Call `_raise_if_crashed()` at the top of `_wait_for_cross_file_references_if_needed`, before the latch. The first query's behaviour is unchanged. One test for the latched case; CHANGELOG entry.

## Note

`TypeScriptServerCrashedError` is not a `LanguageServerTerminatedException`, so this does not trigger the automatic restart; later queries keep failing with the reason until the server is restarted. typescript-language-server/typescript-language-server#1125 makes the language server exit together with tsserver, which would let the crash reach the restart path. The two changes complement each other.

## Checklist

- [x] This PR follows the guidelines in `CONTRIBUTING.md` regarding the scope of PRs.
- [x] For changes that add features or fix problems, I have added an entry to `CHANGELOG.md`, which concisely describes the change.
````

## issue 4 件

HEAD `701e7c84` の行番号。1 件ずつ別の issue にする（Serena の issue template は checklist だけで、本文の形は自由）。設定の欄は共通: Serena 1.7.1.dev0 at `701e7c84`, read from the source and reproduced with solidlsp directly where stated; Linux; Python (pyright) and TypeScript.

(b-1) Title: `A request timeout is a bare TimeoutError and bypasses the language-server restart path`

````markdown
`Request.get_result` turns `queue.Empty` into a plain `TimeoutError` (`src/solidlsp/ls_process.py:101-107`). Everything that recovers from a broken language server keys on `SolidLSPException.is_language_server_terminated()` (`src/serena/tools/tools_base.py:382-397`), so a timed-out request never reaches the restart-and-retry branch, even when the server is hung rather than slow. The agent sees "Tool execution timed out after N seconds." (`tools_base.py:430-433`) with no indication whether the server is still healthy, and the next tool call goes to the same server.

`$/cancelRequest` is defined (`lsp_protocol_handler/lsp_requests.py`) but never sent, so the timed-out request also keeps running in the server and its late response is dropped as an unknown id.

Suggestion: raise a `SolidLSPException` subclass for the timeout (carrying the language), send `$/cancelRequest`, and let the tool layer decide whether a timeout counts as "terminated" (e.g. after a health probe such as a cheap `documentSymbol`).
````

(b-2) Title: `A failed write to the language server's stdin is logged and swallowed; the request then waits for the full timeout`

````markdown
`StdioLanguageServer` catches `BrokenPipeError`, `ConnectionResetError` and `OSError` around `stdin.writelines` / `flush`, logs "Failed to write to stdin" and returns (`src/solidlsp/ls_process.py:660-667`). The `Request` that was just registered stays in the pending table, so the caller blocks in `get_result` until the request timeout (up to 235 s with Serena's defaults: `tool_timeout` 240 s minus 5, `src/serena/project.py`) unless the stdout reader thread happens to see the process exit first and fails the pending requests itself.

The comment says "don't raise to prevent cascading failures", but the write failing means the server is gone; the honest outcome is `LanguageServerTerminatedException` for that request, which is exactly what the restart path needs.

Suggestion: on a write failure, fail the request with `LanguageServerTerminatedException` (and mark the process as not running) instead of returning silently.
````

(b-3) Title: `Full-document didChange on re-open reuses the document's version number`

````markdown
When a file is re-opened while it is already open in the language server and its mtime changed on disk, `LSPFileBuffer._open_in_ls` sends a full-text `textDocument/didChange` with `version: self.version`, unchanged (`src/solidlsp/ls.py:141-158`). The range-based `didChange` sent for edits increments the version (`ls.py:1378, 1420`).

LSP's `VersionedTextDocumentIdentifier` says the version "will increase after each change". The three servers I checked (pyright, typescript-language-server, rust-analyzer) all apply the new text regardless of the number, so the analysis itself is right. What breaks is everything keyed on the version:

- `publishDiagnostics` carries the document version the diagnostics were computed for (pyright: `languageServerBase.ts` `sendDiagnostics`; rust-analyzer: `main_loop.rs`, taken from `mem_docs`). After the reused version, diagnostics for the old text and for the new text carry the same number, so a client cannot tell which text they describe, and a client that drops diagnostics for an outdated version keeps the old ones.
- rust-analyzer rejects `codeAction/resolve` whose `data.version` differs from the current version ("stale code action", `handlers/request.rs`) and ignores outdated inlay-hint resolve data. With the number reused, resolve data computed from the old text passes those checks.

Serena's own diagnostics wait does not read the version (it counts publishes), so today this is a conformance problem rather than a visible bug in Serena; it becomes visible for any client that does read it.

Suggestion: `self.version += 1` before building the notification, as the edit path does.
````

(b-4) Title: `find_symbol with include_info silently drops info when symbol_info_budget is exceeded`

````markdown
`request_info_for_symbols` stops issuing hover requests once `symbol_info_budget` (default 10 s, `serena_config.py:944`; the docstring in `symbol.py` still says 5 s) is spent and sets `info = None` for the remaining symbols (`src/serena/symbol.py:687-694`). The only trace is a single `log.debug` and the perf summary, also at debug level (`symbol.py:715-721`). The tool result is the same as for a symbol that genuinely has no docstring or signature, so the agent cannot tell a budget cut from an absence.

Suggestion: report the cut in the tool result (a count of symbols whose info was skipped, or a note on each affected symbol), and log it at INFO or WARNING.
````

## registry に lsp-det を載せる提案（oraios/serena#1988 へのコメント）

oraios/serena#1988 がマージされてから、その形に合わせて出す（マージ前なら PR へのコメント）。Serena に自前の readiness 判定を捨てさせない（却下した案）。

````markdown
Thanks for this registry. I would like to use it for a different kind of external implementation and want to check that the shape fits before writing it.

lsp-det (https://github.com/tagawa0525/lsp-det) is a transparent proxy that sits between a client and a language server and adds one thing: a machine-readable server state (`experimental/serverState` request and `experimental/serverStateChanged` notification, with `health`, `readiness`, and what the server's index covers). It holds cross-file requests such as `references` until the server has finished indexing, and turns a crashed server into an explicit error instead of an empty result. Serena already works through it with only `ls_specific_settings.<language>.ls_base_cmd` (measured with pyright and typescript-language-server, including the tsserver-crash case where the direct path returns 0 references and the proxied path raises).

What I would register through this registry is one adapter per language that derives from the built-in one (`PyrightServer`, `TypeScriptLanguageServer`, …), prepends `lsp-det --` to the launch command, declares `experimental.serverState` in the client capabilities, and replaces the adapter's own readiness wait with a read of the server state. Everything else (dependency handling, initialize params, file matching) stays inherited.

Three questions:

1. Is deriving from a built-in adapter and registering the subclass under a new key (e.g. `python-lsp-det`) the intended use, or would you rather see `register(…, allow_override=True)` on the existing key so that `project.yml` does not change?
2. The readiness wait lives inside each adapter's `_start_server` (`PyrightServer` waits for "Found N source files", `TypeScriptLanguageServer` for `$/progress`). Would a hook such as `_wait_for_initial_readiness()` on `SolidLanguageServer`, called from `start_server`, be acceptable so that a subclass can override only that part? Without it the subclass has to copy `_start_server`.
3. Should such an adapter live in this repository (under `src/solidlsp/language_servers/`) or as a separate package that uses the entry point? I am happy to maintain it either way.
````

## 提出後の反応（2026-09-12）

PR oraios/serena#2007 と issue oraios/serena#2003〜#2006 に、メンテナからの返信はない。#2005（`version`）と #2006（hover の予算）には第三者が修正 PR を出している（2026-09-10。[oraios/serena#2009](https://github.com/oraios/serena/pull/2009) は再 open の全文 `didChange` の前に `self.version += 1`、[oraios/serena#2010](https://github.com/oraios/serena/pull/2010) は打ち切った symbol の `info` に予算切れの注記を入れて INFO でログする。どちらも issue で挙げた修正の形どおりで、レビューはまだ付いていない。マージされたら [research/serena-integration-measurement.md](../research/serena-integration-measurement.md) の (b-3) と (b-4) の所在を消す。(b-1) の `TimeoutError` と (b-2) の書き込み失敗には修正 PR がなく残る）。#2003、#2004、#2007 には人からの反応がない（#2007 に付いているのは提出直後の Copilot の自動レビュー "Approval recommended" だけ）。oraios/serena#1988 へのコメントにも返信はないが、作者 opcode81 が 2026-09-12 に PR を force-push した（最終的な head は `faed2fd3` "Refactor external language server registration"。同日 09:28 UTC の `78485f45` を 10:58 UTC に amend したもの。本文は「OO 設計に寄せ、Protocol `LanguageServerIdLike` で共通の interface を置き、登録の関心事を `LanguageServerRegistry` に集約」で、こちらへの言及はない）。その差分を 3 つの質問に当てると次のとおり。

| 質問                                                                                              | 状態                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 1. 組み込み adapter の子クラスを新しいキーで登録するか、`allow_override` で既存キーを上書きするか | 両方できる形になった。旧版は `register_ls()` が「実装クラス 1 つにつき ID は 1 つ」「組み込み ID の上書き禁止」で、`SolidLanguageServer.get_language_server_id()` がクラスから enum を逆引き（`LanguageServerId.from_ls_class`）していたため、子クラスでは `ValueError` になった。新版は逆引きを消して `ls_id = config.ls_id`（registry のキーがそのまま来る）とし、`LanguageServerRegistry.get_instance().register(ls_id, allow_override=False)` に上書きの引数が付いた |
| 2. `_wait_for_initial_readiness()` の hook                                                        | 未対応。`ls.py` の変更は `ls_id` の取り方だけで、`start_server` の周りは動いていない                                                                                                                                                                                                                                                                                                                                                                                     |
| 3. 本体に同梱するか別パッケージか                                                                 | 文書（`docs/03-special-guides/external_language_server_registration.md`）は別パッケージが entry point を公開する形で書かれている。group 名は `serena.language_servers` から `solidlsp.language_server_registration` に変わった。本体への同梱の可否には触れていない                                                                                                                                                                                                       |

質問 1 が解けたのが refactor の副産物かこちらのコメントを読んでのものかは、返信がないので分からない。

関連して、vitalyruhl の [oraios/serena#2016](https://github.com/oraios/serena/pull/2016)（2026-09-11）が「registry の導入後、`ls_specific_settings` の string キーが失われ、設定した `ls_path` が起動前に消える」と報告し、opcode81 は 2026-09-12 に「`LanguageServerId` はもう `StrEnum` ではないので、キーは `get_key()` で引くのが正しい」と答えて force-push に取り込んだ。`faed2fd3` の `SolidLSPSettings.ls_specific_settings` は `dict[str | LanguageServerId, dict[str, Any]]` と注釈され、`get_ls_specific_settings()` は string キー（`ls_id.get_key()`）を引き、組み込みの `LanguageServerId` なら enum キーも引く（両方にあれば `ValueError`）という意図だが、`LanguageServerId` を `TYPE_CHECKING` の下でしか import しておらず `from __future__ import annotations` もないので、dataclass の field 注釈の評価で `solidlsp.settings` の import 自体が `NameError` になる。#1988 の CI（run 34689842034）は全 job がこの `NameError` で落ちている。#2016 は draft のまま OPEN。したがって「lsp-det の Serena 統合が依存する `ls_specific_settings.<language>.ls_base_cmd` の経路が #1988 のマージ後も string キーで残る」は作者の意図として読めるだけで、現 head では確認できない。マージされた版で確かめる。

2026-09-12 時点の次: 返信はしない。同日 21:16 UTC（JST では 09-13）に返信が来て #1988 もマージされたので改めた（「提出後の反応（2026-09-15）」）。

## 提出後の反応（2026-09-15）

### oraios/serena#1988: opcode81 の返信（2026-09-12 21:16 UTC）とマージ

[返信](https://github.com/oraios/serena/pull/1988#issuecomment-5648743907)は 21:16:36 UTC で、その 16 秒後の 21:16:52 に #1988 は main にマージされた（マージコミット `403ad0a5`。私は 09-14 までこれを見落として OPEN と報告していた）。返信の要点。(1)「`references` を索引の完了まで保留する」はどう実現しているのか、サーバーごとに大きく違うのでは、という問い返し。(2) 質問 1（子クラスの新キーか `allow_override` か）は「実装を置き換えたいかどうかで、どちらも可」。(3) 質問 2 の hook は「多くのラッパーがイベントハンドラをローカルなクロージャで書いていて、readiness の信号と他のハンドラの登録を切り分けられないので現実的でない」。(4) 質問 3 は「本物の問題を解くなら本体に入れる価値はあるが、中間にプロキシプロセスを挟むのは避けたい間接化。SolidLSP の中に直接実装する解を望む。issue を立てて、何をしていてどう SolidLSP に足せるのか詳しく書いてほしい」。

ユーザーの決定（2026-09-15 JST。以下この節の日付は上流の出来事が UTC、こちらの決定が JST）:

- 招待に乗って issue を立てる。内容は設計の提案ではなく、事実（実測）と要件（仕様 9 章の規則と 9.1 のテスト可能な形）を出し、**境界の定義はそちらに頼む**（core が持つもの: サーバーごとの状態、保留の規則、要求経路の検査。adapter が供給するもの: 状態の遷移。書き方はクロージャのままでよい）。SolidLSP に取り込まれる方が望ましく、Serena の経路では lsp-det は消える運命（構想の「準拠すれば補正が不要になる」）
- 提案の大きさは (i) health の要求経路への反映と横断要求が見る readiness の 2 点を本文に、(ii) `coverage` の宣言と `didChangeWatchedFiles` の鮮度は「その先」として 1 段落
- 外部 adapter パッケージ（`python-lsp-det` 等。組み込みの adapter を継承して `lsp-det --` を前置し、`experimental.serverState` を宣言して待ちを状態の読み取りに置き換えるもの）は棚上げ。`ls_base_cmd` の設定だけの経路と観測できる結果は同じで、違いは保留を lsp-det が代行するか Serena 側の Python がやるかだけ。相手が要らないと言った形で、hook が断られた以上は言語ごとに `_start_server` を写して追従する保守費が値打ちに見合わない。再検討の条件は、相手が定義した境界が「adapter は状態を供給する」形になり、写像のないサーバーの供給元として lsp-det 経由の adapter が求められたとき
- 返信には LSP 本体への提案の準備中であることも書く。ただし「ドラフトで、Serena や LSP 側の反応を見て形を決める」と明記する。rust-analyzer のメンテナが `ready: bool` を選んだこと（[rust-analyzer.md](rust-analyzer.md) の「提出後の反応（2026-09-14）」）は、adapter のサーバー固有の半分がサーバー自身の報告に置き換わっていく例として 1 つだけ挙げる

返信の根拠として上流 main を読み直して分かったこと（読んだのは #1988 のマージ直前の `813fd98f`。マージ後の HEAD `403ad0a5` で同じ箇所を確かめ、行番号はそちらのもの。返信（提出済み）と issue の予備の草案はこれに基づく。どちらも下の「#1988 への返信（提出済み 2026-09-14 UTC）と issue（予備）」の節）:

- 要求経路の継ぎ目は既にある。`SymbolLocationRequest.execute()`（`ls.py:1452-1460`）が definition / implementation / references の毎回の要求で `_pre_open_for_cross_file_references()` → `open_file` → `_wait_for_cross_file_references_if_needed()` → 送信、の順に呼ぶ。hook の依頼は不要
- そこを流れているのは状態ではなく一度きりの latch。`_has_waited_for_cross_file_references`（`ls.py:582` で False。以後リセットなし）。既定の実装は `sleep(2)` を一度（`ls.py:1624-1628`。docstring は「信頼できる initializing 完了の信号がない LS 向け」）。typescript / Metals / Vue は progress を待つ実装に上書きしているが、どれも timeout で "proceeding anyway"（typescript のコメントは "historical permissive behavior"。"strict companion servers" は失敗させる、とある）
- #2007 の穴はこの latch そのもの。`TypeScriptServerCrashedError` は `is_language_server_terminated()` が偽なので再起動の経路には乗らない。`map_exception` は -32603 を言い換えるだけで失敗を握りつぶしてはいない。`workspace/symbol`（`ls.py:3122`）と rename（`ls.py:3149`）はこの経路の外
- 「サーバー固有では」への答え: 半分はそのとおりで、lsp-det は 17 の写像で 18 のサーバー（pyright と basedpyright は 1 つ）。写像が読む pyright の "Found N source files" と typescript-language-server の `$/progress` は、Serena の adapter が待っているものと同じ信号。共通の半分（状態と、要求経路がそれを見る規則）だけがサーバーに依らない

### oraios/serena#2004: opcode81 の問い返し、返信、クローズ（2026-09-14）

問い返し（2026-09-14）: 「実際にどう遭遇したのか。観測したのか。サーバーが本当にいないなら読み取りスレッドが検知して要求をキャンセルする」。

(b) は読んで見つけたもので測っていなかった（[research/serena-integration-measurement.md](../research/serena-integration-measurement.md) の「一般化してはならない点」に明記済み）。相手の主張は保留中の要求については正しく、元の文面はその場合まで「打ち切りまで待つ」と読めた。残る場面（保留が空のときに死に、同じツール呼び出しの中で次の要求を送る）を測ったところ事実だった（同報告の「(b-2) の実測」。probe は `scripts/serena/dead-write-probe.py`）。ユーザーの承認のうえで 2026-09-15（JST。09-14 15:24 UTC）に返信した（[コメント](https://github.com/oraios/serena/issues/2004#issuecomment-5666371144)）。返信に書いた "pyright 1.1.412" は誤りで、probe が `ls_base_cmd` を設定しておらず Serena の既定の provider が起動した `uvx --from pyright==1.1.403` だった（PR #107 のレビューの指摘。機構は solidlsp 側で結論は変わらない。probe は PATH の 1.1.412 を指定する形に直し、結果は同じ）。問い返しの前日（09-13 01:42 UTC）に第三者 feiiiiii5 が修正 PR [oraios/serena#2030](https://github.com/oraios/serena/pull/2030)（`_send_payload` の書き込み失敗で `_cancel_pending_requests(LanguageServerTerminatedException(...))`。偽の stdin のテスト 3 件）を出していて、返信の時点で見落としていた。probe をその枝に当てるとコード 0（要求 #2 は 0.00 秒で `LanguageServerTerminatedException`。同報告の「#2030 の枝での結果」）。返信の 30 分後（15:54 UTC）に opcode81 は "Not an actual issue. Closing." で閉じた。窓は事実だが影響は小さいという判断で、反論はしない。#2030 の帰趨も相手に任せる。読んだだけの主張は「読んだ」と書くか、測ってから出す。

出した文面:

````markdown
No — I found it by reading `ls_process.py` while measuring something else (the tsserver crash in #2007), not by hitting it in practice. Your point is right for the case I had in mind when I wrote "waits for the full timeout": if the request is already pending when the process dies, the stdout reader thread fails it through `_cancel_pending_requests`. I should have narrowed the claim.

So I measured the remaining case today (solidlsp directly, pyright 1.1.412, request timeout set to 8 s): send `request_references` once (2 locations), SIGKILL the pyright process while nothing is pending, wait 1 s, send `request_references` again.

```text
reader thread: "Language server stdout reader thread has terminated" → "Cancelling 0 pending language server requests"
1 s later:     ls.is_running() == False
request #2:    "Failed to write to stdin: [Errno 32] Broken pipe" (twice: the didOpen and the request)
8.00 s later:  TimeoutError: Request timed out (timeout=8.0)   — a bare TimeoutError, not SolidLSPException
```

`_cancel_pending_requests` runs once, when the reader thread exits; a request registered after that point has nobody to fail it. The write to the dead stdin is where that request could still be failed cheaply, and `is_running()` already says the truth at that moment. `_ensure_functional_ls` covers a death between two tool calls, but not one inside a tool call that sends more than one request (`find_symbol` with `include_info`, or any `open_file` followed by a request).

If you would rather treat this as part of #2003 (the bare `TimeoutError` not reaching the restart path), I am fine closing this one; the fix is the same either way — fail the request with `LanguageServerTerminatedException` when the write fails or when `is_running()` is already false, instead of letting it wait.
````

### oraios/serena#2003: opcode81 の反論（2026-09-14 15:51 UTC）

「打ち切った応答で LS の再起動の経路に乗せるのは筋が通らない。打ち切りは待つと決めた時間を超えただけで、短い打ち切りや大きなコードベースの重い要求では正常でありうる。LS に再起動を要する問題があることを意味しない。実際にどんな問題に遭遇したのか、なぜ再起動が適切だと思うのか」。

相手の読みは題名（"bypasses the language-server restart path"）から来ていて、その読みでは相手が正しい。出した文面の提案は「`SolidLSPException` の子クラスにして**ツール層が**打ち切りを terminated と見なすかを決められるようにする（health probe の後で等）+ `$/cancelRequest` を送る」で、無条件の再起動ではないが、題名がそう読めるのはこちらの落ち度。#2004 と同じく、実際に踏んだのではなくソースを読んで書いたもの。ユーザーの指示で測ってから返信し（[research/serena-integration-measurement.md](../research/serena-integration-measurement.md) の「(b-1) の実測」。応答が打ち切りより遅れる状況で、素の `TimeoutError`、`$/cancelRequest` なし、遅れた応答は放棄した `Request` を pop して静かに消える）、再起動の部分を取り下げて not planned で閉じた（2026-09-14、[コメント](https://github.com/oraios/serena/issues/2003#issuecomment-5667011948)）。出した文面の "the timed-out request keeps running in the server" は、cancel を送らないことからの推論で、実測（pyright は 4 ms で答え終わっている）の結論ではない。PR #107 のレビューで指摘され、報告書には測れたこと（cancel なし、`Request` の残留）と推論を分けて書き、ユーザーの指示で投稿済みのコメントも訂正した（2026-09-14 16:26 UTC。当該の一文を測ったことに直し、末尾に *Edit* で訂正の旨を添えた。下の文面は訂正後。文面の "a stdio proxy … that holds `textDocument/references` requests" は、その時点で走らせた旧版のプロキシ（要求を渡す前に止める）の記述で、記録として残す。現在の `delay-proxy.py` は応答を止める）。閉じた理由: 本筋（Serena を状態を読む消費者にする）に寄与しない粗に相手の注意を使わせない。

出した文面:

````markdown
No, I did not encounter it in practice; like #2004, this came from reading the code, and the title overstates it. You are right that a timeout says nothing about the server's health, and I withdraw the restart part.

I measured what actually happens (solidlsp directly, pyright 1.1.412, request timeout 5 s, a stdio proxy in front of pyright that holds `textDocument/references` requests for 8 s and passes everything else through):

```text
7.47 s   request_references raises TimeoutError("Request timed out (timeout=5.0)")  — 2 s of that is the default sleep in _wait_for_cross_file_references_if_needed
         is_running() == True; no $/cancelRequest reaches the proxy; the Request stays in _pending_requests
10.47 s  the proxy releases the request, pyright answers at once; the late response pops the abandoned Request and completes it into a queue nobody reads (no log line — so "dropped as an unknown id" in my report was wrong about the mechanism)
15.47 s  a second references request times out the same way; documentSymbol right after it succeeds in 0.00 s — the server is fine throughout
```

So the two things that remain are small: no `$/cancelRequest` is sent after the timeout, and the abandoned `Request` stays in `_pending_requests` until the server eventually answers. Neither is what the title claims. Closing this; the two measured points are recorded here in case they become relevant.

*Edit: the earlier wording of the paragraph above said the timed-out request "keeps running in the server". That was an inference from the missing `$/cancelRequest`, not something this measurement shows — pyright had answered in 4 ms and it was the proxy holding the response. Corrected to what was measured.*
````

### 09-14 の 3 件から読めること

opcode81 は 15:07〜15:54 UTC の間に #2004 → #2003 の順で読み、どちらも「実際に踏んだのか」を最初に問うた。読んで書いた issue は、実測を添えても "not an actual issue" になりうる。そもそも Serena に期待した役割は、下流の被験者（ADR 0010 M7）と、状態を読む消費者（本文書「位置づけ」）であり、(b) の 4 件は研究報告の副産物で本筋ではなかった。残す価値があったのは LSP 準拠と誠実さに関わる #2005 / #2006 で、どちらも第三者が修正 PR を出している。#1988 の issue（下）は要件の一覧ではなく、**実測と実害**（tsls の `references` が `[]` を成功として返す事例、ドッグフーディング第 6 回でエージェントが使われている関数を消した事例）を中心に据える。

2026-09-14 時点の次: #1988 への返信と issue の草案を書いて確認に出す。翌日、issue は予備にして返信で答えることに改め、出す前に試作と再現で確かめ（[research/serena-request-path-state-prototype.md](../research/serena-request-path-state-prototype.md)）、そのうえで返信した（次の節）。#2003 と #2004 は閉じた。#2004 の修正は #2030 に任せる。次: 返信への反応を待つ。issue を求められたら予備の草案を出す。催促はしない。

## CLA の導入と署名（2026-09-14）

oraios/serena は 2026-09-14 にライセンス体系を変えた（コミット "Introduce component-based licensing: Serena GPL-3.0-or-later, SolidLSP MIT"。Serena 本体は GPL-3.0-or-later、`src/solidlsp` は MIT）と同時に CLA（`CLA.md`）を導入し、CLA assistant の bot が open な全 PR に「未署名」のコメントを付けた（#2007 には 19:47 UTC。#2030 にも同時刻）。`CONTRIBUTING.md` は「すべての貢献に CLA の受諾が必要。`license/cla` のチェックが通るまで PR はマージできない。一度受諾すれば以後の PR にも適用」。

CLA の要点（読んで確かめたもの）: 著作権は貢献者に残る（第 2 条。譲渡ではなくライセンス）。Oraios（と Oraios が配布するソフトウェアの受領者）に、永続・世界的・**非独占**・取消不能・無償・サブライセンス可の著作権ライセンスを与え、Oraios はそれを任意の条件（プロプライエタリ・商用を含む）で再ライセンスできる（第 3 条）。特許ライセンスも永続・世界的・非独占・無償で与えるが、取消不能には例外があり、貢献またはプロジェクトについて特許訴訟を起こした主体へのライセンスは提訴日に終了する（第 4 条）。貢献者を含む誰の公的ライセンス上の権利も制限しない（第 7 条）。準拠法はドイツ法（第 8 条）。

ユーザーの条件は「同じ変更を自分が他所で行うことを妨げないこと」で、著作権が貢献者に残ること（第 2 条）、付与するライセンスが非独占であること（第 3 条・第 4 条。ライセンス自体は Oraios が第三者に移転できる "transferable"）、貢献者自身の権利を制限しないこと（第 7 条）により満たす。ユーザーが 2026-09-14 に署名し、#2007 の `license/cla` は SUCCESS になった（23:39 UTC。bot の再確認 URL を叩いて反映）。#2007 は CI 全緑・CLA 済みで、メンテナのレビュー待ち。

## #1988 への返信（提出済み 2026-09-14 UTC）と issue（予備）

opcode81 の返信（「提出後の反応（2026-09-15）」）への対応。ユーザーの決定（2026-09-15 JST）: **答えは #1988 に書く**。相手の 2 つの問い（どう実現しているか、どう SolidLSP に足せるか）に、マージ済み PR のスレッドで自己完結して答える。issue は相手が「追跡のために欲しい」と言ったときに出す予備で、その草案は下に残す。理由: 相手が求めたのは "more details on what exactly it is your solution does and how it could be added" で、それは返信で答えられる。#2003 / #2004 の直後に同じ報告者が大きな issue を出すより、まず答えて相手に選ばせる。

読者は「実際に踏んだか」を最初に問うので、Serena 自身の issue（oraios/serena#1937、#1858、#1923、#1978）を根拠の筆頭に置く。事実の出典は [research/serena-integration-measurement.md](../research/serena-integration-measurement.md)（tsserver クラッシュ後の `[]`、latch の `sleep(2)`）と上流 `403ad0a5` の行番号。

### #1988 への返信

**提出済み（2026-09-14 20:25 UTC、JST では 09-15。[コメント](https://github.com/oraios/serena/pull/1988#issuecomment-5670324283)）。** 出す直前にリンク先の生存（fork の枝 `b7a7093d`、#23331 / #23362 / #1937 / #1978 / #2007 は open、#1858 は closed）と、上流 main（`18fa47bf`）に `_wait_for_cross_file_references_if_needed()` の呼び出しが残っていることを確かめた。2026-09-15 JST に、この会話の文脈を持たないサブエージェントに事実の箇条書きだけを渡して起草させ（既存の草案も研究報告も読ませない。相手の語と普通の英語だけ、完全な文、比喩なし、進捗を盛らない、構成は自分で決める）、ユーザーの指摘で 3 点を直した版: (1) lsp-det の信号の説明は相手の adapter が同じものを読んでいるので「違いは読んだ結果の使い方だけ」の 3 文に縮める、(2) 18 サーバーの信号の一覧（corpus.md）の申し出は落とす、(3) 13 ファイルと 12 project の出どころ（共有パッケージ 1 + それを使うパッケージ 12 + app 1。13 = app のファイル + 各パッケージの入口 1 つ、12 project = 各パッケージの tsconfig）を明記する。事実は [research/serena-request-path-state-prototype.md](../research/serena-request-path-state-prototype.md) と一致することを確認済み（c8827191 の時点で tsls の adapter が `$/progress` を `do_nothing` で捨てていたことも履歴で確認）。

````markdown
Thank you for the answers, and for merging the PR.

**How lsp-det holds cross-file requests**

It reads the same indicators your adapters already read; for typescript-language-server, the `$/progress` tokens and the "[tsserver] Exited" log line. The difference is only in what happens with the result. lsp-det keeps it as a state — whether the server is ready, and whether it is functioning — and checks that state before every cross-file request; requests such as `references` wait while the server is not ready and fail immediately while it is broken, and single-file requests such as `hover` are never delayed.

**On the proxy**

I agree that an intermediate proxy process should be avoided, and I do not ask for it to be included in Serena. The proxy is a stopgap. What I want is for each language server to report, in a common form, whether indexing has finished and whether it is functioning. I have proposed this to rust-analyzer (discussion in rust-lang/rust-analyzer#23331, PR rust-lang/rust-analyzer#23362, which adds `ready: bool` to `experimental/serverStatus`; it is not merged). A proposal for the LSP itself is still a draft, and its form is not decided.

**How this could be done in SolidLSP without a new hook**

I withdraw the request for a `_wait_for_initial_readiness()` hook. SolidLSP already has a place on the request path that can do this. On `main` at 403ad0a5, the base class calls `_wait_for_cross_file_references_if_needed()` immediately before sending each definition, implementation and references request, and an adapter can override it. The default implementation checks `_has_waited_for_cross_file_references`, calls `sleep(2)` on the first call only, and sets the flag; the flag is never reset. The typescript-language-server adapter overrides it and, on the first call only, waits until its `_active_progress_tokens` set is empty; on later calls it returns immediately because of the flag. The flag and the `sleep(2)` came in with commit c8827191 (2025-09-18, "fix(swift-lsp): improve CI stability with enhanced delays and retry logic"); at that time the adapter did not read `$/progress`.

The change I propose is that this method consults the adapter's readiness state on every call, not only on the first one. The event handlers can stay as local closures; the method only needs to read the state they already maintain.

**What I measured**

I reproduced oraios/serena#1937 (TypeScript: `find_referencing_symbols` returns partial results from the second call on). The setup is a generated pnpm-style monorepo with one shared package, 12 packages that import it (each with its own tsconfig, so each is a separate tsserver project), and one app that imports all 12; the shared package has no tsconfig of its own, the root has no solution tsconfig, and a file of the app is kept open. The complete answer for a symbol in the shared package is 13 files: the app file and one file in each of the 12 packages. A `find_referencing_symbols` call issued within 100 ms of the previous one returns 1 of those 13 files, 30 times out of 30. The result is the same with typescript-language-server 5.3.0 + TypeScript 5.9.3 and with the reporter's 5.1.3 + TypeScript 6.0.3.

I then added 8 lines to the typescript-language-server adapter: in `_wait_for_cross_file_references_if_needed()`, call `wait_for_indexing()` whenever `_active_progress_tokens` is not empty, even after the flag is set. With this change, 35 of 36 calls return the complete 13 files. The branch is https://github.com/tagawa0525/serena/tree/request-path-consults-state. oraios/serena#1978 already proposes the same change, so I will not open a PR for it.

The cost is this. A call on which the wait is triggered takes about 0.4 s, which is the time tsserver needs to reload the 12 package projects; `main` answers the same call in 10 ms with the wrong result. Calls during which no reload is running take 10–20 ms, the same as `main`. The one miss in 36 has a known cause: tsserver's first `$/progress` notification arrives a few ms after `didOpen`, and the check sometimes runs before it.

My open PR oraios/serena#2007 adds a check at the top of the same method for a different case: it detects that tsserver has exited, so that a `references` request after the crash does not return `[]` as a success.

One limit: the request-path approach does not cover Metals (the "second call" version of oraios/serena#1858). A `references` request immediately after creating and opening a new file does not include the new reference; one second later it does. At that moment the server has not started working yet (its first `$/progress` arrives 135 ms after `didOpen`), so there is nothing in the adapter's state to consult. Closing that window requires either a timed wait or the server itself holding the request.

**What I can provide**

- The fixture generator and the probe used for the reproduction above.
- Tests, once it is decided what the base class holds and what each adapter supplies.
- A tracking issue, if you would like one.
````

### issue（予備。相手が追跡用に欲しいと言ったら出す）

題名: `Cross-file requests consult a one-shot latch, not the server's state: partial and empty results after the first query`

内容は上の返信と同じ材料を issue の形に組み直したもの（Summary → What happens today (measured) 1〜5 → What I am asking (a)(b)(c) と規則 5 つ → What I can bring → Later, not now → Context）。返信に入れなかった項目は次の 3 つで、出すときに足す: エージェントの実害の事例（Claude Code 経由。第 6 回のドッグフーディング: `didOpen` の 1 ms 後の `findReferences` で宣言だけを得て export された関数を消し `tsc` が TS2305、gopls で新規ファイルの参照 0 件で消し `go build` 失敗。Serena ではないことを明記）、仕様（https://github.com/tagawa0525/lsp-det/blob/main/docs/spec/server-state.md。9 章がクライアント側の規則）、サーバー自身が報告し始めている例（rust-analyzer の `ready: bool`、rust-lang/rust-analyzer#23331 / #23362）と LSP 本体への提案（ドラフト。こうした実装からの反応の後に形を決める、と明記）、その先（`ready` が保証する範囲の宣言、`didChangeWatchedFiles` 後の鮮度）。

出す前に当て直すこと（返信・issue 共通）: 行番号が `main` の最新で動いていないか、#1978 と #2007 が動いていないか、リンク先。issue で `serena-integration-measurement.md` 等の日本語の報告にリンクするなら、先に英訳して日本語版を `.ja.md` にリネームする（英語が正の文書にする。ADR 0017 の追補が要る）。
