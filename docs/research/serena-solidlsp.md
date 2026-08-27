# serena solidlsp 調査報告: 言語別 readiness 処理・シンボル補正・起動前 CLI 実行

調査対象: `reference/serena/src/solidlsp/`(oraios/serena の浅い clone)。
以下、引用パスは `reference/serena/` からの相対パス、`:N` は行番号。

## 要約

- solidlsp は「LSP サーバーの準備完了」を統一的には扱っておらず、言語ごとに
  (a) サーバー固有通知、(b) `$/progress` トークンの排出(drain)、(c) `window/logMessage`
  の文字列/正規表現マッチ、(d) `publishDiagnostics` 初回到着、(e) 固定秒数 sleep、
  (f) initialize 応答のみ(即時 ready 扱い)の 6 類型を使い分けている。
  全実装が「タイムアウト後は proceed anyway」のフォールバックを持つ(例外: svelte は raise)。
- 基底クラスは「確実な初期化完了シグナルを持たない LS」向けに、初回クロスファイル参照要求の
  前に固定 sleep を入れる仕組みを持ち(既定 2 秒、`src/solidlsp/ls.py:1042-1048`,
  `:1628-1633`)、15 言語が `_get_wait_time_for_cross_file_referencing` を 0.0〜15.0 秒で
  上書きしている。
- シンボル補正は 12 言語で上書きされており、内容は「名前からの型注釈/宣言キーワード/
  arity の除去」「selectionRange バグの修正」「SymbolKind の再マップ」「コンパニオン
  サーバーへの委譲」の 4 系統。
- LSP プロセス起動前に CLI コマンド実行(subprocess)を要求する言語は 20 超。lsp-det が
  PATH 偽装で挟まる場合、**サーバーバイナリ自体への CLI 応答**が必要なのは
  gopls(`gopls version`)、rust-analyzer(`<path> --version`)、zls(`zls --version`)、
  sourcekit-lsp(`sourcekit-lsp -h`)、nixd(`nixd --version`)、
  verible-verilog-ls(`--version`)。ただし rust-analyzer は `rustup which rust-analyzer`
  を PATH より優先するため、PATH 偽装だけでは挟めない点に注意。

---

## 1. 言語別 readiness 処理の全数調査

### 1.1 基底クラスの仕組み

- `SolidLanguageServer._get_wait_time_for_cross_file_referencing()`: 既定 2 秒
  (`src/solidlsp/ls.py:1042-1048`)。docstring に「finished initializing シグナルが
  信頼できない LS 向け。未初期化だと `request_references` が同一ファイル内の参照しか
  返さないことがある」と明記。
- この待ちは **最初のファイルを open した後** に一度だけ適用される
  (`_wait_for_cross_file_references_if_needed`, `src/solidlsp/ls.py:1628-1633`)。
- `_pre_open_for_cross_file_references()`(`src/solidlsp/ls.py:1620-1626`): didOpen 前に
  indexing 追跡イベントをクリアして「pre-arm」するためのフック(TS/svelte 系が利用)。

### 1.2 言語 × 手段 × 値 の一覧

手段の凡例:
**通知** = サーバー固有 LSP 通知 / **progress** = `$/progress` トークン drain /
**ログ** = `window/logMessage` の文字列・正規表現 / **diag** = `publishDiagnostics` 到着 /
**sleep** = 固定秒数 / **即時** = initialize 応答後に無条件 ready。

| 言語 (クラスファイル) | 手段 | 監視対象 / 具体値 | 起動時タイムアウトとフォールバック | 根拠 |
| --- | --- | --- | --- | --- |
| rust-analyzer | 通知 | `experimental/serverStatus` の `quiescent: true` | 120 秒待ち → proceed anyway | `language_servers/rust_analyzer.py:742-744,779-786` |
| gopls | 即時 | なし(「typically ready immediately」) | — | `language_servers/gopls.py:327-328` |
| Java (eclipse_jdtls) | 通知 | `language/status` `type=ServiceReady` + ProjectStatus | ServiceReady は無期限 wait、project ready は 20 秒 → proceed | `language_servers/eclipse_jdtls.py:1307-1313,1348-1359` |
| C# (Roslyn, csharp) | 通知 | `workspace/projectInitializationComplete` | 30 秒 → proceed | `language_servers/csharp_language_server.py:644-651,705-708` |
| C# (omnisharp) | 通知 | `experimental/serverStatus` `quiescent` を event に set するのみ(起動時に wait しない) | — | `language_servers/omnisharp.py:267-269` |
| TypeScript | 通知 + progress | `$/typescriptVersion` 受信で ready、`$/progress` トークン drain で indexing 完了 | ready 10 秒(`SERVER_READY_TIMEOUT`)、indexing 30 秒(`INDEXING_PROGRESS_TIMEOUT`)→ proceed(基底実装) | `language_servers/typescript_language_server.py:103-106,433-440,442-483,512-528` |
| Svelte(TS コンパニオン) | progress | コンパニオン TS サーバーの indexing drain。**timeout で例外を投げる strict 実装** | ready/indexing timeout で `TimeoutError` raise | `language_servers/svelte_language_server.py:155-161,363-396` |
| Vue | ログ + コンパニオン | logMessage に "initialized"/"ready";TS コンパニオンは 5 秒 | Vue 本体 3 秒(`VUE_SERVER_READY_TIMEOUT`)、TS 5 秒 → proceed | `language_servers/vue_language_server.py:167-168,676-684,763-768,814-817` |
| Angular | 通知 | `angular/projectLoadingFinish`;コンパニオン TS/HTML サーバー各 10 秒 | 本体 10 秒(`NG_SERVER_READY_TIMEOUT`)→ proceed | `language_servers/angular_language_server.py:216-218,550-552,577-585` |
| Kotlin | 通知 or progress | IntelliJ 版: `intellij/ready-for-test`;新 KLS: `workDoneProgress/create` で clear → progress drain;旧 0.253.x: 同期初期化で即時 | 120 秒(`_INDEXING_TIMEOUT`)→ proceed | `language_servers/kotlin_language_server.py:501-533,589-600` |
| Scala (Metals) | progress + 静穏期間 | `MetalsProgressTracker.wait_until_idle`: timeout 180 秒、start_grace 15 秒、quiet_period 3 秒(トークンが空でも 3 秒静穏を確認) | 設定で上書き可 | `language_servers/scala_language_server.py:36-38,102-207,404-419` |
| Python (pyright) | ログ regex + 独自 progress | regex `Found \d+ source files?` / `pyright/beginProgress`・`reportProgress`・`endProgress` | 60 秒(`_TIMEOUT_FOR_INITIAL_ANALYSIS`)→ proceed | `language_servers/pyright_server.py:27,134-177,237` |
| Python (basedpyright) | ログ regex + 独自 progress | pyright と同一(regex・通知名を継承) | 60 秒 → proceed | `language_servers/basedpyright_server.py:24,113-172,193` |
| Python (pyrefly) | progress | `$/progress` drain → `_indexing_complete`;再 index 中のキャンセルはリトライで吸収 | 30 秒 → proceed | `language_servers/pyrefly_server.py:245-256,293-296` |
| Python (jedi) | 通知 | `experimental/serverStatus` `quiescent` → `completions_available`(補完可否のみ) | — | `language_servers/jedi_server.py:143-153` |
| clangd | 即時(通知ハンドラのみ) | `quiescent` ハンドラは登録するが initialize 直後に無条件 set(「TODO This defeats the purpose」) | — | `language_servers/clangd_language_server.py:375-377,411-414` |
| ccls | 即時 | initialized 直後に set | — | `language_servers/ccls_language_server.py:143-151` |
| Ruby (solargraph) | 通知 | `language/status` `type=ServiceReady`, `message="Service is ready."` | 60 秒 → proceed | `language_servers/solargraph.py:302-306,344-349` |
| Ruby (ruby-lsp) | sleep(基底) | クロスファイル待ち 0.5 秒に短縮 | — | `language_servers/ruby_lsp.py:75-82` |
| OCaml | 通知 + ログ | `language/status` ServiceReady / logMessage "initialization done"。ただし initialized 直後にも無条件 set | — | `language_servers/ocaml_lsp_server.py:386-396,420-422` |
| Elixir (expert) | progress | `$/progress` の "Building <project>" begin→end をプロジェクトビルド完了とみなす | 300 秒 → proceed | `language_servers/elixir_tools/elixir_tools.py:349-357,409-417` |
| Erlang | ログ + progress + sleep | logMessage に "compilation finished"/"indexing complete" 等、`$/progress` end の "initialized"/"ready"/"complete";その後 settling sleep(CI 15 秒 / ローカル 5 秒) | timeout 後も proceed | `language_servers/erlang_language_server.py:139-170,212-225` |
| Elm | diag | 初回 `publishDiagnostics` 到着 = workspace scan 完了 | 30 秒 → proceed | `language_servers/elm_language_server.py:174-214` |
| Haxe | progress + diag | `_server_ready` は **最初から set**(コンパイルキャッシュ有効時)。progress トークン発生で clear → drain。診断到着でトークン空なら set | 60 秒(`_COMPILATION_TIMEOUT`) | `language_servers/haxe_language_server.py:53,65-66,306-314,353-359,385` |
| Gleam | progress | 初回 `$/progress` begin を 10 秒待ち(来なければ依存解決済みで ready)、来たら idle まで 180 秒 | proceed | `language_servers/gleam_language_server.py:32-33,214-230` |
| Nextflow | progress + 明示同期 | `initialize` トークンの progress drain(180 秒 `_WORKSPACE_SCAN_TIMEOUT`)。references 要求時は都度サーバーと明示同期するため blind wait 0.0 秒 | proceed | `language_servers/nextflow_language_server.py:61,243-269,295-300,347-349` |
| Solidity | 独自通知(カウント) | `custom/file-indexed` の受信数 == プロジェクト内 .sol ファイル数;`custom/validation-job-status` も監視 | 60 秒、timeout 時さらに `sleep(30)` | `language_servers/solidity_language_server.py:245-270,288-298` |
| AL | 独自リクエスト(ポーリング) | `al/hasProjectClosureLoadedRequest` を 0.5 秒間隔でポーリング | 3 秒(`_wait_for_project_load`) | `language_servers/al_language_server.py:666-669,895-957` |
| MATLAB | ログ | logMessage に "mvm attach success" or "adding workspace folder"(小文字比較) | 60 秒 → proceed | `language_servers/matlab_language_server.py:435-441,480-484` |
| Bash | ログ | logMessage に "Analyzing" or "analysis complete" | 3 秒 → proceed | `language_servers/bash_language_server.py:257-263,291-297` |
| Pascal | ログ | logMessage に "initialized" or "ready" | 5 秒 → proceed | `language_servers/pascal_server.py:892-898,937-942` |
| Luau | ログ | logMessage に "workspace ready" or "initialized" | 5 秒 → proceed | `language_servers/luau_lsp.py:337-342,368-374` |
| PowerShell (PSES) | 動的登録 + ログ | `client/registerCapability` で documentSymbol 登録を検知、または logMessage に "started"/"ready" | 10 秒 → proceed | `language_servers/powershell_language_server.py:336-353,381-385` |
| mSL | ログ | **任意の** logMessage 受信で ready | 2 秒 → proceed | `language_servers/msl_language_server.py:74-76,94-97` |
| Crystal (crystalline) | sleep | 初期化から最低 10 秒(`_MIN_COMPILATION_DELAY`)経過するまで definition 要求を遅延 | — | `language_servers/crystal_language_server.py:16-19,67-81` |
| Swift (sourcekit-lsp) | sleep | 初回 references 前に経過時間ベースで sleep(ローカル 5 秒 / CI 15 秒)+ 追加 `time.sleep(5)` | — | `language_servers/sourcekit_lsp.py:343-368` |
| Haskell (HLS) | sleep | initialize 後 `time.sleep(5)` | — | `language_servers/haskell_language_server.py:364-367` |
| Perl | sleep | settling 0.5 秒 | — | `language_servers/perl_language_server.py:240-245` |
| PHP (intelephense) | sleep(要求毎) | references/definition 前に `sleep(1)`(TODO コメントで原因不明と明記) | — | `language_servers/intelephense.py:209-226` |
| PHP (phpantom) | sleep | `sleep(1)` ×2(クロスファイル index 更新待ち) | — | `language_servers/phpantom.py:249-256` |
| Clojure (clojure-lsp) | 即時(+通知ハンドラ) | `quiescent` ハンドラ登録済みだが initialized 直後に set | — | `language_servers/clojure_lsp.py:377-379,406-408` |
| Ada (ALS) | 即時 | capabilities assert 後 set | — | `language_servers/ada_language_server.py:209-217` |
| BSL | 即時 | 同上 | — | `language_servers/bsl_language_server.py:209-217` |
| CUE | 即時 | 「ready immediately after initialized」 | — | `language_servers/cue_language_server.py:257-259` |
| その他即時系 | 即時 | terraform-ls / zls / nixd / lua-ls / marksman / regal / YAML / JSON / deno / phpactor / groovy / godot(既存エディタへ TCP 接続) / julia / qml / taplo / texlab / ty / HLSL / wolfram / ansible / dart / fortran / R / systemverilog / some-sass / vscode-html 等。initialize 応答のみで ready 扱い | — | 例: `language_servers/terraform_ls.py:273`, `language_servers/zls.py:203-204`, `language_servers/nixd_ls.py:426`, `language_servers/marksman.py:259`, `language_servers/godot_language_server.py:3-37` |

### 1.3 `_get_wait_time_for_cross_file_referencing` の上書き値(基底: 2 秒)

| 言語 | 上書き値 (秒) | 根拠 |
| --- | --- | --- |
| F# | 15.0 | `language_servers/fsharp_language_server.py:430-434` |
| Elixir | 10.0 | `language_servers/elixir_tools/elixir_tools.py:40-41` |
| Lean 4 | 10.0 | `language_servers/lean4_language_server.py:159-161` |
| Angular | 5.0 | `language_servers/angular_language_server.py:602-603` |
| Haxe | 5 | `language_servers/haxe_language_server.py:407-408` |
| R | 5.0 | `language_servers/r_language_server.py:19-20` |
| Vue | 5.0 | `language_servers/vue_language_server.py:851-852` |
| Fortran | 3.0 | `language_servers/fortran_language_server.py:31-32` |
| Solidity | 3.0 | `language_servers/solidity_language_server.py:229-231` |
| C# (Roslyn) | 2 | `language_servers/csharp_language_server.py:759-760` |
| Elm | 1.0 | `language_servers/elm_language_server.py:217-218` |
| Kotlin | 1.0(起動時に indexing 待ち済みのための安全バッファ) | `language_servers/kotlin_language_server.py:605-607` |
| VTS | 1 | `language_servers/vts_language_server.py:272-273` |
| Ruby (ruby-lsp) | 0.5 | `language_servers/ruby_lsp.py:75-82` |
| Nextflow | 0.0(references 要求で明示同期するため) | `language_servers/nextflow_language_server.py:347-349` |

---

## 2. 範囲・シンボル補正の上書き

| 言語 | 上書きメソッド | 補正内容 | 根拠 |
| --- | --- | --- | --- |
| C# (Roslyn) | `_normalize_symbol_name` | Roslyn 5.5.0+ が返す型注釈付き名(`Name : string`, `Add(int, int) : int`)をベース名に正規化。元の名前を位置キーでキャッシュし、型情報を `detail` に格納 | `language_servers/csharp_language_server.py:262-290` |
| AL | `request_document_symbols` / `_normalize_symbol_name` / `request_full_symbol_tree` | `Table 50000 "X"` 形式からオブジェクト種別・ID を除去、メソッドの引数括弧・`action` 接頭辞・フィールドの `: 型` を除去。元名をキャッシュ。パス区切りも正規化 | `language_servers/al_language_server.py:703,994-1037` |
| Erlang | `_normalize_symbol_name` | `name/arity`(例 `create_user/2`)の `/` が Serena の name path 区切りと衝突するため `ARITY_SEPARATOR` に置換(arity は関数の同一性の一部なので保持) | `language_servers/erlang_language_server.py:66-78` |
| PowerShell | `_normalize_symbol_name` | `class X {`→`X`、メソッドシグネチャ→メソッド名、`function f(...)`→`f` | `language_servers/powershell_language_server.py:261-277` |
| Swift (sourcekit) | `_normalize_symbol_name` | Function/Method/Constructor の名前から `(...)` 引数部を除去 | `language_servers/sourcekit_lsp.py:65-74` |
| Dart | `_normalize_symbol_name` | `A.b` 形式の外側修飾を除去(`rsplit(".", 1)[-1]`) | `language_servers/dart_language_server.py:71-72` |
| Nextflow | `_normalize_symbol_name` | `process GREET` / `workflow SAY_HELLO` / `function foo` の宣言キーワード接頭辞を除去(`<entry>` はそのまま) | `language_servers/nextflow_language_server.py:357-368` |
| F# | `request_document_symbols` | FsAutoComplete の Module 宣言の selectionRange が `module` キーワードを指すバグ(#925)を、行を正規表現で解析してモジュール名位置へ補正 | `language_servers/fsharp_language_server.py:80-130` |
| Fortran | `request_document_symbols` | fortls の selectionRange が行頭 (character 0) を指すバグを、行解析で識別子位置へ再帰的に補正 | `language_servers/fortran_language_server.py:142-175` |
| Marksman (Markdown) | `request_document_symbols` | 見出しの `SymbolKind.String`(15) を `Namespace`(3) に再マップ(String は low-level 扱いで概要から除外されるため) | `language_servers/marksman.py:172-194` |
| Svelte | `request_document_symbols`(+ references/definition) | .ts/.js ファイルの documentSymbol をコンパニオン TS サーバーへ委譲(素の svelte LS は .svelte 以外に何も返さない) | `language_servers/svelte_language_server.py:637-706` |
| Bash | `request_document_symbols` | 実質ログ出力のみのラッパー(補正なし) | `language_servers/bash_language_server.py:301-318` |

備考: Vue/Angular も `request_references` / `request_definition` をコンパニオンサーバー
委譲のため上書きしている(`language_servers/vue_language_server.py:417-442`,
`language_servers/angular_language_server.py:632`)。

---

## 3. LSP サーバー起動前に CLI コマンド実行を要求する言語

lsp-det を PATH 偽装で挟む場合、以下のコマンドが LSP プロセス起動前に subprocess で
実行される。「★」= **LSP サーバーバイナリ自体** に対する CLI 呼び出し(偽装バイナリが
応答を返す必要がある)。

| 言語 | 実行コマンド | 目的 / 失敗時挙動 | 根拠 |
| --- | --- | --- | --- |
| Go ★ | `go version`, `gopls version` | どちらか失敗で `RuntimeError`(起動拒否)。`gopls version` は returncode 0 + 出力必須 | `language_servers/gopls.py:63-104` |
| Rust ★ | `rustup --version`, `rustup which rust-analyzer`, `rustup component add rust-analyzer`, `<候補パス> --version` | **rustup 解決を PATH より優先**。候補には `--version` の機能チェック(returncode 0、timeout 10 秒) | `language_servers/rust_analyzer.py:56-113` |
| Zig ★ | `zig version`, `zls --version` | インストール確認 | `language_servers/zls.py:40,51` |
| Swift ★ | `sourcekit-lsp -h` | インストール確認 | `language_servers/sourcekit_lsp.py:37` |
| Nix ★ | `<nixd> --version`(未検出時 `nix-env -iA nixpkgs.nixd` / `nix profile install github:nix-community/nixd`) | 確認 + 自動インストール | `language_servers/nixd_ls.py:134` ほか |
| SystemVerilog ★ | `verible-verilog-ls --version` | バージョンログ用(失敗しても続行) | `language_servers/systemverilog_server.py:40-59` |
| Erlang | `erl -version`(×2), `rebar3 version` | ツールチェーン確認 | `language_servers/erlang_language_server.py:83-103` |
| Ruby (ruby-lsp) | `ruby --version`(mise 経由あり), 未検出時 `gem install ruby-lsp -v <ver>` | 確認 + 自動インストール | `language_servers/ruby_lsp.py:167,245-260` |
| Ruby (solargraph) | `ruby --version`, `gem list ^solargraph$ -i` | 確認 | `language_servers/solargraph.py:75` ほか |
| OCaml | `opam list -i ocaml-lsp-server`, `opam exec -- which/where ocamllsp`, `opam exec -- ocaml -version`, `opam exec -- dune build @ocaml-index` | opam 経由でパス解決 + index ビルド | `language_servers/ocaml_lsp_server.py:64-255` |
| PHP (phpactor) | `php --version` | ランタイム確認 | `language_servers/phpactor.py:71` |
| Perl | `perl -v`, `perl -MPerl::LanguageServer -e ...` | モジュール確認 | `language_servers/perl_language_server.py:48` ほか |
| R | `R --version`, `R --vanilla ... require('languageserver')` | パッケージ確認 | `language_servers/r_language_server.py:37` ほか |
| Elixir | `elixir --version` | ランタイム確認 | `language_servers/elixir_tools/elixir_tools.py:67` |
| Julia | `julia -e "using LanguageServer"`, 未検出時 `julia -e 'using Pkg; Pkg.add("LanguageServer")'` | 確認 + 自動インストール(stdin=DEVNULL 必須の注意書きあり) | `language_servers/julia_server.py:80-110` |
| Java (JDTLS) | `java -XshowSettings:properties -version` | JVM 検出 | `language_servers/eclipse_jdtls.py:640-650` |
| BSL | `java -version` | Java メジャーバージョン検出 | `language_servers/bsl_language_server.py:48-62` |
| Scala (Metals) | `coursier setup --yes`, `mkdir -p <metals_home>` | Metals ブートストラップ | `language_servers/scala_language_server.py:663-675` |
| F# | `dotnet --info`, `dotnet tool install --tool-path ./ fsautocomplete --version <ver>` | .NET 確認 + ツールインストール(RuntimeDependency の `command`) | `language_servers/fsharp_language_server.py:145,334` |
| PowerShell | `pwsh -NoLogo -NoProfile -Command "Save-Module ..."` | PSScriptAnalyzer 取得 | `language_servers/powershell_language_server.py:185-200` |
| Lean 4 | `lake env` | 環境変数取得(クロスファイル参照に必要) | `language_servers/lean4_language_server.py:48-62` |
| Gleam | `gleam deps download` | LSP 内依存ダウンロードの先回り(readiness 待ち短縮のため) | `language_servers/gleam_language_server.py:140-155` |

補足: ユーザーは LS 固有設定 `ls_base_cmd` / `ls_path` / `ls_args` / `ls_extra_args` で
起動コマンドを明示上書きできる(`src/solidlsp/dependency_provider.py:55-63`)。
PATH 偽装より確実に lsp-det を挟むにはこの設定経路が使える。

---

## 4. lsp-det アダプタ設計への知見

### 4.1 監視すべきシグナルの類型(アダプタ実装の優先順)

1. **`experimental/serverStatus` の `quiescent: true`** — rust-analyzer が発行
   (`language_servers/rust_analyzer.py:742-744`)。clangd もハンドラは登録されるが serena は
   活用していない(`language_servers/clangd_language_server.py:412-414` に TODO)。
   lsp-det の clangd アダプタは serena がやめてしまった quiescent 待ちを実装する価値がある。
2. **`$/progress` トークン drain + 静穏期間(quiet period)** — 最も汎用的。Metals 実装
   (timeout 180 秒 / start_grace 15 秒 / quiet_period 3 秒、
   `language_servers/scala_language_server.py:36-38,167-207`)が最良の一般形:
   「begin が一つも来ないまま grace が切れたら progress 非対応とみなす」
   「トークンが空になっても quiet_period の間、新規 begin が来ないことを確認する」
   の 2 点で、複数フェーズ(indexing → モジュール毎コンパイル)の谷間を誤検知しない。
   TypeScript / Kotlin / Gleam / pyrefly / Nextflow / Haxe も同方式の変種。
   注意: progress を受けるには initialize の client capabilities で
   `window.workDoneProgress: true` を宣言し、`window/workDoneProgress/create`
   リクエストに応答する必要がある(`language_servers/csharp_language_server.py:496-512,631-653`)。
3. **サーバー固有通知** — `$/typescriptVersion`(typescript-language-server)、
   `angular/projectLoadingFinish`、`intellij/ready-for-test`(Kotlin IntelliJ 版)、
   `workspace/projectInitializationComplete`(Roslyn)、`language/status ServiceReady`
   (JDTLS / solargraph / OCaml)、`custom/file-indexed` を .sol ファイル数と突き合わせる
   カウント方式(Solidity)。
4. **サーバーへの能動的問い合わせ** — AL の `al/hasProjectClosureLoadedRequest`
   ポーリング(`language_servers/al_language_server.py:895-957`)。readiness を
   リクエストで聞ける稀有な例で、プロキシのゲート判定に最適(未対応サーバーは
   例外で検知してフォールバック)。
5. **ログ正規表現** — pyright の `Found \d+ source files?`
   (`language_servers/pyright_server.py:140-145`、コメントに「pyright is unreliable
   and there seems to be no better way」)、MATLAB の "mvm attach success" 等。
   脆いが、progress を出さないサーバーでは唯一の手段。
6. **初回 `publishDiagnostics` 到着** — Elm(`language_servers/elm_language_server.py:174-214`)。
   didOpen 後にしか発火しない点に注意。

### 4.2 rust-analyzer / gopls 以外でアダプタを書くなら

- **JDTLS / solargraph**: `language/status` の `ServiceReady` を待つだけでよく実装が容易。
  JDTLS は追加で ProjectStatus(project ready)も見る。
- **typescript-language-server**: `$/typescriptVersion` 受信(tsserver 起動確認)→
  progress drain の 2 段構え。serena 実装がそのまま設計図になる。
- **Roslyn C#**: `workspace/projectInitializationComplete` 一発。ただしソリューション/
  プロジェクトファイルの open(didOpen 相当の solution/open 通知)が先に必要
  (`language_servers/csharp_language_server.py:697`)。
- **Metals**: quiet-period 付き progress drain(4.1-2 の値をそのまま流用可)。
- **pyright 系**: ログ regex + `pyright/endProgress`。regex は固定文字列でなく
  `Found \d+ source files?` として実装すること。
- **gopls 補足**: serena は「即時 ready」とみなしている(`language_servers/gopls.py:327-328`)
  が、これは solidlsp 側が待っていないだけで、lsp-det では gopls の workDoneProgress
  ("Setting up workspace" 等)の drain を実装する余地がある。

### 4.3 プロキシ設計上の注意点

- **フォールバックタイムアウト必須**: solidlsp のほぼ全実装が「timeout → proceed anyway +
  event を set」のパターン(唯一の例外は svelte で raise、
  `language_servers/svelte_language_server.py:155-161`)。readiness ゲートも
  「無期限ブロック」は避け、タイムアウト後は素通しに切り替えるのが安全。
- **didOpen 後にしか待てないサーバーがある**: 基底クラスの固定 sleep は「最初のファイルが
  open された後」に適用される設計(`src/solidlsp/ls.py:1628-1633`)。Elm の diagnostics 待ちや
  intelephense の per-request sleep も同様。ゲートは「initialize 完了」と「初回 didOpen 後の
  索引完了」の 2 段階で持つべき。
- **pre-arm パターン**: 索引の再開を検知するには、didOpen 送信前に完了イベントをクリアする
  フックが要る(`_pre_open_for_cross_file_references`, `src/solidlsp/ls.py:1620-1626`)。
  didOpen → `workDoneProgress/create` の競合を避けるため、TS 実装は create 受信時点で
  イベントをクリアしている(`language_servers/typescript_language_server.py:442-455`)。
- **通知は透過転送しつつ盗み見る(tee)**: serena 自身が上記の独自通知をハンドルするため、
  lsp-det が通知を消費・改変するとクライアント側の readiness 判定が壊れる。
- **initialize 応答の capabilities を改変しない**: clangd や typescript は capabilities を
  assert しており(例: `language_servers/typescript_language_server.py:504-510` の完全一致
  assert、`language_servers/clangd_language_server.py:406-409`)、プロキシが capabilities を
  変えるとクライアントが起動時にクラッシュする。
- **PATH 偽装の限界**: rust-analyzer は `rustup which rust-analyzer` を PATH より優先する
  (`language_servers/rust_analyzer.py:95-106`)。また複数実装が偽装バイナリへの
  `--version` / `version` / `-h` 応答(returncode 0 + それらしい出力)を要求する(§3 ★印)。
  確実な注入経路は `ls_base_cmd` / `ls_path` 設定(`src/solidlsp/dependency_provider.py:55-63`)。
- **コンパニオンサーバー構成**: Vue / Svelte / Angular は本体 + TypeScript(+ HTML)の
  複数サーバー合成で、readiness も合成条件(全サーバー ready ∧ コンパニオン indexing 完了)。
  この種の言語をゲートする場合、プロキシは複数プロセスの readiness を AND で束ねる必要がある。
- **stdio 汚染への注意**: Julia アダプタの教訓として、子プロセスに stdin を継承させると
  JSON-RPC チャネルを壊す(`language_servers/julia_server.py:82-86`、serena #1577)。
  lsp-det が補助 CLI を起動する際は `stdin=DEVNULL` を徹底する。
