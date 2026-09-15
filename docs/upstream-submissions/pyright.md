# pyright への提出

[../upstream-submissions.md](../upstream-submissions.md) の戦略に沿って pyright に出した文面と、提出後の反応・再測定の記録。出した文面は一字も変えずに残す。現在地（何を出し、何が返ってきたか）は同文書の一覧を見る。

## enhancement request（`InitializeResult.serverInfo`）

feature_request の欄に合わせる。Title: `Report serverInfo (name and version) in the initialize result`

````markdown
**Is your feature request related to a problem? Please describe.**

A client, or a proxy in front of the server, cannot tell from the LSP handshake which server it is talking to or which version: pyright's `InitializeResult` has no `serverInfo`. The name and version appear only in the startup `window/logMessage` ("Pyright language server 1.1.412 starting"), which a client has to parse. `serverInfo` has been a standard field since LSP 3.15, and basedpyright already returns it.

**Describe the solution you'd like**

`InitializeResult.serverInfo = { name: productName, version }` in `LanguageServerBase.initialize` (`packages/pyright-internal/src/languageServerBase.ts`). It is a four-line change; I have it ready and can open a PR if this is acceptable.

**Additional context**

I use it in lsp-det (https://github.com/tagawa0525/lsp-det), a proxy that selects a per-server readiness mapping by the server's name and version. Today it has to read pyright's startup log for that.
````
