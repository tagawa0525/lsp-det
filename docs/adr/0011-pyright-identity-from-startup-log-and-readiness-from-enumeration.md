# ADR 0011: `serverInfo` のないサーバーは起動時のログの名乗りで写像を選び、pyright の readiness はファイル列挙の完了で決める

- 日付: 2026-09-03
- 状態: 採用
- 関連: [ADR 0009](0009-success-criterion-and-two-sided-reference.md) 決定 D-2、[ADR 0010](0010-python-typescript-mappings-for-client-adoption.md) 決定 B、[research/pyright-readiness-measurement.md](../research/pyright-readiness-measurement.md)

## 経緯

M5（pyright の写像）に着手する前の実測（research/pyright-readiness-measurement.md）で、ADR 0010 の前提が 2 点崩れた。

1. **pyright は `InitializeResult.serverInfo` を返さない**。写像は `serverInfo.name` で選ぶ（ADR 0009 決定 D-2）ので、pyright には写像が選べない。basedpyright は `serverInfo` を返す。Claude Code の公式プラグインも Serena の既定も pyright 本体なので、主経路で名乗りがない
2. **pyright の横断リクエストの完全性を決めるのは `$/progress` ではなくファイル列挙の完了である**。references は追跡中のファイル一覧を走査し、その一覧はタイマーで少しずつ列挙される。3001 ファイルの fixture で `initialize` 直後の references は 0 件、「Found 3001 source files」のログの後は全件だった。`$/progress` は開いたファイルの解析と references の実行中にしか出ず、2 ファイルの fixture では一度も出なかった。列挙完了の唯一の信号は `window/logMessage`（info）の "Found N source files" または "No source files found." である

## 決定

### A. 写像の選択（ADR 0009 決定 D-2 の補完）

1. 写像は引き続き `InitializeResult.serverInfo.name` で選ぶ。`serverInfo` があればそれが最優先で、他の手段より強い名乗りとして扱う
2. **`serverInfo` のないサーバーは、起動時に自ら送る `window/logMessage` の名乗りで選ぶ**。pyright 系は コンストラクタで `${productName} language server ${version} starting` を info に出す（"Pyright language server 1.1.412 starting"、"basedpyright language server 1.39.8 starting"）。設定の読み込み前なので抑制されず、`initialize` 応答より先に届く。名前も版もここから取る
3. 選択の時点は `initialize` 応答に限らない。起動時のログが `initialize` 応答より先に届いたら、その時点で写像を選ぶ。後から `initialize` 応答に `serverInfo` があれば、それで選び直す（basedpyright は両方を出すが同じ写像を指す）
4. 名乗りの認識は写像ごとに書く。汎用の正規表現機構は作らない。写像を足すときに必要になったら、その写像の認識を足す
5. 上流の起動コマンド名（`pyright-langserver`）では選ばない。Serena や npx のようなラッパー越しでは名前が変わり、ワイヤ上の自己申告という趣旨から外れる

### B. pyright の写像（ADR 0010 決定 B の M5 を置き換える）

1. **readiness**: `initializing` から始め、ファイル列挙の完了で `ready` にする。完了の信号は `window/logMessage` の "Found N source files" または "No source files found."。ワークスペースフォルダごとに "Starting service instance \"name\"" が出て列挙もフォルダごとなので、**"Starting service instance" の数だけ完了ログを数えてから `ready`**（gopls の全フォルダ待ちと同じ）
2. "Searching for source files"（再列挙の開始、log レベル）が届いたら `indexing` に戻す。既定の logLevel（Info）では届かないので、既定設定では再列挙は観測できない。これは pyright の語彙の限界として記録し、gopls の go.mod 変更時と同じ扱いにする
3. **health**: 信号がないので `unknown` のまま（仕様 8.2 の 2）。列挙の完了は「機能している」ことの観測ではない。クラッシュは接続の終了（EOF）で伝わる（ADR 0009 決定 C-3）
4. `$/progress` は読まない。開いたファイルの解析の進行であって、横断リクエストの完全性とは別の事柄である（診断の完了は予約のみ。ADR 0003 決定 4）
5. basedpyright は同じ写像を使う。`serverInfo.name` "basedpyright" と起動ログの "basedpyright language server" の両方で選ぶ
6. 保証（`completeness` / `freshness`）は、実サーバーで 7.2 / 7.3 を通した版にだけ宣言する（ADR 0009 決定 D-5）。仕様 10 章の見込みは宣言ではない
7. クライアントが `logLevel` を Warning 以上に設定すると完了ログが届かず、写像は `initializing` に留まる。lsp-det は設定を書き換えない（ボディは原文のまま転送する。設計 4.4）。Claude Code の公式プラグインも Serena も `logLevel` を設定しないことを確認した。他のクライアントで問題が出たら、そのときに観測して決める

### C. 上流への働きかけ

- pyright 本体に `InitializeResult.serverInfo` を足す PR を出す。basedpyright が既に持っており、数行の変更で LSP 3.15 以降の標準項目である。最初の上流貢献として、本 ADR の A-2 が不要になる方向の変更である
- 列挙完了を `$/progress` や専用通知で伝える提案は、vision 5 章の経路（サーバー状態プロトコルそのものの提案）に含める。ログ文字列の解釈は暫定であり、最終形はサーバーが本プロトコルを話すことである

### 却下した案

| 候補                                                                    | 不採用の理由                                                                                                                    |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| 上流の起動コマンド名（argv）で写像を選ぶ                                | ラッパー越しで名前が変わる。ADR 0009 が `--adapter` を消した理由（起動の仕方に判定を依存させない）に反する                      |
| pyright は写像なし（両軸 `unknown`）とし、basedpyright だけ写像する     | 正直だが主経路（Claude Code の公式プラグイン、Serena の既定）に効かない。起動時のログという自己申告があるのに使わない理由がない |
| `$/progress` の begin / end を readiness にする（ADR 0010 の原案）      | 実測で横断リクエストの完全性と無関係だった。2 ファイルでは一度も出ず、3001 ファイルでは列挙完了と無関係なタイミングで出る       |
| 最初の完了ログで `ready` にする（フォルダを数えない）                   | 複数フォルダで残りのフォルダが列挙中のまま `ready` を名乗る。gopls で Copilot に指摘された誤りと同じ                            |
| lsp-det が `workspace/configuration` の応答を書き換えて logLevel を確保 | ボディの原文転送（設計 4.4）に反する。クライアントの設定を勝手に変える中継層は信頼できない                                      |
| 起動ログを汎用の正規表現テーブルで認識する                              | 必要になるまで作らない。現時点で必要なのは pyright 系の 1 形式だけ                                                              |

## 影響

### ADR 0009 / ADR 0010

- ADR 0009 決定 D-2 は本 ADR の A で補完する（`serverInfo` がないときの手段を足す）。D-2 自体は生きている
- ADR 0010 決定 B の M5 の記述（`$/progress` からの合成）は本 ADR の B に置き換える。M5 の位置づけ・手順・M6 / M7 は変わらない

### 設計（docs/v0.1-design.md）

- 4.2 上流側: 写像の選択に「`serverInfo` がなければ起動時のログの名乗り」を足す
- 5.3 と 8 章 M5: pyright の信号を本 ADR の B に合わせる

### 実装

- `Tracker` に、上流の通知から写像を選ぶ経路を足す（`initialize` 応答前でも選べる）。`adapter::select` に起動ログからの選択を足す
- `src/adapter/pyright.rs`（新設）。`TESTED_VERSIONS` は M5 で実サーバーの準拠テストを通してから載せる

### 仕様

- 規範（3〜9 章）は変更なし。10 章（既存実装との対応、情報提供）の pyright 行は本 ADR と同時に「`window/logMessage` のファイル列挙完了」に書き換える。保証の欄は M5 で準拠テストを当ててから確認済みにする
