# ADR 0008: readiness に観測者専用の `unknown` を足し、`health` による待機終了を規範化する

- 日付: 2026-08-30
- 状態: 採用（決定 A・B と追補 C はいずれも 2026-08-30 承認）
- 関連: [ADR 0003](0003-extension-s-zero-based.md) 決定 2（`dead` の位置づけ）、[ADR 0004](0004-spec-grilling.md) 決定 1（2 軸の独立）、[ADR 0007](0007-quiescent-flap-measured.md)

## 経緯

M2 の PR 2（拡張 S surface）で 2 つの問題が表面化した。

**(1) アダプタなしのときに何を宣言するか。** v0.1-design 4.1 はアダプタなしで `serverStateProvider: true`（基本グレード）を宣言するとしていたが、その場合に `readiness` として返せる値がない。アダプタがないと ready 信号を観測する手段がなく、`initializing` に留め置けば「まだ何も答えられない」という嘘、`ready` を名乗れば無言の嘘そのものになる。仕様 3 章の `readiness` に「不明」を表す値はない。PR 2 ではアダプタがある場合にのみ surface を提供して決着を保留した。

**(2) 失敗したとき、待つ側はどう抜けるか。** (1) の検討中に「サーバーが返すべき失敗の値は要らないのか」という問いが出た。設計 4.2〜4.4 のゲートは「`ready` まで待つ」としているが（仕様 7.1 の 3 は遷移の観測要件であり、待機を定めるのは設計側である）、インデックスが失敗して `health` が `error` になったまま `readiness` が `indexing` から動かないサーバーに対して、ゲートは非常口タイムアウト（既定 300 秒）まで保留し続ける。壊れたサーバーに 5 分黙るのは無言の嘘の変種である。仕様 3 章の「`health` が `error | dead` のとき `readiness` を判断材料に使うべきではない」は非規範の推奨解釈に留まっており、待機の終了条件としては効いていない。

### 根拠: rust-analyzer の失敗時挙動

`crates/rust-analyzer/src/reload.rs` の `current_status()` を確認した。

- ワークスペースのロード失敗（`fetch_workspace_error()` が `Err`）→ `health |= Error`、メッセージ「Failed to load workspaces.」
- ビルドスクリプトの失敗・設定エラー・ワークスペース未発見 → `health |= Warning`
- `quiescent` は `is_fully_ready()` から `health` と独立に計算される。失敗した取得もキューとしては完了するため、ロード失敗時は `{health: error, quiescent: true}` になる

つまり本家の語彙でも、失敗は `readiness`（`quiescent`）ではなく `health` で表現されている。`quiescent: true` は「試行が終わった」であり「成功した」ではない。

## 決定

### A. `readiness` に `unknown` を追加する

1. 仕様 3 章の `readiness` に `"unknown"` を追加する。定義: **`readiness` を観測する手段がない**。クライアントは基本グレードと同じく、応答が不完全でありうることを承知で進むか、自前で待つかを判断する
2. 仕様 6.1 の送出主体の表で、`readiness: unknown` は**サーバーが送出してはならず、中継層のみ送出できる**とする。サーバーは自分の readiness を必ず知っているので、`unknown` を出す理由がない。`health: dead` と対称の、観測者だけが出せる値である
3. 仕様 7.1 の 3（`ready → indexing → ready` の遷移が観測できる）は「`readiness` を `unknown` 以外で報告する実装に適用する」と条件付ける。7.1 の 1（initialize 直後に `ready` ではない）は `unknown` で満たされるため変更しない
4. v0.1-design 4.1 のアダプタなしの宣言を確定する: `serverStateProvider: true`（基本グレード）、`readiness` は**最初から** `unknown`、`health` はプロセス観測に基づく ~~`ok | dead`~~ `unknown | dead` を追跡する（追補 C-1 で訂正。`ok` は観測できない）。`initializing` から始めない（追跡していないものを追跡しているように見せない）
5. 仕様 6.1 末尾の「上流提案時に位置づけを再検討する」留保を `unknown` にも適用する。`dead` と `unknown` は同じ論点（観測者の知識状態をサーバーの状態と同じ語彙に置く）を共有しており、新しい問題は増えない

### B. `health` による待機終了を規範化する

1. 仕様 6 章に項を追加する: **サーバーはインデックスの失敗を `readiness` ではなく `health` で表さなければならない**（規範）。**`health` が `error | dead` のとき `ready` を待ち続けることは本拡張の意図に反する**（推奨解釈。追補 C-4 で非規範に改めた。仕様 1 章「クライアントの挙動を強制しない」を守るため）
2. `readiness` に失敗を表す値（`failed` 等）は**足さない**。失敗は `health` と直交しないため（後述の却下理由）
3. v0.1-design 4.2 のゲートの条件を次のとおり定める:
   - **保留を続ける条件**: `readiness` が `ready` でも `unknown` でもなく、**かつ** `health` が `error` でも `dead` でもない
   - `health` が `error` になったら、`dead` と同じく保留分を含む以後の横断リクエストに即座にエラーを応答する。エラーには `message`（人間向け）を含めてよい
   - `readiness` が `unknown` ならゲートは働かない（純透過）。アダプタなしの現状の挙動を文書化するものであり、実装上の変更はない
4. 非常口タイムアウト（設計 4.4）の位置づけは変えない。`health: error` はタイムアウトを待たずに抜ける正常系であり、非常口は「アダプタが ready 信号を取り逃した」ときの検出器のままである

### C. 追補（code-review の指摘により、A・B の穴を埋める）

決定 A・B を仕様に写す過程で、承認済みの文面のままでは矛盾が残る箇所が見つかった。いずれも A・B の趣旨から導かれる帰結であり、新しい方向性ではない。

1. **`health` にも `unknown` を足す**。A-4 の「アダプタなしは `health: ok`」は観測なしの主張だった。`ok` は仕様 3 章で「完全に機能している」と定義されており、プロセスが生きていることしか観測しない中継層には名乗れない。`initializing` を退けたのと同じ理由である。送出主体は `readiness: unknown` と同じくサーバー禁止・中継層のみ。アダプタなしの lsp-det は両軸 `unknown` で始まり、消失時に `health: dead` になる
2. **保証は `health` が `error | dead` でないときにのみ適用する**（仕様 5 章・6 章 1・2 項）。B-1 が「失敗時に `readiness` は `ready` にしてもよい」と認めたため、rust-analyzer の失敗時の `{error, ready}` に対して 6 章 1 項が「`ready` なら完全でなければならない」を要求し、`completeness` を宣言する lsp-det が仕様違反に追い込まれていた。3 章の「`error | dead` のとき readiness を見るべきではない」を保証の適用条件として明文化する
3. **6 章 3 項（再インデックスで `indexing` に戻す）を `unknown` 以外で報告する実装に限定する**。7.1 の 3 だけを条件付けても、6 章 3 項が無条件のままではアダプタなしの lsp-det は準拠テストを通りながら非準拠だった
4. **B-1 のクライアント側の文を非規範に戻す**。「クライアントは待ってはならない」は仕様 1 章「本拡張はクライアントの挙動を強制しない」（ADR 0004 決定 5 の設計哲学）と衝突する。サーバー側の義務（失敗は `health` で表す）だけを規範とし、待機の終了は推奨解釈に留める。代わりに仕様 7.1 に 4 項（失敗が `health` として報告される）を足し、サーバー側の義務をテスト可能にする
5. **ゲートの判定を (readiness × health) の表にする**（設計 4.2）。B-3 の文章表現では `{unknown, dead}`（B-3 は純透過と言い、4.2 は即時エラーと言う）、`error` からの回復（「以後」が永久を意味するか）、`warning` の扱いが定まらなかった。表にして全組み合わせを埋め、状態遷移ごとに再評価することを明記する。`warning` は `ok` と同じ扱い（待っても改善しない）。`dead` は 4.7 のとおりプロキシ終了を伴うため「以後のリクエスト」は存在しない

### 却下した案

| 候補 | 不採用の理由 | 再検討の条件 |
| --- | --- | --- |
| `readiness` に `failed` を足す | `health: error \| warning` と同じ事実の二重表現になる。`{health: ok, readiness: failed}` が文法上書けてしまうが、インデックスできないのに結果を信頼できる状態はありえず、意味を持たない。`failed` が持つ情報で `health` に写らないものはない。回復可能性（`Cargo.toml` を直せば再ロードして `ok` に戻る）も `error` と `failed` で変わらず、語を分ける利益がない | サーバーが自己申告する失敗のうち `health` に写らない情報（回復不能と回復可能の機械可読な区別等）を必要とする実例が現れたとき |
| `readiness` を省略可能にする（`unknown` の代わり） | 意味は同じだが、`state.readiness !== "ready"` → 保留、という素朴なクライアント実装が「永久に待つ」罠を踏み、しかもログに現れない。明示的な値なら網羅的な match で扱いを強制でき、ログにも残る | 上流提案の場で LSP 側が省略可能フィールドの形を求めたとき |
| アダプタなしでは宣言しない（PR 2 の候補 1）／`--adapter` を必須にする（候補 3） | どちらも `health: dead` を捨てる。プロセス消失の観測はアダプタと無関係にできるので、clangd のように readiness 信号を持たないサーバー（本 ADR 以前の仕様 8 章は「中継層でも合成困難」と記していた）にも `dead` は届けられるはずである。中継層の固有価値（ADR 0003）をアダプタのないサーバーにまで届けるのが `unknown` の役割であり、1・3 はそれを放棄する | クライアントが接続断から `dead` 相当を自前で合成できるようになり、中継層が `dead` を出す価値が消えたとき |

## 影響

### 仕様（docs/spec/extension-s-server-state.md）

- 3 章: `readiness` に `"unknown"` を追加し定義を書く。推奨解釈の直後に「待機については 6 章 5 項が規範」と参照を置く
- 6 章: 5 項「`health` が `error | dead` のとき待ってはならない。失敗は `health` で表す」を追加
- 6.1: 表の `readiness` 行を分け、`unknown` はサーバー送出禁止・中継層のみとする。末尾の留保を `dead` と `unknown` の両方に適用
- 7.1: 3 を「`unknown` 以外で報告する実装に適用」と条件付け
- 8 章: clangd の欄を「中継層は `readiness: unknown` + `health` を提供」に更新

### 設計（docs/v0.1-design.md）

- 4.1: 「未決」節を本 ADR で決着済みとして書き換える。アダプタなし → `true`、`unknown`、`health` は `ok | dead`
- 4.2: 保留の継続条件と `health: error` での即時エラーを追記。`unknown` でゲート無効を明記
- 4.4: 変更なし（`health: error` は非常口の対象外である旨を一文添える）

### 実装

- `state::Readiness` と `state::Health` に `Unknown` を追加（ワイヤ形式のテストも追加）
- アダプタなし用の追跡: 現状はアダプタなしだと surface が丸ごと省略され（`Option<RustAnalyzerAdapter>`）、`dead` も出ない。状態の保持と `mark_dead` を `RustAnalyzerAdapter` から切り出し、アダプタの有無によらず health を追跡する。あわせて `Surface::on_client` が無条件に rust-analyzer の capability を注入している箇所と、`leaves_the_initialize_alone_when_no_adapter_is_selected` テストの期待を改める
- 準拠テストハーネスの `wait_until_ready` は `readiness != ready` で回り続ける。これは本 ADR が警告する「永久に待つ」クライアントそのものなので、`health` が `error | dead` なら抜けるようにし、`unknown` の被験者には使わない
- 準拠テストスイート: アダプタなしの lsp-det を被験者に加え、7.1 の 1・2 と 6.1（dead）を通す。7.1 の 3 は条件付き実行にする。`health: error` を偽上流から送らせ、ゲート導入後に即時エラーを検証するテストを PR 3 で追加
- ゲート（PR 3）: 保留条件を決定 B-3 のとおりに実装する。`health: error` 時のエラーコードは `dead` と同じにするか区別するかを PR 3 で決める（本 ADR では定めない）

### 上流提案への影響

「中継層でしか出せない値がある」という主張（ADR 0003）が `health: dead` の 1 点から、`health: dead` と `readiness: unknown` の 2 軸対称な主張になる。提案時にはどちらも「クライアントライブラリが合成する値」として位置づけを再検討する（6.1 の留保）。
