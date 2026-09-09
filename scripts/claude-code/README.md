# Claude Code が言語サーバーに送るものを記録する

`tee-probe/` は Claude Code のプラグイン。言語サーバーの位置に `tee.py` を置き、クライアントが stdin に書いたものを時刻付きで記録してからサーバーに流す。ドッグフーディングの記録（`docs/research/claude-code-dogfooding.ja.md`）の「CC が送る通知の全数」「`initialize` の capability」「`shutdown` の `params`」は、この形のラッパーで取った。

```bash
cd /path/to/rust-project   # Cargo.toml のあるディレクトリ
claude -p "Use the LSP tool exactly once: findReferences for the symbol at src/lib.rs line 1 column 8. Print the tool result verbatim. Do not read any file and do not call any other tool." \
  --plugin-dir /path/to/lsp-det/scripts/claude-code/tee-probe \
  --debug-file /tmp/cc-debug.log --output-format json --allowedTools LSP --model sonnet
```

- 記録は `/tmp/claude-code-tee-rust-analyzer.log`（`.lsp.json` の第 2 引数）。`### 0.0054s` の行がクライアントの書き込みの時刻で、その後に原文の LSP メッセージが続く
- 他のサーバーを測るときは `.lsp.json` の項目を足す（`command` は `python3`、`args` は `${CLAUDE_PLUGIN_ROOT}/tee.py <ログ> <サーバーのコマンド>`。`${CLAUDE_PLUGIN_ROOT}` は Claude Code が展開する）
- lsp-det 経由を測るときは `args` を `["${CLAUDE_PLUGIN_ROOT}/tee.py", "<ログ>", "lsp-det", "--", "rust-analyzer"]` にすると、CC が lsp-det に書いたものが記録される。lsp-det が上流に書いたものを記録するなら `lsp-det -- python3 …/tee.py <ログ> rust-analyzer`
- `--debug-file` のログには CC 側の時刻（`Sending request 'textDocument/references - (1)'`、`Received response … in 2ms`）と、`shutdown` の失敗のような CC 自身のエラーが出る
- 入れ子の Claude Code から動かすときは `env -u CLAUDECODE` を付ける
