# ADR 0015: 下流側が `workspace/didChangeWatchedFiles` を代行し、重複した `didOpen` を `didChange` に書き換える

- 日付: 2026-09-04
- 状態: 採用
- 関連: [ADR 0002](0002-extension-b-surface-in-v01.md) 決定 3（下流が宣言していれば下流側は代行しない）、[ADR 0009](0009-success-criterion-and-two-sided-reference.md) 決定 D-7（宣言しているのに待たないクライアントは隠さない）・D-10（時間判定の排除）、[ADR 0014](0014-freshness-covers-watched-file-changes.md)、[research/disk-edit-propagation-measurement.md](../research/disk-edit-propagation-measurement.md)、[research/serena-processing-around-lsp.md](../research/serena-processing-around-lsp.md) 2 章

## 経緯

lsp-det の下流側は「クライアントに足りないものを代行する」参照実装である（設計 4.1）。これまで代行してきたのは仕様 9 章の推奨挙動（`ready` を待つ、`error` なら待たない）で、本プロトコルの範囲に閉じていた。

Claude Code（CC）の観測（research/disk-edit-propagation-measurement.md、claude-code-dogfooding.md 第 4 回）で、本プロトコルの外にある LSP 上の欠落が 2 つ分かった。

1. CC は `workspace.didChangeWatchedFiles` を宣言せず、通知も送らない。LSP はファイルの監視をクライアントの役割と定め（サーバーは `client/registerCapability` で見てほしい glob を登録する）、サーバーが自前で監視するのは代替にすぎない。gopls と pyright は自前で監視しないので、CC が Bash で編集した開いていないファイルはセッションの間ずっと古いままである。rust-analyzer と typescript-language-server は自前の監視で拾う
2. CC は Write のたびに、既に開いている uri へ `textDocument/didOpen` を送り直す（LSP 違反。開いている文書の変更は `didChange` で伝える）。rust-analyzer・gopls・pyright は黙認するが、typescript-language-server は "Can't open already open document" で拒み、古いバッファがディスクの内容を覆い続ける

どちらも「編集を織り込まない応答」で、ADR 0014 の約束（知らされたら織り込む）の手前で、知らせる側が欠けている。Serena は自前で全ソースファイルの mtime を走査して通知を送っており（research/serena-processing-around-lsp.md 2 章）、そのやり方は時計ではなく要求を引き金にしている点で本プロジェクトの原則と合う。

## 決定

### A. `workspace/didChangeWatchedFiles` の代行

クライアントが capability `workspace.didChangeWatchedFiles` を宣言しておらず、通知 `workspace/didChangeWatchedFiles` を一度も送っていないとき、下流側が代行する。

- **引き金**: 仕様 7.0 の一覧のリクエスト（保留の対象）が下流から届いたとき。時計は使わない（ADR 0009 決定 D-10）
- **列挙**: `initialize` の `rootUri` と `workspaceFolders` の各ルートで `git ls-files --cached --others --exclude-standard` を実行し、追跡中のファイルと無視されていない未追跡のファイルを得る。`.gitignore` の解釈は git に任せ、lsp-det は言語ごとの拡張子の一覧も除外の規則も持たない。git 管理外のルートでは代行しない（stderr に一度だけ記す）
- **差分**: 前回の一覧（パスと mtime）と比べ、増えたものを `Created`、mtime の変わったものを `Changed`、消えたものを `Deleted` として 1 つの通知にまとめ、上流へ送ってからリクエストを転送する。最初の一覧は `initialize` の後に取る
- **停止**: クライアントが自分で通知を送ったのを観測したら、以後は代行しない（Serena は宣言せずに送るので、二重を避ける）
- **写像は関与しない**: どの言語サーバーにも同じに効く。サーバーが自前で監視する場合は通知が二重になるが、4 サーバーとも未登録の通知を害なく受け付けることを実測済み。サーバーは登録した glob で自分に関係ないファイルを捨てる
- **費用**: 要求ごとの `git ls-files` と stat。実装の段で 1935 ファイル（zed）と 3001 ファイル（pyright の fixture）で測って記す
- **実行時の前提**: `git` が PATH にあること。クレートの依存は増えない（ADR 0005）

### B. 重複した `didOpen` の書き換え

下流側は開いている uri の集合を保ち、既に開いている uri への `textDocument/didOpen` を、全文の `textDocument/didChange`（`contentChanges: [{text}]`、`version` は `didOpen` のもの）に書き換えて上流へ送る。すべての上流に対して行う（LSP が求める形に直すだけで、黙認するサーバーにも害がない）。`text` は原文のバイト列を切り出して使い、再シリアライズしない。`didClose` で集合から外す。

### C. 位置づけ

どちらも本プロトコルの値ではなく、仕様には触れない（設計 4.3 の変更）。代行はクライアントの欠落を埋めるものであり、クライアントが自分で行うようになれば消える（A は宣言か送信で止まり、B は重複が来なければ何もしない）。CC への報告（`docs/upstream-submissions.md`）は別途出す。ADR 0009 決定 D-7 の「宣言しているのに待たないクライアントは隠さない」は変わらない。A は宣言していないクライアントにだけ効く。

### 却下した案

| 案                                                                                                                  | 却下理由                                                                                                                                                        |
| ------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 何もせず CC への報告だけにする                                                                                      | 報告は出すが、直るまで CC の利用者の Go / Python / TypeScript の応答は古いまま。代行は消えていく前提で置く                                                      |
| OS のファイル監視（inotify / FSEvents / ReadDirectoryChangesW）で代行する                                           | OS ごとの実装が 3 つ増える。要求を引き金にした走査で足りる                                                                                                      |
| 写像が言語ごとの拡張子と除外の規則を持ち、サーバーが自前で監視するなら省く                                          | 言語ごとの事実の一覧が増え続け、Serena の言語別コードと同じ道をたどる。二重の通知は害がない                                                                     |
| `.gitignore` の解釈器を自前で持つ、または全部を歩く                                                                 | 解釈器は実装が大きく、全部を歩くと `node_modules` や `.venv` を抱える。git に任せる                                                                             |
| 上流への `initialize` に `workspace.didChangeWatchedFiles.dynamicRegistration` を注入し、登録された glob だけを見る | 宣言すると rust-analyzer が自前の監視を止め、lsp-det の代行が唯一の監視になる。要求を引き金にしない再ロード（診断のための `Cargo.toml` の変更など）を取りこぼす |
| B を typescript-language-server のときだけ行う                                                                      | 書き換えは LSP の求める形そのもので、他の上流にも害がない。写像に依存させる理由がない                                                                           |
| B で `didOpen` を `didClose` + `didOpen` に置き換える                                                               | 2 通知になり、閉じた瞬間に診断が消える。`didChange` 1 つで足りる                                                                                                |

## 影響

### 設計（docs/v0.1-design.md）

- 4.3 に代行 2 つを足す。4.4 の完全パースの対象に `textDocument/didOpen`（uri・version・text）と `workspace/didChangeWatchedFiles`（クライアントが送ったことの観測だけ。本文は読まない）を足す
- 4.6 に実行時の前提として `git` を記す

### 実装（ADR 0013・0014 と合わせて）

- 下流側に、開いている uri の集合、ワークスペースのルート、前回の一覧（パスと mtime）を持たせる
- 準拠テスト（`tests/client_conformance.rs`）: 宣言しないクライアントの横断リクエストの前に、ディスク上の変更が `didChangeWatchedFiles` として上流に届く。宣言したクライアント、または自分で送ったクライアントには届かない。git 管理外では届かない。重複した `didOpen` が全文の `didChange` として上流に届く
- 実サーバー: 7.3 の第 2 のテストを、宣言しないクライアント（代行あり）でも通す
- 費用の実測を research に記す

### README

使い方の節に、代行 2 つと `git` の前提を記す。
