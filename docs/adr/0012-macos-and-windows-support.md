# ADR 0012: macOS と Windows に対応し、プロセス寿命の追従を OS ごとの機構で実装する

- 日付: 2026-09-04
- 状態: 採用
- 関連: [ADR 0006](0006-external-review-fixes.md) 決定 5（置き換える）、[ADR 0005](0005-runtime-and-dependencies.md)（依存の制約を引き継ぐ）、[ADR 0009](0009-success-criterion-and-two-sided-reference.md) 決定 D-10（時間判定の排除を引き継ぐ）

## 経緯

ADR 0006 決定 5 は v0.1 の対応 OS を Linux のみとし、macOS でビルドが失敗することを「pdeathsig なしで静かに動くより正直な失敗」として受け入れた。v0.1 と v0.2 が完了し、README を書いて上流への働きかけに移る段階（ADR 0010 決定 A-4）で、この制約を見直した。

- lsp-det が Linux に依存する箇所は `PR_SET_PDEATHSIG` の 2 回の呼び出し（`src/process.rs`）だけである。フレーミング・スレッド・イベントループ・写像はすべて標準ライブラリで、OS に依らない
- 上流の保守者と、Serena や Claude Code の利用者には macOS が多い。「Linux のみ」は、プロトコルを試してもらう段階で不利になる
- `PR_SET_PDEATHSIG` が担う 2 経路（クライアントが不意に死んだら lsp-det が死ぬ、lsp-det が不意に死んだら上流が死ぬ）は多重防御の 2 段目である。1 段目の stdin の EOF（クライアントが死ねば lsp-det の stdin が閉じ、lsp-det が死ねば上流の stdin が閉じる）は OS に依らず働く

現状（2026-09-04、main `fa5f563`）: `cargo check --target aarch64-apple-darwin` は `libc::prctl` と `libc::PR_SET_PDEATHSIG` が見つからず失敗する。`--target x86_64-pc-windows-msvc` はライブラリとバイナリは通る（`cfg(not(unix))` の空実装）が、テスト補助（`libc::kill`、`/proc`、`pgrep`、`PermissionsExt`）が失敗する。

## 決定

### A. 対応 OS を Linux、macOS、Windows とする

ADR 0006 決定 5 と設計 2 章の非目標「Linux 以外の OS」を置き換える。3 つの OS でビルドが通り、同じテストが通ることを求める。

### B. プロセス寿命の 2 経路は OS ごとの機構で実装する

「クライアントが不意に死んだら lsp-det も終了する」（自身の追従）と「lsp-det が不意に死んだら上流も終了する」（上流の追従）を、それぞれの OS が持つ機構で実装する。時間に基づく判定（親の生存をポーリングする等）は持ち込まない（ADR 0009 決定 D-10）。

| OS      | 自身の追従                                                                                                                          | 上流の追従                                                                                                                                                                       |
| ------- | ----------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Linux   | `PR_SET_PDEATHSIG`（現状のまま）                                                                                                    | `PR_SET_PDEATHSIG`（現状のまま）                                                                                                                                                 |
| macOS   | 親 pid を `kqueue` の `EVFILT_PROC` / `NOTE_EXIT` で監視するスレッド。親の終了を観測したら上流を殺して終了する                      | 同等の機構がない。lsp-det の正常終了では既存どおり上流を殺す。lsp-det が `SIGKILL` 等で不意に死んだときは上流の stdin の EOF に委ねる（言語サーバーが EOF で終了することに依存） |
| Windows | 親プロセスのハンドルを `WaitForSingleObject` で待つスレッド。親の終了を観測したら終了する（上流は下記の Job Object が道連れにする） | Job Object の `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`。lsp-det が死ぬとハンドルが閉じ、カーネルが上流を殺す                                                                         |

macOS の上流の追従が EOF 頼みになることは、4 つの言語サーバーが stdin の EOF で終了するかを実測して記録する（Linux で `PR_SET_PDEATHSIG` を使わずに測れる）。EOF で終了しない言語サーバーが見つかったら、そのときに機構を足す。

Windows の親 pid は `NtQueryInformationProcess`（`ProcessBasicInformation`）で取る。pid は再利用されるので、起動直後にハンドルを開き、以後は pid ではなくハンドルで待つ。

### C. 依存は増やさない

Windows API は `kernel32` / `ntdll` の必要な関数だけを `extern "system"` で宣言する。macOS の `kqueue` は既存の `libc` にある。

### D. 検証は 3 つの OS の CI で行う

- GitHub Actions に CI を置き、`ubuntu-latest` / `macos-latest` / `windows-latest` で `cargo build` と `cargo test` を回す。実サーバーの結合テスト（`#[ignore]`）は従来どおり CI に入れない（設計 6 章）
- プロセス寿命の 2 経路は多プロセスの結合テスト（`tests/process_lifetime.rs`）を OS 共通に書き、CI の 3 ランナーで回す。これで macOS と Windows の挙動も自動で確かめられ、「ビルドだけ確認」という状態を作らない
- この開発環境（Linux）でも `rustup` のターゲット `aarch64-apple-darwin` と `x86_64-pc-windows-msvc` で `cargo check --tests --examples` を通せる。`scripts/check-targets.sh` にまとめ、push の前に回す
- 実サーバーの結合テストと上流の受け入れ条件は Linux で回す。テスト補助のプロセス探索（`/proc` と `pgrep`）は OS ごとに分ける
- CI の toolchain は `rustup` の stable（`flake.nix` は作者の開発環境の固定で、CI はそれに依存しない）

### E. タグで各 OS のバイナリを作る

`v*` のタグを push したら、GitHub Actions が Linux（x86_64、aarch64）・macOS（x86_64、aarch64）・Windows（x86_64）のリリースビルドを作り、GitHub Release に添付する。利用者が `cargo build` なしに試せるようにするためで、上流の保守者にプロトコルを試してもらう段階（ADR 0010 決定 A-4）の入口になる。バイナリの名前は `lsp-det-<target>`（Windows は `.exe`）とし、アーカイブしない。

### 却下した案

| 案                                                   | 却下理由                                                                                                                                                         |
| ---------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS と Windows では stdin の EOF だけに頼る        | 1 段目だけになる。EOF が届かない場面（別プロセスがパイプの書き込み側を継承している等）で lsp-det と上流が残る。Linux と同じ 2 段を、機構があるところでは用意する |
| 親の生存を一定間隔でポーリングする                   | 時間に基づく判定になる（ADR 0009 決定 D-10）。各 OS に信号で待てる機構がある                                                                                     |
| `windows-sys` クレートを足す                         | 必要な関数は 10 個に満たない。依存は保守コストで、宣言を手で書く方が小さい（ADR 0005）。再検討条件: 宣言が増えて保守が難しくなったとき                           |
| macOS で上流の追従のために仲介プロセスを置く         | 3 プロセス構成になり、仲介プロセス自身の寿命という同じ問題が 1 つ増える。言語サーバーは stdin の EOF で終了するのが LSP の慣行であり、まず実測する               |
| `cfg(unix)` を残し macOS だけ pdeathsig なしで動かす | ADR 0006 が退けた「静かに動く」そのもの。追従できないことを機構の有無で決め、文書に記す                                                                          |

## 影響

### ADR 0006

決定 5 は本 ADR が置き換える。索引の該当行を更新する。

### 設計（docs/v0.1-design.md）

- 2 章の非目標から「Linux 以外の OS」を外す
- 4.5 の「Linux: `PR_SET_PDEATHSIG` + stdin EOF 検知の二重化」を、OS ごとの機構の表に置き換える
- 4.6 の依存に Windows API の直接宣言を記す

### 実装

- `src/process.rs` を OS ごとのモジュールに分ける。公開 API（`spawn`、`Upstream`、自身の追従の開始）は共通
- `tests/process_lifetime.rs` を足す
- テスト補助（`tests/support/mod.rs` のプロセス探索、`tests/upstream_dev.rs` の実行可能判定）を OS ごとに分ける
- `scripts/check-targets.sh`、`.github/workflows/ci.yml`、`.github/workflows/release.yml` を足す

### README と CLAUDE.md

対応 OS とリリースバイナリの記述を更新する。
