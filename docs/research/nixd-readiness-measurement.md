# nixd の readiness の実測（M25）

ADR 0021 決定 C の M25。コーパスは nixd を「`onInitialize` の応答の後に nixpkgs の評価と NixOS のオプションの評価を独立した `$/progress` で報告する。2 本が並行する」とソースから確認済みだったが、未実測だった。nixpkgs（固定中の rev）を `NIX_PATH` に置いた被験体で測り、**信号は 2 本並行し、索引に依る要求はサーバー自身が評価の完了まで待たせる**と確かめた。token は乱数の整数で、title "evaluating …" だけが固定の語彙である。

一方、7.0 の横断要求のうち索引に依るのは nixpkgs の属性と NixOS のオプションへの `definition` だけで、`references` は要求のあった文書の中に閉じる（ADR 0021 決定 D）。health の信号はなく、評価の失敗は end の message が「evaluated …」のまま stderr にしか出ず、失敗した worker への次の要求で nixd 自身が SIGPIPE で落ちる。

## 方法

- nixpkgs の nixd 2.9.2（`serverInfo` は `{"name": "nixd", "version": "2.9.2"}`。`flake.nix` の `servers`）。`NIX_PATH=nixpkgs=<flake.lock の nixpkgs の store path>`。2026-09-08
- 被験体: `default.nix`（`{ pkgs ? import <nixpkgs> { } }:` の下で `pkgs.hello`、`with pkgs; vim`、`import ./tool.nix`）、`tool.nix`（`pkgs.ripgrep`）、`module.nix`（NixOS のモジュール。`services.openssh.enable = true;`）
- 道具: scratchpad の `lsp_probe.py`。クライアントは `window.workDoneProgress` を宣言し、`window/workDoneProgress/create` と `workspace/configuration`（既定は `{}`）に答える
- 走行: (1) 起動して `default.nix` を開き `pkgs.hello` の `definition` を 1 秒ごとに送る、(2) `module.nix` の `enable` の `definition` と `hover`、(3) `NIX_PATH` を存在しないパスにする、(4) 起動の 4 秒後に nixpkgs の worker を `kill -9`、(5) `tool.nix` を書き換えて `didChangeWatchedFiles` Changed、(6) `workspace/configuration` に `nixpkgs.expr` と `options.nixos.expr` を答え、`ready` の後に `didChangeConfiguration`、(7) `references`（`pkgs` の仮引数）を `tool.nix` も開いた状態で、(8) 開発環境の `NIX_PATH`（`nixpkgs=flake:nixpkgs`）のまま (1)
- 裏付けに `reference/nixd` を読んだ（`nixd/lib/Controller/LifeTime.cpp`、`Configuration.cpp`、`Definition.cpp`、`FindReferences.cpp`、`Support.cpp`、`nixd/lib/Eval/AttrSetProvider.cpp`、`Launch.cpp`、`nixd/lspserver/src/Connection.cpp`）

## 結果

### 語彙

| 信号                              | 内容                                                                                                                                                                                                                             |
| --------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `InitializeResult.serverInfo`     | `{"name": "nixd", "version": "2.9.2"}`。版が語彙に現れる                                                                                                                                                                         |
| `window/workDoneProgress/create`  | 評価のたびに 1 度。token は `rand()` の整数（走行のたびに同じ列: 1804289383、846930886、1681692777、…）                                                                                                                          |
| `$/progress`                      | begin（title "evaluating nixpkgs entries" / "evaluating nixos options"。設定で足した式は "evaluating " + 名前。percentage なし、report なし）→ end（message "evaluated …"）。`initialize` の応答の直後に 2 本が同時に begin する |
| `workspace/configuration`         | `initialize` の応答の直後に `{"section": "nixd"}` で 1 度、`didChangeConfiguration` のたびに 1 度。答えに `nixpkgs.expr` や `options.<名前>.expr` があれば、その式の評価が新しい token で走る                                    |
| `textDocument/publishDiagnostics` | `didOpen` / `didChange` の直後に同期で出る（構文と未使用の引数など。評価には依らない）                                                                                                                                           |
| `client/registerCapability`       | **なし**。`workspace/didChangeWatchedFiles` は登録せず、送ると stderr に "unhandled notification"（走行 5。`showMessage` は出ない）                                                                                              |

health の信号はない。`window/showMessage` も `window/logMessage` も使わず、ログは stderr にしか出ない。

### 起動と索引に依る要求（走行 1、2、8）

| 時刻   | 出来事                                                                                                                                                       |
| ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 0.011s | `initialize` の応答                                                                                                                                          |
| 0.012s | `create` + begin "evaluating nixpkgs entries"、`create` + begin "evaluating nixos options"、`workspace/configuration`。`didOpen` の直後に最初の `definition` |
| 0.105s | end "evaluated nixpkgs entries"。**その直後**に最初の `definition` の応答（nixpkgs の `hello/package.nix` の 1 箇所）                                        |
| 0.549s | end "evaluated nixos options"                                                                                                                                |

最初の `definition` は評価中に届き、nixpkgs の評価の完了まで待たされて完全な答えを返した。ソースのとおり、controller は worker への RPC（`attrset/attrpathInfo`）をセマフォで待ち（`Definition.cpp:179-190`）、worker は 1 本の同期ループで `evalExpr` を終えてから次の要求を読む（`Connection.cpp:318`）。NixOS のオプションへの `definition`（走行 2。`services.openssh.enable` の `enable` → nixpkgs の `sshd.nix`）と `hover`（型と説明）も同じで、オプションの評価の完了（0.354s）まで待って完全な答えを返した。`openssh` や `networking` の位置では 0 件で、nixd はオプションのパスの末尾でしか解決しない。

評価は速い（nixpkgs 0.1〜0.2 秒、オプション 0.35〜0.55 秒）。「索引に観測できる時間を要する」精度ではないが、要求を評価中に送れば待たされる順序は毎回観測できる。開発環境の `NIX_PATH`（`nixpkgs=flake:nixpkgs`。走行 8）でも同じに動く。

### 単一文書に閉じる `references`（走行 7）

`pkgs` の仮引数の `references` は `default.nix` の 3 箇所だけで、`tool.nix` を開いていても変わらない（`tool.nix` の `pkgs` は別の束縛）。ソースのとおり `references` は同じ文書の `uses()` を返す（`FindReferences.cpp:24-47`）。Nix の名前解決が文書ごとに閉じ、`import ./tool.nix` は辿らない。

### 失敗の見え方（走行 3、4、6）

- `NIX_PATH` が解決できないと、2 本の評価は 0.023s に end する。message は成功時と同じ「evaluated nixpkgs entries」「evaluated nixos options」で、失敗は stderr の `E[…] nixpkgs entries eval expr: -32001: file 'nixpkgs' was not found in the Nix search path` にしか出ない。1 秒後の次の `definition` で controller が死んだ worker に RPC を書き、**nixd 自身が SIGPIPE（返り値 -13）で終了**した
- worker を `kill -9` しても同じ。次の `definition` で nixd が SIGPIPE で終了する。クライアントから見える信号は終了だけで、lsp-det は上流の終了として扱う（既存の経路）
- 走行 6 では `didChangeConfiguration` を評価中に送った直後に `shutdown` / `exit` を送ると SIGSEGV（返り値 -11）で終了した。1 度の観測で、再現の条件は詰めていない

### 再評価（走行 6）

`workspace/configuration` に `nixpkgs.expr` と `options.nixos.expr` を答えると、既定の 2 本に加えて "evaluating nixpkgs entries" と "evaluating nixos" の 2 本が同じ 0.012s に begin し（4 本並行）、`ready` の後の `didChangeConfiguration` でさらに 2 本が begin した。end は 0.084s〜0.520s に入り混じり、全部が end した時点で未完了のトークンが 0 になる。`ready` → `indexing` → `ready` の遷移は 7.1 の 3 のとおり観測できる（引き金は `didChangeConfiguration`。`didChange` では評価は走らない）。

### 変更の取り込み

`didChange` は主ループで同期に解析され（`Support.cpp:23-58`）、後続の要求は新しい木を見る。ただし 7.3 のテストは横断（別のファイルからの問い合わせ）を要し、上のとおり nixd の横断要求は同じ文書に閉じるので、7.3 の 1〜4 はどれも構成できない。`didChangeWatchedFiles` は読まない（走行 5）。

## 写像（設計）と未決の点

- **識別**: `serverInfo.name` "nixd"。版は `serverInfo.version`
- **readiness**: `initializing` から、title が "evaluating " で始まる `$/progress` の begin で token を未完了に加えて `indexing`、その token の end で外し、未完了が 0 になったら `ready`。token は整数なので JSON の値のまま比べる。以後の begin（`didChangeConfiguration` の再評価）で `indexing`、全部の end で `ready`。他の title の token は読まない
- **先読み**: しない。サーバーが要求を待たせるので、評価中に転送しても古い答えは返らない。lsp-det の保留は結果を変えず、`ready` の後に転送する順序を保証するだけ
- **health**: 信号がなく `unknown`。失敗の end は成功と区別できず、終了だけが見える
- **coverage / freshness**: 決定 E の答え（(b)）により、通した版（2.9.2）に `coverage: {scope: "document", incomplete: {}}` を宣言する。`freshness` は宣言しない: 7.3 は横断（別のファイルからの問い合わせ）を要し、文書に閉じるサーバーでは構成できない（`didChange` は LSP の順序の保証で足りる）
- **実サーバーの結合テスト**: 7.1（識別、`initializing` から 2 本の begin と end を経て `ready`、評価中に送った `definition` が `ready` の後に nixpkgs の `hello/package.nix` を指して返る）と 7.2 の 1（`ready` の後の `references` が文書内の結果に一致すること。`includeDeclaration: true` でも `pkgs` 仮引数自身の宣言位置は返らず、実際の使用箇所だけが返る）。`NIX_PATH` が要る（開発環境の `nixpkgs=flake:nixpkgs` で足りる）
- **上流に求めること**（`docs/upstream-submissions.md` の候補）: (1) 評価の失敗を end の message か `window/showMessage` で区別できるようにする（今は成功と同じ "evaluated …"）、(2) worker の死で nixd 自身が SIGPIPE で落ちないようにする（SIGPIPE を無視して RPC のエラーとして返す）、(3) `workspace/didChangeWatchedFiles` を登録なしで受けても stderr に出さず黙って無視する（既にエラーではないので優先は低い）

## コーパスへの反映

`readiness-vocabulary-corpus.md` の nixd の行を「確認済み」から「実測済み」に更新する。信号は 2 本並行の `$/progress`（title "evaluating …"、token は乱数の整数）、索引に依る要求はサーバー自身が待たせる、health の信号はなし、`references` は単一文書。
