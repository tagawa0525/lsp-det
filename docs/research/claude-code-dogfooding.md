# Dogfooding observations through Claude Code (2026-09-03)

[日本語](claude-code-dogfooding.ja.md)

Using the procedure in `dogfood/README.md`, lsp-det is placed in front of Claude Code (CC) to observe how CC uses a language server. The goal is to fill in the observation items from design chapter 8 (when CC starts the server, when it sends its first cross-workspace request, the request timeout and how errors are shown, how it handles an unknown notification). This document is appended to after each observation.

## Observation environment

- CC: `claude --plugin-dir dogfood/claude-plugin` (same as `dogfood/README.md`; adding `-c` to resume a session does not change the behavior). Round 1 was without `--debug`, round 2 with `--debug`. The working directory is this repository, and direnv puts `target/release` on the PATH
- lsp-det: a release build of main `f9b8237`
- Upstream: rust-analyzer 2026-08-03 (a nixpkgs build pinned by flake.nix)
- Confirmation method: CC's LSP tools (hover / findReferences) plus checking the process lineage with `ps` / `/proc`. Round 3 made the subject a nested non-interactive CC (`claude -p "<send findReferences once and write the result verbatim>" --plugin-dir <this repository>/dogfood/claude-plugin --debug --output-format json --allowedTools LSP`), launched with the target directory as cwd. The `session_id` in `--output-format json` locates the debug log

## Round 1 (2026-09-03): the path forms, and startup timing

### Conclusion

| Item                                                     | Observation                                                                                                                                                                                                                                                                    |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| The path forms                                           | **It forms.** The process lineage is `claude` -> `lsp-det -- rust-analyzer` -> `rust-analyzer-unwrapped-2026-08-03`. lsp-det's binary is `target/release/lsp-det`, cwd is the repository                                                                                       |
| When CC starts the server                                | **Not at session start, but on the first LSP tool call.** For a while after the session starts there is no lsp-det process; it appears right after a hover is sent                                                                                                             |
| A non-cross-workspace request right after startup        | The first hover returned "no hover information". hover is not a cross-workspace request under spec 7.0, so the downstream side forwards it without holding, and the still-indexing rust-analyzer returned null. CC shows this as "not on a symbol, or the index is incomplete" |
| After the index completes                                | A hover at the same position returns the type and documentation, and `findReferences` returns 6 locations across 2 files. The response goes through lsp-det                                                                                                                    |
| Conflict with another plugin handling the same extension | The official `rust-analyzer-lsp` plugin, enabled in settings, only has a README in its cache and carries no LSP definition, so there is no conflict. CC's only child process is lsp-det                                                                                        |
| Log of the mapping selection                             | lsp-det's stderr is wired to CC's socket. Without `--debug` nothing is kept, so confirming the declared content requires restarting with `claude --debug` (next round)                                                                                                         |

### Implications for the design

- A "space response from an incomplete index" occurring outside a cross-workspace request (hover, documentSymbol, etc.) is outside the scope of design 4.3's decision table, so it is visible as-is. This is as designed; whether to widen the scope is a question for spec 7.0's definition of a cross-workspace request, not something lsp-det decides alone
- Because CC lazily starts the server, "a cross-workspace request arrives right after startup" happens routinely with CC. The downstream side's hold (holding `references` while `indexing` and releasing on `ready`) has real meaning for CC

## Round 2 (2026-09-03): a cross-workspace request right after startup, and the `--debug` log

Reopened with `claude --debug`, and with the language server not yet running, sent `findReferences` as the first operation. With `--debug`, CC keeps the language server's stderr and the sent/received LSP messages in `~/.claude/debug/<session id>.txt`.

### Conclusion

| Item                                              | Observation                                                                                                                                                                                                                                                          |
| ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Mapping selection and declaration                 | stderr showed `upstream is "rust-analyzer" version "2026-08-03"; using its mapping, declaring {"completeness":true,"freshness":true}`. Since the version is on the tested-versions list, the guarantee is declared                                                   |
| When CC sends its first cross-workspace request   | **6ms after** the `initialize` response. The order is `initialized` -> `didOpen` (one target file) -> `textDocument/references`, not waiting for the index to complete                                                                                               |
| The downstream side's hold                        | The state when `references` arrived was `{ok, indexing}`. lsp-det held it and forwarded on `ready` 6.7 seconds later. CC received the **complete result** (6 locations in 2 files). Without the hold this would have been the empty response seen in round 1's hover |
| CC's request timeout                              | Not timed out at 8.4 seconds to the response (6.7s hold + 1.7s of rust-analyzer processing). The upper bound is unknown from this observation (only that it is longer than 8.4 seconds)                                                                              |
| How CC handles a notification it has not declared | 223 `$/progress` notifications reached CC, with no error or log warning. `experimental/serverStateChanged` is by design not forwarded to CC when a mapping is used, so it never arrived (design 4.2)                                                                 |
| A server-to-client request                        | CC answered rust-analyzer's `workspace/diagnostic/refresh` in 0ms. The only thing lsp-det stands in for is `window/workDoneProgress/create`, which comes from the injected capability, and it is passed through as-is                                                |

Timeline (times from the `--debug` log, with the `initialize` send at 0):

| Time   | Event                                                                                |
| ------ | ------------------------------------------------------------------------------------ |
| 0.000s | CC sends `initialize`. lsp-det's initial state is `{unknown, unknown}`               |
| 0.005s | `initialize` response. A mapping is chosen, moving to `{unknown, initializing}`      |
| 0.006s | CC sends `initialized`, `didOpen`, and `references (1)`                              |
| 0.008s | The first `experimental/serverStatus` reports `{ok, indexing}`. `references` is held |
| 6.715s | `{ok, ready}`. The hold is released                                                  |
| 8.376s | The `references (1)` response reaches CC                                             |

### Implications for the design

- "CC sends a cross-workspace request right after startup" is now confirmed as fact. The downstream side's hold directly serves this way CC uses the tools
- CC's request timeout was not hit by this round's hold (6.7 seconds). It remains unobserved how CC shows a long hold in a large workspace. Since the spec bans a time-based cutoff (chapter 6 item 6), the response to a hold that does trigger CC's timeout is the path where CC's `$/cancelRequest` cancels the hold (design 4.3)
- CC silently accepts an unknown notification (`$/progress`). Design 4.2's condition for switching to swallowing them ("if a problem turns up") has not been met so far

## Round 3 (2026-09-03): a long hold, the gopls path, and rejecting `error`

The 3 unobserved items from round 2 were filled in using prior-art repositories under `reference/`. Each subject is again a nested non-interactive CC, sending a single `findReferences` right after startup (see Observation environment).

### Conclusion

| Subject                                                                 | Upstream                 | State when `references` arrives | Downstream side's judgment                        | Time to response | How CC shows it                                                                                                                                                                      |
| ----------------------------------------------------------------------- | ------------------------ | ------------------------------- | ------------------------------------------------- | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `fn main` in `reference/zed` (Rust, 1935 files, 1815 dependency crates) | rust-analyzer 2026-08-03 | `{ok, indexing}`                | Held, forwarded 80.6s later on `{warning, ready}` | **82.4 seconds** | The complete result (1 location). **No timeout, no `$/cancelRequest`**                                                                                                               |
| `AnalysisHost` in `reference/rust-analyzer` (Rust, 1481 files)          | rust-analyzer 2026-08-03 | `{ok, indexing}`                | Held, forwarded 2.7s later on `{warning, ready}`  | 3.6 seconds      | The complete result (45 locations in 11 files; grep also finds 11 files)                                                                                                             |
| `packages.Load` in `reference/golang-tools` (Go, 1935 files)            | gopls v0.23.0            | `{unknown, indexing}`           | Held, forwarded 0.67s later on `{ok, ready}`      | 0.9 seconds      | The complete result (147 locations in 45 files)                                                                                                                                      |
| `lib.rs` in a directory with no Cargo.toml                              | rust-analyzer 2026-08-03 | `{error, ready}`                | Rejected (RequestFailed)                          | 3ms              | `Error performing findReferences: LSP request 'textDocument/references' failed for server '…': lsp-det: the language server reports health: error (Failed to discover workspace. …)` |

- **CC's request timeout does not trigger even on an 82-second hold.** The upper bound is still unknown, but it is confirmed that a hold does not become a problem for a real large workspace (zed)
- **`warning` is forwarded** (as design 4.3's decision table says). In the zed and rust-analyzer repositories, a build script failed under nixpkgs's rustc, putting health at `warning` ("Failed to run build scripts of some packages."), but `references` still returned a complete result. "Complete" here means complete apart from code that comes from the build script, a distinction rust-analyzer's vocabulary cannot draw any finer
- **rust-analyzer's `ready` is not proportional to the file count.** The rust-analyzer repository (1481 files) took 2.7 seconds, this repository (a few dozen files) 6.7 seconds, and zed 80.6 seconds. `quiescent`'s substance is loading the VFS and priming caches (ADR 0007); what governs the time is running build scripts and proc macros
- **A rejection with `error` reaches CC as-is.** CC displays the reason text lsp-det attached (including rust-analyzer's own `message`) as the error body, and it is readable enough to point to the next step (setting `linkedProjects`)

### A byproduct: CC's `shutdown` carries `params: {}`, and rust-analyzer rejects it

CC attaches `params: {}` to its `shutdown` request on exit. rust-analyzer (the lsp-server crate) reads `shutdown`'s params as `()`, so it returns InvalidParams (-32602) with `invalid type: map, expected unit`, and CC logs "Failed to stop LSP server" as an error and disconnects without sending `exit`. gopls accepts `params: {}`. lsp-det just forwards the body as-is and is not involved; a direct connection was confirmed to produce the same response (both accept it and return `result: null` if `params` is omitted). After CC disconnects, lsp-det takes the upstream down with it on stdin's EOF (design 4.5), and across 4 rounds of observation no orphan process has been left. This is a problem on CC's side (or rust-analyzer's own strictness), and should happen the same way with the official rust-analyzer plugin too.

### Implications for the design

- The downstream side's hold held up consistently for CC's actual "cross-workspace request right after startup", from small (7 seconds) to large (80 seconds). Even without the cutoff timer the spec bans (chapter 6 item 6), CC is not troubled by it
- CC never sent `$/cancelRequest` in any of the 4 rounds of observation. Design 4.3's cancellation path has only been confirmed by the conformance tests

### Unobserved (for future rounds)

- The actual value of CC's request timeout (only known to be longer than 82 seconds)
- The conditions under which CC sends `$/cancelRequest`
- How CC shows it when gopls's health becomes `error` ("Error loading workspace")

## Round 4 (2026-09-04): the full set of notifications CC sends, and `initialize`'s capability

All 29 `--debug` logs (6 with LSP traffic) were checked in full, and a wrapper that records stdin with `tee`, placed where the language server would be, captured the `initialize` of a nested non-interactive CC (2.1.259) verbatim. The measurement itself is in [disk-edit-propagation-measurement.md](disk-edit-propagation-measurement.md) (Japanese).

### Conclusion

| Item                      | Observation                                                                                                                                                                                                                    |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Notifications CC sends    | Only 3 kinds: `initialized`, `textDocument/didOpen`, `exit`. `didChange`, `didSave`, `didClose`, `workspace/didChangeWatchedFiles`, and `$/cancelRequest` never once appear across the 29 logs                                 |
| After a Write             | 1ms after writing (writing a temp file and renaming it), it resends `didOpen` for the same file (new content, not closed). Only for languages with a server. The trigger is the diagnostics path, not an LSP tool call         |
| After a Bash edit         | Nothing is sent (none of the 377 Bash invocations are accompanied by an LSP line)                                                                                                                                              |
| `initialize`'s capability | `workspace` is only `{configuration: false, workspaceFolders: false}`, **with no `didChangeWatchedFiles`**. `textDocument.synchronization` declares `didSave: true` but never sends it. There is no `window` or `experimental` |
| A request from the server | It answers `workspace/diagnostic/refresh` with `-32601 Unhandled method`                                                                                                                                                       |

### Implications for the design

- Since gopls and pyright do not watch files on their own, CC's Bash edits stay invisible for the entire session. rust-analyzer picks them up through its own notify, when watching is not declared. tsls refuses the second `didOpen` and keeps a stale buffer (disk-edit-propagation-measurement.md). The basis for ADR 0015's two stand-ins
- Material for the report to CC (`docs/upstream-submissions.md`, Japanese)

## Round 5 (2026-09-06): confirming the two downstream stand-ins with CC 2.1.261

Whether ADR 0015's two stand-ins (standing in for `workspace/didChangeWatchedFiles`, rewriting a duplicate `didOpen` into `didChange`) hold up on real CC was checked by comparing **with and without lsp-det**. This is the measurement behind writing "it disappears once lsp-det is placed in front" in the report to CC.

### Method

- Subject: a nested non-interactive CC 2.1.261 (`claude -p "<procedure>" --plugin-dir <plugin> --debug-file <log> --output-format json --allowedTools "LSP,Bash" --model sonnet`. The procedure is fixed, and the subject is not allowed to read files). The model is Sonnet (the subject is CC's LSP client; the model itself is not relevant)
- Two plugins: **direct** (`command` is the tee wrapper, recording the server's stdin) and **through lsp-det** (`lsp-det -- <tee wrapper>`, recording what lsp-det writes to the upstream)
- lsp-det: a release build of main `1234bdd` (0.3.0). Upstream: gopls 0.23.0, typescript-language-server 5.3.0 (TypeScript 5.9.3)
- Test subject: a directory with only `git init` run (the stand-in also picks up untracked files through `git ls-files --others`). Go has `a.go` (`func Target()` and a call to it in `main`) and `b.go` (one call). TS has `a.ts` (`export function target()`) and `b.ts` (an import and one call)
- Procedure (Go): `findReferences` (`Target`) -> create `c.go` with Bash (one call) -> `findReferences`. Procedure (TS): `findReferences` (`target`) -> overwrite `b.ts` with Write (two calls) -> `findReferences` -> overwrite again with Write (three calls) -> `findReferences`

### Results

| Test subject                                    | Direct                                                                                                                                                                             | Through lsp-det                                                                                                                       |
| ----------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| gopls: count after adding `c.go` with Bash      | 3 -> **3** (`c.go` is invisible; only `didOpen a.go` and the references request reached the upstream)                                                                              | 3 -> **4** (`c.go` included; `workspace/didChangeWatchedFiles` `Created c.go` reached the upstream before the 2nd references request) |
| tsls: count on the 1st call right after startup | **1** (only `a.ts`'s declaration; the project loads after `didOpen`, and this is the response from before that)                                                                    | **3** (held 0.175s while `indexing`, forwarded the complete result on `ready`)                                                        |
| tsls: count from the 1st Write to the 2nd       | 4 -> 5                                                                                                                                                                             | 4 -> 5                                                                                                                                |
| What CC sends after the 2nd Write               | `textDocument/didChange` (full text, version 2) and `textDocument/didSave`                                                                                                         | Same (no rewrite occurs)                                                                                                              |
| `initialize`'s capability                       | Same as round 4. `workspace` is `{configuration: false, workspaceFolders: false}`, **with no `didChangeWatchedFiles`**, and no `didChangeWatchedFiles` notification is sent either | Same                                                                                                                                  |
| `shutdown`                                      | `params: {}` (same as round 4)                                                                                                                                                     | Same                                                                                                                                  |

### Conclusion

- **The `didChangeWatchedFiles` stand-in works on real CC.** CC 2.1.261 neither declares nor sends watching, and gopls does not watch on its own, so directly, a file Bash creates stays invisible for the session. With lsp-det in front, a Created arrives before the next cross-workspace request and is included in the result
- **A duplicate `didOpen` does not occur on CC 2.1.261.** 2.1.259 (round 4) resent `didOpen` on every Write, but 2.1.261 sends `didChange` (full text) and `didSave` on a second Write to an already-open document. ADR 0015 decision B's rewrite does not fire on CC; its typical trigger has disappeared on CC's side. The rewrite itself stays in place as a general remedy for an LSP violation
- **The silent lie right after startup shows up on CC with tsls too.** Directly, the first `findReferences` returns just the one declaration, and CC takes it as the result. Through lsp-det it holds until `ready` and returns 3
- The report to CC (`docs/upstream-submissions.md`, Japanese) shrinks to 4 items: a cross-workspace request right after startup (now on tsls too, in addition to rust-analyzer), `shutdown`'s `params: {}`, the missing `didChangeWatchedFiles`, and not sending `didClose`. `didChange` / `didSave` are now sent
- Through lsp-det, the stand-in also sends `Changed b.ts` after CC's Write (a disk change to an already-open document; harmless, since the server prefers the open buffer)
- `exit`: CC stops the client in the same millisecond it sends `exit`. Through lsp-det, `exit` never appeared on the upstream's stdin, yet the upstream still exited with no process left behind (either the EOF or the process-lifetime path; the cause was not pinned down)
- Cost: 5-7 turns per run, around $0.10 with Sonnet

## Round 6 (2026-09-06): one concrete case of real-world harm. An agent that trusts an empty response deletes a function still in use

Round 5 was the control -- "the result becomes complete once lsp-det is placed in front". As the external review ([external-review-2026-09.md](external-review-2026-09.md) §4, Japanese) pointed out, moving upstream needs a record of "how it went wrong without the hold". ADR 0018 decision A-2.

### Method

The same tools as round 5 (nested non-interactive CC 2.1.261, Sonnet, tee wrapper, a `git init`-only test subject, direct / through lsp-det). lsp-det is a release build of main `ad39c33` (with logging of when a hold starts and is released). Two kinds of instructions.

- **Directed version**: a fixed procedure. "Call `findReferences`, and if the count excluding the declaration is 0, delete the function with Edit. If it is not 0, do nothing." The subject is not allowed to read files
- **Natural version**: "Clean up unused functions. Judge only by `findReferences`, without reading files or grepping"

Two test subjects.

- **Right after tsls starts**: `a.ts` has `target` (used from `b.ts`) and `other` (unused). It asks about `target`'s references
- **gopls with a file created by Bash**: `a.go` has `Helper` (initially unused). `findReferences` is called first to start the server, then Bash creates `c.go`, which uses `Helper()`, and `findReferences` is called again. Since CC starts the server on its first LSP tool call, the server has to be started first, or else `c.go` would already be visible from startup (the first 4 runs were invalidated by this design mistake and discarded)

After the deletion, `tsc -p .` / `go build ./...` checked whether the build broke.

### Results

| Run                     | Direct                                                                                                                                                                                                                                      | Through lsp-det                                                                                                                                                                                                                                  |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| tsls, directed version  | `references` 1ms after `didOpen`. A declaration-only response in 213ms (0 usages). **Deletes `target`.** `tsc`: `b.ts(1,10): error TS2305: Module '"./a"' has no exported member 'target'.`                                                 | Held for 0.244s (`holding … while {ok, indexing}` -> `released … after 0.244s: ready`). 2 usages. Does not delete. `tsc` passes                                                                                                                  |
| tsls, natural version   | Deletes only the unused `other` (correct). `target` survives. The agent took 4.3 seconds thinking before its first `references`, and the project had finished loading by then. The query position is the start of the line (above `export`) | Same result                                                                                                                                                                                                                                      |
| gopls, directed version | The `references` after creating `c.go` also returns 0. **Deletes `Helper`.** `go build`: `./c.go:3:9: undefined: Helper`                                                                                                                    | The stand-in's `didChangeWatchedFiles` (`Created c.go`) arrives before the 2nd `references`, returning 1. Does not delete. `go build` passes                                                                                                     |
| gopls, natural version  | After seeing 0 twice, the agent touches `c.go` through an LSP tool (CC sends `didOpen c.go`), and gopls sees `c.go`, returning 1. Does not delete. Avoided by chance                                                                        | 2 (including the declaration) from the stand-in's `Created`. Does not delete. The agent explains "a reference from a file I just created is self-fulfilling" and "`main` isn't deleted even at 0 references", not following the JSON instruction |

Cost: $1.50 for 12 runs (including the 4 invalidated ones).

### Conclusion

- **One concrete case of real-world harm was captured.** In the directed version, directly, the agent deletes a function still in use both with tsls (a declaration-only response right after startup) and with gopls (a new file it was not told about), breaking the build in both. Through lsp-det, the hold (tsls) and the stand-in (gopls) produce the correct count and it is not deleted
- **Whether harm occurs is decided not by the instruction but by the timing and steps before the query.** In the natural version, the agent escaped harm because loading finished during the few seconds it spent thinking, or because touching another file happened to inform the server. This does not mean "a slow agent is fine" -- it means "a faster agent grabs the lie" -- and the 1ms-later query in the directed version is CC's actual behavior (round 2: 6ms after the `initialize` response)
- The report to CC and the wording to upstream will carry the directed version's 2 examples (the raw response, the build error after deletion), with the natural version noted alongside as "the same lie, with harm that happens to vanish"
- The log of a hold starting and being released (ADR 0018 decision A-1) stays intact in the `claude --debug` log, and the hold durations (0.244s, 0.029s) are readable there

## Round 7 (2026-09-08): 3 languages from a daily-use environment. The Nix path forms

0.6.0 (ADR 0021 decision F). Instead of `--plugin-dir`, this uses an environment where the nixfiles pull in lsp-det as a flake input, put `packages.default` on the PATH, and place `dogfood/claude-plugin` at `~/.claude/skills/lsp-det-dogfood` (nixfiles PR #180, already rebuilt). CC reads `lsp-det-dogfood` as skills-as-plugins and registered 5 LSP servers ("Loaded 1 skills-as-plugins" and "Loaded 5 LSP server(s) from plugin: lsp-det-dogfood" in the `--debug` log). The official `rust-analyzer-lsp` is disabled.

### Method

- Subject: a nested non-interactive CC 2.1.263 (`claude -p "<send findReferences once and write the result verbatim. Do not read files>" --debug-file <log> --output-format json --allowedTools LSP --model sonnet`, no `--plugin-dir`). Launched with cwd set to each repository
- Test subjects: for Nix, `~/nix/nixfiles` (`claudeCodeStaticSettings` in `modules/home/parts/claude-code.nix`; used in 1 place); for Rust, `colors_for_model` in `src/chart.rs` of `~/github/cc-bar` (rust-overlay's rust-analyzer 1.97.1, launched inside `nix develop`); for Python, `main` in `.claude/hooks/guard-migrations.py` of `~/github/almanaut`
- lsp-det: the first 3 runs picked up the working-tree build (main `41860e3`, with decision E in) that direnv, in the parent shell with this repository as cwd, put at the front of the PATH. Nix and Python were rerun with the `lsp-det` from the profile (nixfiles's lock `649528f`, before decision E). The hold and release are the same

### Results

| Language | Naming and declaration                                                                                                                     | Hold                                                                                                    | Result                                                                                                                                               |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| Nix      | nixd 2.9.2. `{"coverage":{"scope":"document","incomplete":{}}}` (`{}` under `649528f`)                                                     | `references` 1ms after `didOpen`, held 0.386s between two evaluations (`indexing`), released on `ready` | 1 result (line 272). Correct                                                                                                                         |
| Rust     | rust-analyzer 1.97.1 (8bab26f 2026-07-14). `coverage: {scope: "workspace", incomplete: {"workspace/symbol": 128}}`, 3 kinds of `freshness` | Held 18.4 seconds (`indexing` -> `ready`)                                                               | 6 results (declaration + 5)                                                                                                                          |
| Python   | pyright 1.1.412 (from the startup log). `coverage: {scope: "workspace", incomplete: {}}`, `freshness: {fileChanges: ["Changed"]}`          | Held 0.25s under `initializing`                                                                         | 2 results (declaration + 1). The first run was off by one column (right after `def`'s space) and got 0. CC's LSP tool uses 1-based lines and columns |

CC's behavior:

- **CC does not support `workspace/configuration`.** nixd's log shows "workspace/configuration: client does not support workspace configuration". Since nixd evaluates with the default expression, there is no effect. nil similarly runs with defaults, unable to receive settings
- CC answers `window/workDoneProgress/create` (nixd's "<-- reply(1)"). `$/progress` is received by CC and discarded ("Received notification '$/progress'")
- `shutdown`'s `params: {}` (round 3) is still there on 2.1.263, and rust-analyzer 1.97.1 rejects it with "invalid type: map, expected unit", so CC logs an ERROR. nixd and pyright accept it

Cost: 6 runs.

### Conclusion

- **The Nix path forms in a daily-use environment.** `references` between nixd's two evaluations is held, and the answer after release is the complete result within the document
- **skills-as-plugins plus a flake input route 3 languages through lsp-det.** `--plugin-dir` is not needed. In the directory where direnv puts the working-tree build at the front of the PATH (this repository), the working-tree build wins, and the path is the same
- Unobserved: whether CC answers nil's `window/showMessageRequest` (low priority, since the daily route uses nixd)

## Re-measurement before submission (2026-09-09, CC 2.1.266)

Right before the stage 1 submissions (`docs/upstream-submissions.md`), I checked that the facts to be reported still hold on the latest CC. The tool is `scripts/claude-code/tee-probe/`, a plugin that puts a recording wrapper in the language server's place (the ad hoc wrapper of rounds 4 onward, saved). The subject is a crate right after `cargo init` with `pub fn target()` and a `caller()` that calls it twice. A nested non-interactive CC 2.1.266 (`claude -p … --plugin-dir scripts/claude-code/tee-probe --allowedTools LSP --model sonnet`) was told to issue one `findReferences`. rust-analyzer 2026-08-03, direct (no lsp-det). Two runs, same result.

| Item                                       | Observed on 2.1.266                                                                                                                                                                                                                                                   | Before                 |
| ------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------- |
| When the first cross-file request goes out | Right after the `initialize` response (19 ms): `initialized` → `didOpen` → `references` within 1 ms of each other (5.2 ms, 5.2 ms, 5.4 ms on the wrapper's clock). rust-analyzer answers `[]` in 2 ms and CC's tool result says "No references found …" (there are 2) | Round 2: 6 ms after    |
| `initialize` capabilities                  | `workspace: {configuration: false, workspaceFolders: false}`. No `didChangeWatchedFiles`. `textDocument.synchronization` is `{dynamicRegistration: false, willSave: false, willSaveWaitUntil: false, didSave: true}`. No `window`, no `experimental`                  | Same as rounds 4 and 5 |
| Notifications sent                         | Only `initialized` and `textDocument/didOpen`. No `didClose`                                                                                                                                                                                                          | Same as round 4        |
| `shutdown`                                 | `params: {}`. rust-analyzer rejects it with "Failed to deserialize shutdown: invalid type: map, expected unit", CC logs the ERROR "Failed to stop LSP server" and disconnects without sending `exit`                                                                  | Same as rounds 3 and 7 |

All five points reported in stage 1 (the cross-file request right after startup, the missing `didChangeWatchedFiles`, never sending `didClose`, no `workspace/configuration` support, `shutdown` with `params: {}`) are still present on 2.1.266.

## Points not to generalize

- "Starts on the first LSP tool call" and "sends a cross-workspace request right after `initialize`" are observations of this version of CC. They can change with CC's version (indeed, the re-`didOpen` on a Write was observed on 2.1.259 and had disappeared by 2.1.261)
- The window during which the hover right after startup returns null, and the 6.7-second hold, are figures for this repository's size and do not generalize to other workspaces
- "Not timed out at 82 seconds" is only a lower bound; CC's actual timeout value itself is unknown
- Round 6's natural-version result (harm avoided) varies by model and by run. Only the directed version is a reproducible record; the natural version is a caveat that "it can also vanish by chance"
- zed's 80.6 seconds was measured running alongside other subjects (sharing CPU), so it would be shorter alone. Read only the ordering (small < large)
