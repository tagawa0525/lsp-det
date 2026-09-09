# LSP Deterministic Extensions — Long-Term Vision

[日本語](vision.ja.md)

> A minimal extension that lets a client handle LSP (Language Server Protocol) responses mechanically, without interpretation.
> It does not replace existing LSP; it defines a spec that, once conformed to, removes the need for correction. The current core is server state; declaration range and the launch manifest are frozen ([adr/0003](adr/0003-extension-s-zero-based.md) (Japanese) decision 6).
> The motive is coding agents, but the audience is every LSP client, editors included.
> The English text is the primary one; the Japanese version ([vision.ja.md](vision.ja.md)) is a translation kept in sync (ADR 0017 addendum F).

## Where This Document Stands

This document is the long-term vision for specifying and standardizing three extensions (declaration range, server state, launch manifest). The normative text for server state is [spec/server-state.md](spec/server-state.md) (the Japanese version is [spec/server-state.ja.md](spec/server-state.ja.md)), and the current implementation scope is defined in [v0.1-design.md](v0.1-design.md). Within this document, chapter 5 (the path to upstream proposals) and the standardization investigation tasks in 6.7 are frozen; whether to resume them is decided after v0.1 is running stably. For the reasons behind the scope selection, see [adr/0001-tool-first-readiness-gate.md](adr/0001-tool-first-readiness-gate.md) (Japanese) and [adr/0003-extension-s-zero-based.md](adr/0003-extension-s-zero-based.md) (Japanese).

---

## 0. Purpose and Non-Goals

### Purpose

LSP was designed on the premise of "information for an editor to display", and the following are deliberately loose.

| Item          | LSP's current state                                          | Problem that occurs for agents                                         |
| ------------- | ------------------------------------------------------------ | ---------------------------------------------------------------------- |
| Symbol range  | The start/end points of `range` are implementation-dependent | Cutting and pasting the range produces corruption like `type type Foo` |
| Readiness     | `$/progress` is optional, and its format is server-dependent | An empty response mid-index is misread as "no results"                 |
| Launch method | Out of scope for the spec                                    | Each client reimplements launch code for 70 languages                  |

This looseness was already a problem in the editor era. Standardizing readiness was proposed from the editor side in 2018 (LSP issue #511), and VS Code, Neovim, and Zed each carry their own correction code per language server. But human eyes and a sense of timing supplemented the final judgment, so it was enough for each editor to handle it on its own.

Coding agents have no such compensation. They take the response literally and cut and paste mechanically, so the same looseness shows up directly as corruption. And because one client handles all languages, the correction cost that was spread across the editor world piles up in one place (about 27,000 lines of language-specific code in Serena).

In other words, agents did not create a new problem — they made it visible and gave it a stronger motive. This spec tightens the vocabulary under the problem definition of "removing silent lies", with the goal of removing the correction code from both agents and editors.

### Non-goals

- Defining composite queries (impact analysis, call paths, and the like). That is the role of a separate layer placed above this spec (LSAP and the like)
- Specifying output formats (Markdown and the like)
- Specifying how to connect to MCP
- Changing the meaning of existing LSP methods

### Design Principles

1. **Backward compatibility**: A non-conformant server keeps working as it is. Conformance is declared through a capability
2. **Minimal requirements**: What is asked of the server is only the minimal vocabulary that follows from the problem definition. Future candidates are added as extensions of the server state vocabulary, not casually as new independent specs
3. **Conformance tests are the substance**: Writing it in prose does not make it followed (the precedent of `documentSymbol.range`). Only what a test can verify is specified
4. **Lead with the proxy**: Provide a reference proxy that makes a non-conformant server look conformant, so the spec can be used without waiting for upstream to catch up

### Where to Tighten (Why LSP, Not the Bridge)

There are two places to define a strict vocabulary for agents.

```text
(a) Tighten at the bridge: agent ── [strict API] ── bridge ── [loose LSP] ── language server
(b) Tighten at LSP:        agent ── [anything]   ── proxy  ── [strict LSP] ── language server
```

(a) corresponds to specifying and guaranteeing Serena's output, and it is also the approach LSAP / LSAI take. It has enough value on its own, and it is faster to implement.

Why this spec chooses (b):

- In (a), the responsibility for correction stays permanently concentrated in the bridge. Every time a language server is added or changes, the bridge has to keep following it. Serena's 27,000 language-specific lines are the consequence of this structure
- In (b), the correction sits in the proxy only temporarily, aiming to eventually push it into the language server itself and make it disappear. The more conformant servers there are, the thinner the proxy becomes
- A vocabulary tightened at (b) benefits parties other than agents too (editors, LSIF, static analysis tools). (a) stays closed to agents
- Since the end goal is to be absorbed into LSP, writing it as an extension of LSP's own types (`DocumentSymbol` and the like) carries directly over into the shape of a proposal

The layer of (a) (composite queries, agent-facing output formats) is placed separately on top of this spec. This spec does not replace (a); it is the foundation for removing language-specific correction from (a)'s implementation.

---

## 1. Declaration Range Contract

### 1.1 Problem

`textDocument/documentSymbol`'s `DocumentSymbol.range` is specified as "the range enclosing this symbol not including leading/trailing whitespace but everything else like comments", but implementations do not follow it.

Real examples:

- gopls: the range of `type Foo struct` starts at `Foo` and does not include `type`. For functions it does include `func`
- Each server treats decorators / attributes / doc comments differently

### 1.2 Specification

A conformant server adds the following to `DocumentSymbol`.

```typescript
interface DocumentSymbol {
  // existing fields unchanged
  range: Range;
  selectionRange: Range;

  // added
  declarationRange?: DeclarationRange;
}

interface DeclarationRange {
  /**
   * The whole declaration. Includes all of the following:
   *   - the language keyword (type, func, class, def, fn, ...)
   *   - modifiers (pub, static, async, export, ...)
   *   - annotations / decorators / attributes immediately preceding it
   *   - the body (if any) and its closing bracket
   * Does not include:
   *   - the doc comment (returned separately below, in docRange)
   *   - blank lines before or after
   *   - a newline after a trailing semicolon
   */
  full: Range;

  /**
   * The body only. For a function, the inside of { }; for a class, the
   * class body. Omitted for a symbol with no body (a variable
   * declaration, an import, and the like). Does not include the
   * brackets themselves.
   */
  body?: Range;

  /**
   * The doc comment immediately preceding it. Omitted if none.
   */
  doc?: Range;

  /**
   * Whether the server guarantees that "replacing this whole range
   * still leaves the syntax consistent". If false, the client must
   * not use full for cut-and-paste.
   */
  replaceable: boolean;
}
```

### 1.3 Capability

```typescript
interface ServerCapabilities {
  documentSymbolProvider?: boolean | {
    // added
    declarationRange?: boolean;
  };
}
```

### 1.4 Conformance Tests

For each language, prepare a fixture that includes at least the following cases, and fix the expected values of `full` / `body` / `doc` at byte offsets.

- Declarations with a keyword (functions, types, classes)
- With modifiers (public, static, async, export)
- With annotations / decorators / attributes
- With a doc comment
- No body (variables, constants, imports)
- Nested symbols
- Single-line and multi-line declarations

The test author generates the expected values by hand. The server's own output is not used as the expected value as-is.

---

## 2. Server State

### 2.1 Problem

Many servers are still indexing after `initialize` completes, and answer queries during this time with an empty array or a partial result. `$/progress` is optional, and the meaning of the progress (what finishing means for what can be answered) is server-dependent. Beyond that, the response cannot distinguish the server's death or a partial failure, and there is no way to know whether a response has incorporated a recent edit. An agent cannot tell "no results" apart from "cannot answer yet", "cannot answer any more", and "a stale answer". All of these show up as a "response that is valid on the protocol but not true" — a silent lie (the evidence is in [research/lsp-pain-points-synthesis.md](research/lsp-pain-points-synthesis.md) (Japanese)).

### 2.2 Specification

The normative text is [spec/server-state.md](spec/server-state.md) (the Japanese version is [spec/server-state.ja.md](spec/server-state.ja.md)). In brief:

- The `workspace/serverState` request and the `workspace/serverStateChanged` notification return `ServerState`
- `ServerState` has two axes, `health` (`ok | warning | error`) and `readiness` (`initializing | indexing | ready`). A party that observes the server from outside (a proxy, a client library, and the like) may add `unknown` to both axes. The server's death is conveyed by the end of the connection, not by a value
- As long as `health` is not `error`, `ready` guarantees coverage (based on the index of the whole workspace, with the result not growing later) and freshness (every received `didChange` has been incorporated) according to the declared guarantees (`coverage` / `freshness`)
- Diagnostic phases and a freshness token are reserved as forward-compatible future extensions
- Combining with a partner server (a setup like Vue's Hybrid Mode, where a separate-process TS server gives the cross-workspace answer) is the client's responsibility. It is enough for the client to AND the `ServerState` of each connection, and an observer (lsp-det) does not need to know about the connection next to it. This was measured with Vue: holding only the connection that gives the cross-workspace answer was enough to make the result complete ([research/vue-composition-measurement.md](research/vue-composition-measurement.md) (Japanese), [adr/0019](adr/0019-v0.4-corpus-and-counterexamples.md) (Japanese) decision B-5)

The former name "Extension B (Readiness)" handled only readiness, but it was redefined as server state by integrating it with health and freshness ([adr/0003](adr/0003-extension-s-zero-based.md) (Japanese)). The two-layer structure of the spec and the removal of `dead` are in [adr/0009](adr/0009-success-criterion-and-two-sided-reference.md) (Japanese). The `$partial` annotation and `completeMethods` that were in the initial draft were dropped because they cannot be attached to an array response and have no effect on an unmodified client.

---

## 3. Launch Manifest

### 3.1 Problem

How to obtain a language server, its launch command, its initialization options, and how to determine the project root are all out of the spec's scope, and each client implements them per language. The existing de facto standards (Claude Code's `.lsp.json`, nvim-lspconfig, the Mason registry) are mutually incompatible.

### 3.2 Specification

A language server's distribution bundles an `lsp-manifest.json`, or makes one obtainable from a registry.

```typescript
interface LaunchManifest {
  /** The manifest format version */
  manifestVersion: "1";

  /** The server identifier. A reverse FQDN is recommended (e.g. "org.golang.gopls") */
  id: string;

  /** The languageId and extensions it supports */
  languages: {
    languageId: string;
    extensions: string[];
    filenames?: string[];   // "Makefile" and the like
  }[];

  /** How to launch it */
  launch: {
    command: string;
    args?: string[];
    env?: Record<string, string>;
    transport: "stdio" | "socket" | "pipe";
    /** How to specify the port, for socket */
    socket?: { arg: string };
  };

  /** How to obtain it. If omitted, it is expected to be on PATH */
  install?: {
    /** The detection command. Available if it exits 0 */
    detect: { command: string; args?: string[] };
    /** Getting the version. Returns stdout as-is */
    version?: { command: string; args?: string[] };
    /** How to obtain it, per platform. The client gets the user's approval before running it */
    sources?: {
      platform?: ("linux" | "darwin" | "windows")[];
      arch?: ("x64" | "arm64")[];
      /** Via a package manager */
      package?: { manager: "npm" | "pip" | "cargo" | "go" | "brew" | "apt"; name: string };
      /** Direct archive download */
      archive?: { url: string; sha256: string; binaryPath: string };
    }[];
  };

  /** How to determine the project root. Evaluated top to bottom; the first one found is used */
  rootMarkers: string[];   // ["go.work", "go.mod", ".git"]

  /** The default value passed as initialize's initializationOptions */
  initializationOptions?: Record<string, unknown>;

  /** Conformance to this spec */
  conformance?: {
    declarationRange?: boolean;
    serverState?: boolean;
  };

  /**
   * The correction rules applied when not conformant. The proxy
   * applies them. A correction that cannot be written here (complex
   * language-specific logic) lives in the proxy's own code.
   */
  shims?: Shim[];
}

interface Shim {
  /** Which symbol kinds it applies to */
  symbolKinds: number[];   // values of SymbolKind
  /** Extends the start of range forward to the given pattern */
  extendStartToPattern?: string;   // a regular expression. e.g. "^\\s*(pub\\s+)?type\\s+"
  /** Includes decorators/attributes */
  includeLeadingAttributes?: boolean;
}
```

### 3.3 Conformance Tests

- JSON Schema validation of the manifest
- `detect` → `launch` → `initialize` → `shutdown` succeeds
- The correct root is chosen following `rootMarkers` (a monorepo fixture)

---

## 4. Reference Proxy

To make this spec usable without waiting for upstream servers to conform, a proxy with the following behavior is provided as a reference implementation.

### 4.1 Behavior

```text
Agent ──[LSP + declaration range + server state]── Proxy ──[LSP]── Language Server
```

- It looks like a conformant server to the client
- Transparent if upstream conforms to declaration range. If not, it synthesizes `declarationRange` using the manifest's `shims` and language-specific correction code
- Transparent if upstream conforms to server state. If not, it infers `ServerState` from watching `$/progress`, known initialization-complete patterns, and process monitoring (never from elapsed time; [spec/server-state.md](spec/server-state.md), chapter 6, item 6)
- Launches upstream following the manifest

### 4.2 Design Constraints

- A single binary, no external runtime dependency (Rust is assumed)
- Adding a language is possible with just adding a manifest. Language-specific code is limited to corrections that `shims` cannot express
- Process lifetime follows the parent. If the parent dies, it dies too (`PR_SET_PDEATHSIG` / Job Object / `kqueue`)
- The cache is capped. It is never unbounded
- No incidental features like a dashboard

### 4.3 Initial Language Support

Five: gopls, rust-analyzer, typescript-language-server, pyright, and clangd. A conformance-test fixture is prepared for each.

---

## 5. Path to Upstream Proposals

The current strategy (order, rules, the list of submissions) is [upstream-submissions.md](upstream-submissions.md) (Japanese). In brief, three points.

1. Stand up consumers (Claude Code, lsp-det, Serena) first, then ask the server's upstream
2. Fix a defect the other side already feels first; bring the protocol proposal later, as a generalization of it. If upstream already has a different answer to the same problem, build on top of it
3. A proposal to LSP itself (microsoft/language-server-protocol) is filed without waiting for upstream adoption, on the grounds of a measurement of vocabulary fragmentation ([research/readiness-vocabulary-corpus.md](research/readiness-vocabulary-corpus.md)). It connects to the existing issue microsoft/language-server-protocol#511's thread as "a re-proposal from an agent use case". Declaration range and the launch manifest are frozen and out of scope for this path

---

## 6. Prior Art

Before writing the spec, look at how each implementation solves the same problem. Below is the result of the initial investigation, and items to check further.

### 6.1 LSP Itself

| Item                                                                    | Content                                                                                                       | Relation to this spec                                                                           |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| The spec text of `DocumentSymbol.range`                                 | "the range enclosing this symbol not including leading/trailing whitespace but everything else like comments" | What declaration range tightens. Not followed because there is no conformance test              |
| `selectionRange`                                                        | The range of the name only. Split from `range` in 3.10                                                        | `declarationRange`'s `body` / `full` extend the same idea                                       |
| `$/progress` / `window/workDoneProgress`                                | Added in 3.15. Optional. The token and title are free-form                                                    | The predecessor of server state. Cannot express "what finishing means for what can be answered" |
| How `workspace/didChangeWatchedFiles` and the like declare a capability | An optional feature is declared through a capability                                                          | Declaration range and server state follow this same style of declaration                        |

#### New Features in 3.18 (Conflict Check Done, as of 2026-08-27)

Items tagged `@since 3.18.0` in 3.18 are as follows. **None of them overlaps with this spec's three extensions.**

- `textDocument/inlineCompletion` (inline completion)
- `workspace/textDocumentContent` (providing the content of a virtual document)
- `workspace/foldingRange/refresh`
- `SnippetTextEdit` (a snippet-form edit)
- Relative-pattern support in `DocumentFilter`
- `MarkupContent` support for `Diagnostic.message`
- `WorkspaceEditMetadata`
- `RegularExpressionEngineKind`
- Additional `languageId`s (D, Pascal, and the like)
- `Command.tooltip`

The direction centers on "enriching the editor UI"; nothing about determinism or lifecycle for agents is included.

#### Related Existing Issues (microsoft/language-server-protocol)

| Issue                                                       | Content                                                                                                                                                                                                                       | Status            | Relation to this spec                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| ----------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| #511 "Discussion: LSP-server readiness indicator" (2018-06) | The Java extension and OmniSharp each show a readiness state in their own status bar. Proposes standardizing it and presents `window/showStatus` (`type` / `actions` / `message` / `shortMessage`) as a sample implementation | **Open, Backlog** | The closest prior proposal to server state. But its purpose is a human-facing UI (a status-bar display) and it does not include a machine-readable "which method can return a complete result". Server state cites #511 while differentiating itself as "a state for deciding, not for displaying". It has sat untouched for 8 years because, for a UI purpose, it was enough for each editor to do its own thing; a new motive — agent use — needs to be shown |
| #54 "Clarification for the Indexing workflow" (2016-08)     | There is nothing in the spec about client-server communication while the index is being built                                                                                                                                 | Old               | The same problem awareness as server state. A gap recognized from the very beginning                                                                                                                                                                                                                                                                                                                                                                            |
| #312 "filtering documentSymbol operation" (2017)            | A proposal to narrow `documentSymbol` by a specified range                                                                                                                                                                    | —                 | A different matter from declaration range. Unrelated                                                                                                                                                                                                                                                                                                                                                                                                            |

A proposal equivalent to `declarationRange` (a full declaration-range spec including keywords and decorators) was not found even on a further search. What turned up as related was #613 (`document/extendSelection`, the origin of the later `selectionRange`) and #1270 (how `selectionRange` handles a null response), and neither is a proposal to "make the meaning of the range stricter". The spec text of `DocumentSymbol.range` ("not including leading/trailing whitespace but everything else like comments") is reused from `LocationLink.targetRange` and has never been revised since 3.14. **Declaration range can be filed as a new proposal.**

No proposal equivalent to the launch manifest exists in LSP itself. LSP states explicitly that "launching the server is the client's responsibility" and takes the position that it is out of the spec's scope. The launch manifest should be filed not as a proposal to LSP itself but as a separate spec (a registry).

#### Base Protocol 0.9 (Upcoming) — Checked

A spec that carves the "language-server-independent common part" out of LSP. Includes capability exchange, `initialize` / `initialized` / `shutdown` / `exit`, the structure of requests / notifications, cancel, progress, `window/showMessage`, and the like. Its purpose is to let protocols other than LSP (debugging and the like) use the same foundation.

Relation to the extensions:

- Declaration range: unrelated. A matter of `DocumentSymbol` on the LSP side
- Server state: it may make more sense to place it on the Base Protocol side. "Whether the server is alive and can fully answer a request" is not a concept limited to language servers, and it can be proposed as part of the Base Protocol's lifecycle (`initialize` through `shutdown`). Both axes, `health` and `readiness`, are language-independent
- Launch manifest: Base Protocol also puts launching out of scope. It remains a separate spec

Base Protocol is still at 0.9, and holds a reserved list of capability names carved out of existing issues. Whether server state's capability name (`serverStateProvider`) collides with a future reservation is checked at the time of proposal.

### 6.2 The Reality of "Readiness" in Each Language Server

The result of reading what solidlsp's language-specific classes actually wait for. This is primary evidence for the need for server state, and this table is used as-is when making the upstream proposal.

| Server                     | How readiness is judged (solidlsp's implementation)                                                                                                                                                                                                                                                                                       | Assessment                                                                                                                                                                    |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| rust-analyzer              | Waits for `quiescent: true` in the custom extension `experimental/serverStatus`. With a timeout                                                                                                                                                                                                                                           | The most machine-readable. The existing implementation closest to server state                                                                                                |
| jdtls                      | Waits for two stages in the custom notification `language/status`: `type: "ServiceReady"` and `type: "ProjectStatus", message: "OK"`                                                                                                                                                                                                      | Machine-readable but entirely its own format. Distinguishing "service readiness" from "project readiness" corresponds to server state's `initializing` / `indexing` / `ready` |
| pyright                    | Judged by matching the body of `window/logMessage` against the regular expression `Found \d+ source files?`. A code comment says "pyright cannot be trusted, and there is no better way". `experimental/serverStatus` is also watched, as a supplement                                                                                    | **A machine reading a log meant for humans.** A textbook example of there being no spec                                                                                       |
| typescript-language-server | Waits for the `end` of `$/progress`, but since the same `end` also comes when tsserver crashes and the two cannot be told apart, separately watches `window/logMessage` for an abnormal-exit pattern. Has three timeouts: a "grace period before indexing starts" (5 seconds), "indexing complete" (30 seconds), and "ready" (10 seconds) | A real example of `$/progress` being unable to distinguish "finished" from "aborted"                                                                                          |
| clangd                     | No judgment. A code comment says "clangd does not send a meaningful notification on readiness" and "this defeats the purpose of the event"                                                                                                                                                                                                | **Cannot be judged**                                                                                                                                                          |
| gopls                      | No judgment. A comment says "usually ready right after initialize"                                                                                                                                                                                                                                                                        | In fact, an empty response comes back for a large module (Serena issue #890 and the like), but it is ignored for lack of a way to judge it                                    |

**Summary**: of the 5+1 servers, only rust-analyzer and jdtls have a machine-readable readiness notification, and even those two differ in format. The rest are either the ambiguity of `$/progress`, a regex over a log, or giving up. This is the real damage from "LSP has no readiness standard", and it is the grounds for server state.

**Priority order for upstream proposals**: rust-analyzer (already has the concept) → jdtls (same, only a format change) → pyright (made by Microsoft; showing the current log-dependent state makes the motive clear) → gopls → clangd → tsserver.

### 6.3 rust-analyzer: `experimental/serverStatus`

rust-analyzer sends the `experimental/serverStatus` notification as a custom extension, and conveys that indexing is complete with `quiescent: true`.

- The existing implementation closest to server state. Three fields: `health` / `quiescent` / `message`
- pyright and clangd are also watched by solidlsp under the same notification name (there is a possibility that an implementation modeled on rust-analyzer is spreading to other servers too. **To investigate**: whether pyright / clangd actually send `experimental/serverStatus`, or solidlsp is just watching it just in case)
- Positioning server state as the successor of `experimental/serverStatus` — keeping `health` and expanding `quiescent` into readiness's three values — minimizes rust-analyzer's migration cost

### 6.4 Editor-Side Implementations

#### VS Code

- `vscode-languageclient` is the LSP client. Each extension specifies how to launch through `ServerOptions`
- It is customary for the extension to **bundle** the language server. In exchange for pinning the version, it cannot be used from outside
- Range correction stays closed inside each extension
- **To investigate**: whether `vscode-languageclient` has common handling for range or readiness

#### Neovim: nvim-lspconfig + Mason

- nvim-lspconfig: declares `cmd`, `filetypes`, `root_markers`, `settings` per language in a Lua table. The de facto standard schema for launching and initialization options
- Mason registry: writes `source.id` in `package.yaml` in purl form (`pkg:npm/...`, `pkg:github/...`, `pkg:golang/...`) and specifies the executable with `bin`. The de facto standard for how to obtain it. Also has `extra_packages`, a build step, and `schemas.lsp` (a URL for the settings schema)
- The launch manifest is close to a combination of these two. Reusing the vocabulary of purl and nvim-lspconfig, rather than inventing a new format, is more likely to be adopted
- **To investigate**: how the lspconfig format changed with Neovim 0.11's `vim.lsp.config`

#### Zed

- Separates the `LspAdapter` trait (initialization parameters, environment variables, completion labels) from the `LspInstaller` trait (detecting, obtaining, and caching the binary). `check_if_user_installed()` searches PATH and inside the toolchain first, and fetches it on its own if not found. Verified with SHA-256
- The launch manifest's `install.detect` → `sources` order is the same design as Zed's
- Since it is a Rust implementation, much of it can be referenced for the reference proxy's design
- **To investigate**: how Zed's rust-analyzer adapter handles `serverStatus`, and whether it has range correction

#### Helix

- Declares `command`, `args`, `roots`, `config` in `languages.toml`. The same shape as nvim-lspconfig
- No automatic install. Its policy is "the user installs it", the same trade-off as Claude Code's LSP plugin
- **To investigate**: none (the design is simple, so it is a reference only)

#### Emacs: lsp-mode / eglot

- lsp-mode has an automatic install mechanism, eglot does not. Both extremes of implementation exist in the same editor
- **To investigate**: the declaration format of lsp-mode's `lsp-dependency` mechanism

#### JetBrains

- The native implementation is LSP-independent. It has had an LSP API since 2023, but it is supplementary
- Serena also has a path that uses JetBrains as a backend (not LSP)
- Out of this spec's scope, but useful as a reference for range definitions, as "an IDE-internal model stricter than LSP"

### 6.5 Serena / solidlsp

Codebase: oraios/serena `src/solidlsp/` (about 42,000 lines, of which about 27,000 lines are in 74 language-specific files)

**Structure**

- Each language subclasses the `SolidLanguageServer` base class. Inherits multilspy's design
- Hooks the language-specific classes override (by frequency):
  - `__init__` (78), `_start_server` (75), `_create_base_initialize_params` (75) — launching and initialization. The part the launch manifest absorbs
  - `is_ignored_dirname` (56) — ignored directories. **Under consideration for addition** to the launch manifest, as an item that should be in it
  - `_create_dependency_provider` (44), `_setup_runtime_dependencies` (16) — obtaining it. The launch manifest
  - `_get_wait_time_for_cross_file_referencing` (14) — holds a **fixed number of seconds to wait**, per language, as a substitute for readiness. What server state removes
  - `_document_symbols_cache_fingerprint` (10), `_normalize_symbol_name` (9), `request_document_symbols` (8) — range and symbol correction. What declaration range removes
  - `request_text_document_diagnostics` (8) — absorbing the difference between pull and push diagnostics
- `dependency_provider.py` — an abstraction over how to obtain it. Equivalent to Mason's purl, but its own format

**Value to this spec**

- A measured record, for 74 languages, of "where it departs from the standard". Reading what each class overrides shows which cases should go into a conformance-test fixture
- In particular, the value of `_get_wait_time_for_cross_file_referencing` and the override of `request_document_symbols` can be used in the upstream proposal as evidence showing the need for declaration range and server state
- **To investigate**: classify what the 74 classes override into three groups — "expressible in the manifest", "expressible with `shims`", "needs code". This becomes the design grounds for the launch manifest's `Shim`

**What is not carried over**

- A synchronous API, a Python dependency, a dashboard, an unbounded cache, process lifetime management (Issues #944, #1277, #1281, #1367, #1387, #1488)

### 6.6 Prior Agent-Oriented Protocols

#### LSAP (lsp-client/LSAP)

- 38 stars, v0.2.0 (2026-01), MIT, mainly Python
- Positions itself as "LSP is atomic operations, LSAP is cognitive capability", and defines composite queries (locate + references + context extraction in one request) with JSON Schema. Even standardizes the Markdown rendering template
- `rename` has two stages, preview → execute
- Corresponds to a layer above this spec. Not a competitor — a party we want to depend on this spec
- **To investigate**: how LSAP handles range and readiness internally. If it has its own correction, show that this spec can remove it

#### LSAI (LadislavSopko/lsai-protocol)

- 3 stars, 2026-05, CC BY-NC 4.0 (a commercial implementation needs a different license)
- 14 semantic tools (including composites like `impact`, `context`). Close to this spec in that it **includes a fallback strategy in the spec** for when upstream LSP lacks a feature
- Its reference implementation Zerox.Lsai covers 10 languages, verified end to end
- It cannot become a standard because of its license, but its classification of fallback strategies is a useful reference
- **To investigate**: read the fallback definitions in spec/LSAI-v1.4.md for a classification that can be reused in the launch manifest's `Shim`

#### Other MCP Bridges

- claude-code-lsps (Piebald-AI), boostvolt/claude-code-lsps: a collection of plugins in Claude Code's `.lsp.json` form. A real example of a declaration format
- cclsp (ktnyt), mcpls (bug-ops): thin LSP → MCP bridges
- code-yeongyu/codex-lsp: for Codex. Designed to return diagnostics through a post-edit hook
- **To investigate**: how each bridge handles range and readiness (probably unsupported; the fact of being unsupported is itself grounds for this spec)

### 6.7 List of Investigation Tasks

Priority order:

1. [x] Check whether LSP 3.18 has a conflicting proposal → none (see 6.1). Added issue #511 as a citation target as a supplement
1b. [x] Check the content of Base Protocol 0.9 → added the option of placing server state on the Base Protocol side (6.1)
1c. [x] A prior proposal equivalent to `declarationRange` → none. Can be filed as a new proposal (6.1)
2. [x] Check whether gopls / tsserver / pyright / clangd / jdtls have a readiness notification → recorded as a table in 6.2
2b. [ ] Check, from each repository's source, whether pyright / clangd actually send `experimental/serverStatus`
3. [ ] Classify what solidlsp's 74 classes override (manifest / shims / code)
4. [ ] Bring the vocabulary of Neovim 0.11's `vim.lsp.config` and Mason's `package.yaml` into the launch manifest
5. [ ] Bring Zed's `LspInstaller` design into the reference proxy
6. [ ] Check where range and readiness correction live in LSAP's internal implementation
7. [ ] Read LSAI v1.4's fallback classification
8. [ ] For 5 languages, collect `documentSymbol.range`'s actual return values with a fixture and table the divergence from the spec text (evidence for the upstream proposal)

## 7. Open Questions

- A completion notification for diagnostics. The problem that there is no way to know "when have the diagnostics for this edit all arrived". Reserved as a future extension of server state (`phases`), but the concrete vocabulary is not yet designed
- Declaring ignored directories (the measured top-tier correction, overridden by 56 classes in solidlsp). On hold along with the freeze on the launch manifest
- Whether `declarationRange.full` should include a trailing comment (one on the same line)
- Whether `readiness` needs to be returned per file rather than per workspace (a large monorepo)
- Who hosts the manifest registry. A Git repository is enough for now
- Writing down the division of roles with LSAP
- The name has already been changed to "Deterministic Extensions". Whether editor-side implementers will read it as their own concern too is checked by having a few people read it before the proposal

---

## Appendix: Relation to the Existing Specification

| This spec                              | Related existing LSP item                 | Relation                                                                       |
| -------------------------------------- | ----------------------------------------- | ------------------------------------------------------------------------------ |
| Declaration range (`declarationRange`) | `DocumentSymbol.range` / `selectionRange` | Added. Existing ones unchanged                                                 |
| Server state (`serverState`)           | `$/progress`, `experimental/serverStatus` | Added. Existing ones may still be used alongside, as supplementary information |
| Launch manifest (`manifest`)           | None (out of spec)                        | New                                                                            |
