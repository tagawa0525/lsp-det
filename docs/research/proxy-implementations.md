# LSPプロキシ実装調査: ra-multiplex / emacs-lsp-booster / lsp-devtools

lsp-det(単一クライアント・単一上流・stdio・readinessゲート付き透過プロキシ)の
実装設計に向けて、`reference/` 配下の3つの既存実装を読解した結果をまとめる。

## 要約

- **フレーミング**: ra-multiplex は tokio の `AsyncBufRead` 上に `read_until(b'\n')` +
  `read_exact` でヘッダ/ボディを読む堅実な実装。emacs-lsp-booster は std の
  `BufRead` で同等の同期版。lsp-devtools はフレーミングを一切解釈せず生バイトを
  中継する(完全なバイト保存)。3者の設計はそれぞれ「完全パース」「片方向のみ
  パース・素通し」「無パース」の3段階に対応し、lsp-det の設計選択肢を網羅している。
- **パース深度とバイト保存性**: ra-multiplex は全メッセージを serde で完全パースし
  再シリアライズするため、キー順序・数値表現が変わりうる(`preserve_order` 無効)。
  emacs-lsp-booster は client→server 方向をボディ文字列のまま素通しし、バイト列を
  保存する。lsp-det には「ヘッダのみパース + ボディは `method`/`id` を覗くだけで
  原文バイトを転送」というハイブリッドを推奨する。
- **プロセス管理**: ra-multiplex は tokio (current_thread) でタスク分割し、
  `select!` + `Notify` + `start_kill()` で子プロセスを落とす。emacs-lsp-booster は
  std スレッド4本 + mpsc チャネル2本の素朴な構成で、panic フックによる全体終了と
  exit code 伝播を行う。lsp-devtools は terminate → 5秒待ち → kill のエスカレー
  ションを実装しており、shutdown 系の参考になる。
- **ハンドシェイク**: ra-multiplex は initialize を横取りして応答をキャッシュから
  返すが、「サーバの最初のメッセージは initialize 応答である」「クライアントの
  2番目のメッセージは initialized 通知である」という厳格な順序仮定を置いており、
  initialize 応答前に届く通知や割り込みメッセージで即 bail する(issue #89 相当の
  脆弱点)。lsp-det のreadinessゲートでは「想定外メッセージはエラーにせずバッファ
  する」設計が必須。
- **$/cancelRequest**: 3実装とも特別扱いしない。ra-multiplex はコメントで多重化の
  必要性を認識しつつ未実装(id書き換えと不整合のまま素通し)。lsp-det は id を
  書き換えないので素通しで正しいが、ゲート中に保留したリクエストへの cancel だけ
  は自前処理が要る。
- **推奨**: tokio (current_thread) + serde_json(`RawValue` で浅いパース)+
  `Content-Length` のみ必須の寛容なヘッダパーサ。詳細は末尾の推奨事項を参照。

対象リポジトリ(いずれも shallow clone のため issue 番号と commit の突合は不可。
コード上の該当箇所を示す):

| リポジトリ | 言語/ランタイム | 役割 |
| --- | --- | --- |
| `reference/ra-multiplex` | Rust / tokio | 複数クライアント→単一LSPサーバの多重化デーモン |
| `reference/emacs-lsp-booster` | Rust / std スレッド | Emacs向け: server→client JSONをelispバイトコードへ変換する1:1プロキシ |
| `reference/lsp-devtools` | Python / asyncio | LSPトラフィックの記録・観測用の透過エージェント |

## 1. stdio の Content-Length フレーミング実装

### ra-multiplex: `LspReader` / `LspWriter`(非同期・完全パース)

`reference/ra-multiplex/src/lsp/transport.rs` に集約されている。

読み手は `AsyncBufRead` を包む構造体で、ボディ用バッファを使い回す
(transport.rs:10-15)。

```rust
pub struct LspReader<R> {
    reader: R,
    batch: Vec<Message>,
    buffer: Vec<u8>,   // with_capacity(1024)、メッセージごとに clear + resize
    tag: &'static str,
}
```

ヘッダパース `read_header`(transport.rs:47-98)の要点:

- `read_until(b'\n', &mut self.buffer)` で1行ずつ読む(transport.rs:53)。
  `AsyncBufReadExt::read_until` が内部でループするため、**分割到着はここで吸収**
  される。呼び出し側に「途中まで読めた」状態は漏れない。
- `Ok(0)` を EOF として `None` を返す(transport.rs:54)。`ConnectionReset` /
  `ConnectionAborted` / `BrokenPipe` も正常クローズ扱い(transport.rs:56-62)。
- 行末は `strip_suffix(b"\r\n")` で厳格に検証(transport.rs:64-67)。
- ヘッダ名と値は `split_once(": ")` で分離し、名前を `to_ascii_lowercase()` して
  比較(transport.rs:75-90)。`content-type` と `content-length` のみ認識し、
  **未知ヘッダは `bail!` でエラー**(transport.rs:89)。重複ヘッダも `ensure!` で
  拒否(transport.rs:82,86)。
- 空行でヘッダ終了、`content-length` 必須(transport.rs:71-73,93)。

ボディ読み `read_message`(transport.rs:107-158):

- `buffer.resize(content_length, 0)` + `read_exact`(transport.rs:120-122)。
  巨大メッセージも `content_length` ぶん一括確保して読む(サイズ上限なし。
  バッファは使い回しなので、一度大きなメッセージが来ると容量は残り続ける)。
- `str::from_utf8` で UTF-8 検証後、`serde_json::from_str` で `Message` 列挙体へ
  完全パース(transport.rs:133-157)。
- ボディが `[` で始まる場合は JSON-RPC バッチとして `Vec<Message>` にパースし、
  1件ずつ返す(transport.rs:141-150)。LSP ではバッチは実質使われないが防御的。

書き手 `LspWriter::write_message`(transport.rs:161-192)は、使い回しバッファへ
`serde_json::to_writer` でシリアライズし、`Content-Length: {}\r\n\r\n` を前置して
`write_all` ×2 + `flush`(transport.rs:180-191)。**メッセージごとに必ず flush**
するのが重要(LSPは行きが揃わないと相手が固まる)。`content-type` は読んでも
転送しない方針が doc コメントに明記されている(transport.rs:17-27)。

### emacs-lsp-booster: `rpc_read` / `rpc_write`(同期・素通し)

`reference/emacs-lsp-booster/src/rpcio.rs` の40行程度の実装。

```rust
// reference/emacs-lsp-booster/src/rpcio.rs:7-32(要約)
pub fn rpc_read(reader: &mut impl std::io::BufRead) -> Result<String> {
    let mut content_len: Option<usize> = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.is_empty() { return Ok(String::new()); }   // EOF
        if line == "\r\n" {
            if let Some(content_len) = content_len {
                let mut result: Vec<u8> = vec![0; content_len];
                reader.read_exact(&mut result)?;
                return Ok(String::from_utf8(result)?);
            }
        }
        let splitted: Vec<&str> = line.trim().splitn(2, ": ").collect();
        ...
        if splitted[0] == "Content-Length" { content_len = Some(...); }
    }
}
```

- std の `BufRead::read_line` / `read_exact` が分割到着を吸収する同期版。
- `Content-Length` **のみ**を見る(大文字小文字は区別。rpcio.rs:28)。他ヘッダは
  `": "` 区切りでさえあれば無視して読み飛ばす(ra-multiplex より寛容)が、
  区切りのない行は `bail!("Invalid header format")`(rpcio.rs:24-26)。
- 戻り値はボディの `String`。**JSONパースはこの層では行わない**。
- 書き込み `rpc_write`(rpcio.rs:34-41)は `BufWriter` に
  `Content-Length: {content.len()}\r\n\r\n` + ボディ + `flush()`。`&str::len()` は
  バイト長なので Content-Length として正しい。
- メッセージごとに `String`/`Vec` を新規確保する(バッファ使い回しなし)。

### lsp-devtools: フレーミングを解釈しない生バイトポンプ

`reference/lsp-devtools/lib/lsp-devtools/lsp_devtools/agent/agent.py:102-123` の
`connect_streams` は、`source.read(1024)` で読めただけのチャンクを即
`dest.write(data)` へ流す。Content-Length 境界すら見ないため、**転送経路上の
バイト保存性は完璧**(観測用の複製にだけ独自の `!BI` ヘッダを付けて別送する。
agent.py:32,115-116)。LSPとしての解釈は TCP の先の受信側
(`agent/server.py:31-49` の `raw_parser` など)で行う。

stdio の非同期化は Windows 互換のためにスレッドプール + 1バイトずつの読み取り
ループという力技で実現しており(`agent/io_.py:58-68`、コメントで非効率を自認)、
Rust では参考にしなくてよい。POSIX 専用の `connect_read_pipe` 版も併存する
(io_.py:146-165)。

### lsp-det への含意

- 「1行読む→`\r\n`検証→`:`区切りで分割→空行でボディへ→`read_exact`」という骨格は
  2つのRust実装で共通。tokio でも std でもバッファ付きリーダの `read_until` /
  `read_line` + `read_exact` に分割到着処理を任せるのが定石。
- ヘッダ検証の厳格さは ra-multiplex(未知ヘッダで bail)と booster(実質無視)で
  対照的。透過プロキシとしては booster 寄り(`Content-Length` 必須、他は保持 or
  無視)が安全。ただしヘッダ名比較は ra-multiplex 同様 ASCII 小文字化して行うべき
  (仕様上ヘッダ名は case-insensitive とみなすのが無難)。
- 巨大メッセージ対策はどちらも「`content_length` ぶん一括確保」のみ。単一
  クライアント用途なら十分だが、異常な Content-Length(数GB)で即死しないよう
  上限チェックを入れる価値はある。

## 2. プロセス管理

### ra-multiplex(tokio、デーモン型)

ランタイムは single-thread tokio
(`reference/ra-multiplex/src/main.rs:56` の
`#[tokio::main(flavor = "current_thread")]`)。

spawn は `reference/ra-multiplex/src/instance.rs:426-433`:

```rust
let mut child = Command::new(&key.server)      // tokio::process::Command
    .args(&key.args)
    .envs(&key.env)
    .current_dir(&key.workspace_root)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
```

1インスタンスにつき4タスク構成(instance.rs:459-490):

| タスク | 役割 | 該当箇所 |
| --- | --- | --- |
| `stderr_task` | stderr を `read_line` で行単位に読み、tracing でログ出力 | instance.rs:546-568 |
| `stdin_task` | `mpsc::Receiver<Message>`(容量64、instance.rs:474)から受けて子の stdin へ書く。`BrokenPipe` は静かに終了 | instance.rs:571-589 |
| `stdout_task` | 子の stdout からメッセージを読み、各クライアントへ振り分け | instance.rs:639-809 |
| `wait_task` | `select!` で `close: Notify` と `child.wait()` を待つ | instance.rs:592-636 |

`wait_task` が終了処理の中心(instance.rs:598-635)。GCタスクがアイドル判定で
`instance.close.notify_one()` を呼ぶと(instance.rs:375-381)、`wait_task` が
`child.start_kill()` を実行し(instance.rs:600-604)、`child.wait()` 側の分岐で
インスタンスをマップから除去・クライアントを切断する。**SIGTERM等での猶予は
なく即 SIGKILL** で、`kill_on_drop` も使っていない(wait_task が `Child` を
所有し続けるため取りこぼしはない設計)。

shutdown/exit の伝播は意図的に**遮断**している。クライアントの `shutdown` は
プロキシが横取りして `null` 応答を返し接続を閉じるだけで、サーバには送らない
(`reference/ra-multiplex/src/client.rs:355-365`。他クライアントが接続中のため)。
親死亡検知はデーモンなので存在せず、クライアント側 proxy コマンドは stdio の
EOF(`io::copy_bidirectional` の終了、`src/proxy.rs:64-66`)で自然終了する。

### emacs-lsp-booster(std スレッド、1:1型)

`reference/emacs-lsp-booster/src/app.rs:110-120` で `std::process::Command` を
spawn。stdin/stdout は piped、**stderr は `Stdio::inherit()`** で自前処理なし
(app.rs:113)。

スレッド構成は4本 + `std::sync::mpsc` チャネル2本(app.rs:122-162):

- client読み取り(`process_client_reader`)→ c2s チャネル
- c2s チャネル → server stdin 書き込み(`process_channel_to_writer`)
- server読み取り(`process_server_reader`)→ s2c チャネル
- s2c チャネル → client stdout 書き込み

読みと書きをチャネルで分離しているのは、server→client 方向にバイトコード変換
という重い処理があるためと、後述の backpressure カウンタ(`AtomicI32`)で
「サーバの stdin 書き込みが詰まっている」ことを client 読み取りスレッドから
観測するため(app.rs:23,38,59,122-123)。

終了系の要点:

- メインスレッドは `proc.wait()` でブロックし(app.rs:164)、サーバの exit code
  をそのままプロセスの exit code にする(`src/main.rs:84` の
  `std::process::exit(exit_status.code().unwrap_or(1))`)。
- 親(Emacs)死亡の検知は **stdin EOF 経由の連鎖**: client読みスレッドが
  `rpc_read` の空文字列で break → c2s 送信側 drop → 書き込みスレッド終了 →
  サーバ stdin クローズ → サーバが自主終了 → `proc.wait()` 復帰、という間接
  伝播。サーバが stdin クローズで終了しない場合の kill は**ない**。
- どのスレッドの panic でもプロセス全体を落とすよう panic フックを設定
  (`src/main.rs:59-63`)。スレッド構成では取りこぼしやすいエラー伝播を
  雑だが確実に処理している。

### lsp-devtools(asyncio、graceful kill の参考)

spawn は `asyncio.create_subprocess_exec(..., stdin=PIPE, stdout=PIPE,
stderr=PIPE)`(`lib/lsp-devtools/lsp_devtools/cli/agent.py:59-65`)。stderr は
専用コルーチンで行単位に自分の stderr へ転写する(cli/agent.py:17-25)。

`_watch_server_process` がサーバの `wait()` を待ち、終了したらエージェント全体を
停止する(`agent/agent.py:125-129`)。`stop()` の kill エスカレーションが
3実装中もっとも丁寧(agent.py:131-146):

```python
if self.server.returncode is None:
    try:
        self.server.terminate()                              # SIGTERM
        await asyncio.wait_for(self.server.wait(), timeout=5)
    except TimeoutError:
        self.server.kill()                                   # SIGKILL
```

### lsp-det への含意

- 単一クライアント・単一上流なら booster の「4スレッド+チャネル」でも成立する
  が、readinessゲートの状態管理・タイムアウト・kill エスカレーションを絡めると
  `select!` が使える tokio (current_thread) の方が素直(ra-multiplex 型)。
- 子プロセスの後始末は「`Child` を1タスクが所有して `wait()`」+ 取りこぼし保険に
  `kill_on_drop(true)`(ra-multiplex は未使用だが tokio なら安価に足せる)。
  kill は lsp-devtools 式の terminate→猶予→kill を採る。
- 親死亡検知は3実装とも本質的に「stdin EOF」。lsp-det もこれを主とし、EOF 検知後
  に上流へ shutdown/exit を送る猶予付きシーケンス→タイムアウトで kill、とする。
  booster のような「サーバの善意任せ」はゾンビを生む。
- exit code の伝播(booster main.rs:84)と stderr の扱い(inherit するか行転写
  するか)は初期に決めておく。透過性重視なら inherit が最も安全。

## 3. メッセージのパース深度とバイト保存性

### ra-multiplex: 完全パース + 再シリアライズ

全メッセージを untagged enum に落とす
(`reference/ra-multiplex/src/lsp/jsonrpc.rs:11-18`):

```rust
#[derive(Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum Message {
    Request(Request),
    Notification(Notification),
    ResponseError(ResponseError),
    ResponseSuccess(ResponseSuccess),
}
```

各バリアントは `#[serde(deny_unknown_fields)]` 付き(jsonrpc.rs:21,31,40,48)で、
`params`/`result` は `serde_json::Value` のまま保持する。`id` は
`Number(i64) | String` の untagged enum(jsonrpc.rs:83-89。null id は意図的に
不正扱い)。

バイト保存性への対処は**していない**。転送は常に `serde_json` での再シリアライズ
であり、次の差分が出る:

- オブジェクトのキー順序: `serde_json` の `preserve_order` feature を有効化して
  いないため(`reference/ra-multiplex/Cargo.toml:16`)、`Value` 内のマップは
  BTreeMap でキーがソートされる。
- 数値・エスケープの表現差(`1.0`→`1.0` とは限らない、`\uXXXX` の正規化など)。
- `deny_unknown_fields` により、トップレベルに未知フィールドを持つメッセージや
  `jsonrpc` バージョン不一致はパースエラーになり、読みループはそのメッセージを
  **ログして捨てて続行**する(client.rs:347-350、instance.rs:647-650 の
  `Err(err) => { error!(...); continue; }`)。多重化には id 書き換えが必須なので
  完全パースは避けられないという設計判断。

### emacs-lsp-booster: 方向別の非対称パース

- **client→server**: `rpc_read` が返したボディ `String` をそのままチャネルへ流し
  `rpc_write` する(app.rs:33,58)。**再シリアライズなし=バイト保存**。JSONを
  パースするのは backpressure 上限超過時に reject 応答を作る場合のみ
  (app.rs:38-56)。
- **server→client**: バイトコード変換のため `json::from_str` で `Value` に完全
  パースする(app.rs:75-76)。変換失敗時と `--disable-bytecode` 時は元の文字列を
  そのまま転送する(app.rs:83-90)ので、この経路もフォールバックはバイト保存。

なお reject 経路で使う `LspRequest` は `id: Option<i32>`
(`src/lsp_message.rs:5-11`)で、**LSP仕様が許す文字列 id を受けられない**。
覗き見用の型でも `RequestId` 相当の untagged enum にすべき、という反面教師。

### lsp-devtools: 無パース

前述のとおり転送はチャンク単位の生バイトで、完全にバイト保存
(agent.py:109-115)。パースは観測側の別プロセスで行う。

### lsp-det への含意

透過プロキシの信頼性はバイト保存性で決まる。推奨は:

1. フレーミング層はヘッダのみパースし、ボディは `Vec<u8>` で保持。
2. ルーティング判断(readinessゲート、initialize検出、cancel検出)に必要な
   `method` / `id` だけを浅いパースで覗く。serde_json なら
   `&serde_json::value::RawValue` を使った覗き見専用構造体
   (`struct Peek<'a> { method: Option<&'a str>, id: Option<RawId> }` 相当、
   `deny_unknown_fields` なし)で足りる。
3. 転送は常に**元のボディバイト列**を書く(Content-Length は受信値を再利用
   できるが、ヘッダを作り直すなら `body.len()` から再計算しても同値)。
4. プロキシ自身が合成するメッセージ(ゲート応答等)だけ serde でシリアライズ。

これで ra-multiplex 型の「再シリアライズ差分」「未知フィールドで捨てる」問題を
両方回避できる。

## 4. ra-multiplex の handshake 処理と initialize 前後の通知の扱い

### initialize の横取りフロー

1. クライアント側 proxy コマンドが最初の `initialize` を読み、
   `initializationOptions.lspMux` に接続情報を注入してデーモンへ転送、以後は
   `io::copy_bidirectional` で素通し(`src/proxy.rs:31-67`)。
2. デーモン側 `client::process` は「最初のメッセージは `initialize` リクエストで
   ある」ことを要求し、違えば bail(`src/client.rs:35-43`)。
3. 新規インスタンスなら `initialize_handshake` がサーバと握手する
   (`src/instance.rs:496-543`)。固定文字列 id
   `"lspmux:initialize_request"` で initialize を送り(instance.rs:501-516)、
   応答を待つ。
4. クライアントへは(2番目以降のクライアントならキャッシュ済みの)
   `InitializeResult` で応答し(client.rs:198-206)、続いてクライアントの
   `initialized` 通知を読み捨てる(client.rs:211-221)。サーバへの
   `initialized` は握手時に偽造済み(instance.rs:529-540)。

### issue #89 相当: 順序仮定が壊れる2箇所

握手コードは「次に来るメッセージ」を厳格に決め打ちしており、割り込みメッセージ
で接続ごと失敗する。該当箇所は2つ:

サーバ側(instance.rs:518-526)— **initialize 応答の前に通知やリクエストを送る
サーバ**(`window/logMessage`、`$/progress`、`window/workDoneProgress/create`
など。LSP仕様はこれらを禁止していない)で bail する:

```rust
let res = match reader.read_message().await
    .context("receive initialize response")?
    .context("stream ended")?
{
    Message::ResponseSuccess(res) if res.id == request_id => res,
    _ => bail!("first server message was not initialize response"),
};
```

クライアント側(client.rs:211-221)— initialize 応答直後、`initialized` 以外
(`$/setTrace`、`$/cancelRequest`、早すぎる `didOpen` 等)が来ると bail する:

```rust
match reader.read_message().await ... {
    Message::Notification(notif) if notif.method == "initialized" => {
        // Discard the notification.
    }
    _ => bail!("second client message was not `initialized` notification"),
}
```

(shallow clone のため issue 番号との突合はできないが、issue #89「initialize
応答前に届いた通知でハンドシェイクが失敗する」に対応する実装上の弱点はこの
2箇所である。)

### lsp-det への含意

readinessゲートはまさにこの区間(initialize 送信〜initialized 完了〜ゲート開放)
を扱うので、**「期待外のメッセージ=エラー」にしない**ことが最重要:

- サーバからの握手中メッセージ: initialize 応答以外(通知・リクエスト)は
  そのままクライアントへ転送するか、クライアント未接続段階ならキューする。
- クライアントからの握手中メッセージ: ゲートが開くまで FIFO でバッファし、
  開放後に順序を保って上流へ流す。`exit`/`shutdown` だけは即時処理。
- ra-multiplex が `initialized` を偽造してでも「サーバから見た正規の握手列」を
  守っている点は踏襲する価値がある(上流には常に
  initialize → (応答) → initialized の順で届ける)。

## 5. $/cancelRequest の扱い

- **ra-multiplex**: 専用処理なし。`$/cancelRequest` は一般の通知として素通しする
  (client.rs:406-410 の `Message::Notification(notif) => instance.send_message`)。
  設計メモには「cancel 通知は `id` を含むのでリクエスト同様に多重化できるはず」
  と認識だけ書かれている(`src/lsp.rs:26-29`)。実際にはリクエスト id を
  `client_id:<n>:n:<id>` 形式の文字列にタグ付けして転送するため
  (client.rs:368-373、`src/lsp/ext.rs:22-35` の `RequestId::tag`)、素通しした
  cancel の `params.id` はサーバ側の id と**一致せず、多重化下では実質無効**。
  未解決の既知の穴。
- **emacs-lsp-booster**: `$/cancelRequest` の処理なし(素通し)。id を書き換え
  ないので素通しで正しく機能する。関連機能として、上流が詰まっているとき
  (未処理128件超、app.rs:23)に新規リクエストをサーバへ送らず error code
  `-32803`(ServerCancelled)の偽応答を返す backpressure 制御がある
  (app.rs:38-56)。cancel の転送ではなく「プロキシによる先回り拒否」。
- **lsp-devtools**: 生バイト中継なので該当処理なし(そのまま流れる)。

### lsp-det への含意

id を書き換えない単一クライアント構成では cancel は素通しが正解。ただし
readinessゲートで**保留中のリクエストに対する cancel** が来た場合だけは、
(a) ゲート開放時に「リクエスト→cancel」の順序を保って両方流す、または
(b) 保留キューから対象を取り除き `-32800`(RequestCancelled)応答を自前で返す、
のどちらかを明示的に選ぶ必要がある。(a) が実装最小・透過性最大で推奨。
booster の `-32803` 偽応答は「ゲートが長時間開かない場合の防衛」の参考になる。

## 6. lsp-det への推奨事項

### 依存クレート

| クレート | 採否 | 理由 |
| --- | --- | --- |
| tokio(`io-std`, `io-util`, `process`, `sync`, `time`, `rt`, `macros`) | 採用 | 子プロセス wait / stdio / タイムアウト / ゲート状態を `select!` で合成できる。ra-multiplex と同じ feature 構成(Cargo.toml:18)が実績 |
| serde + serde_json | 採用 | 覗き見パースと合成メッセージ用。`RawValue` 利用。転送経路では使わない |
| anyhow | 採用 | 2つのRust実装とも採用。エラー文脈付与(`context`)が読み手コードで効く |
| tracing / env_logger | どちらか | stderr へのログは必須(stdout は絶対に汚さない) |
| lsp-types 等の型クレート | 不採用 | 覗くのは method/id と initialize の一部のみ。全型は過剰依存 |

グローバル指針の「標準ライブラリ優先」に照らすと booster 型の std スレッド構成も
候補だが、readinessゲート(タイマー・状態遷移・kill エスカレーション)を
スレッド+チャネルで組むと同期プリミティブが増えて可読性が落ちる。tokio
current_thread(main.rs:56 と同じ)を「構造的解決」として採る方が保守コストは
低い、という判断を推奨する。

### タスク構成(tokio current_thread)

```text
main
 ├─ client_reader : stdin  → フレーム読み → peek → ルータへ (mpsc)
 ├─ server_reader : 子stdout → フレーム読み → peek → ルータへ (mpsc)
 ├─ router        : readinessゲートの状態機械。保留キューを持ち、
 │                  宛先チャネル(server_writer / client_writer)へ振り分け
 ├─ server_writer : mpsc → 子stdin(書き込み+flush を直列化)
 ├─ client_writer : mpsc → stdout(同上)
 ├─ stderr_pump   : 子stderr → 自stderr(または Stdio::inherit で省略)
 └─ waiter        : child.wait() + shutdownシグナル(select!)
```

- チャネルは有界(ra-multiplex は 64/16、booster は実質128)。単一クライアント
  なら 64 程度で十分。満杯時は await で自然に backpressure がかかる。
- 書き込みタスクを分離するのは両Rust実装共通のパターン。フレーム書き込みの
  アトミック性(ヘッダとボディの間に他メッセージが割り込まない)が構造的に
  保証される。

### 実装上の注意点(本調査からの教訓)

1. **フレーミング**: `BufReader` + `read_until` + `read_exact`。ヘッダ名は ASCII
   小文字化して比較、`Content-Length` 必須、未知ヘッダは無視(bail しない)、
   `content-type` は転送しない。異常な Content-Length への上限チェックを追加。
2. **バイト保存**: 転送ボディは受信バイト列をそのまま書く。再シリアライズは
   合成メッセージのみ。覗き見構造体に `deny_unknown_fields` を付けない。
3. **id 型**: `enum RequestId { Number(i64), String(String) }`(untagged)。
   booster の `Option<i32>` は文字列 id で壊れる反面教師。
4. **ゲート区間の順序仮定を置かない**: 「次は必ず initialize 応答」「次は必ず
   initialized」と決め打ちしない(ra-multiplex の bail 2箇所が反例)。想定外
   メッセージは転送またはFIFOバッファ。
5. **shutdown/exit**: 単一クライアントなので横取り不要、素通しでよい
   (ra-multiplex の横取りは多重化都合)。ただし stdin EOF 検知時は自前で
   shutdown → exit を上流へ送り、猶予付きで `terminate → wait(5s) → kill`
   (lsp-devtools 式)。`kill_on_drop(true)` を保険に付ける。
6. **flush**: メッセージ書き込みごとに必ず flush(transport.rs:190、
   rpcio.rs:39)。BufWriter に溜めたままにすると相手が固まる。
7. **exit code / stderr**: サーバの exit code をプロキシの exit code として伝播
   (booster main.rs:84)。stderr は `Stdio::inherit` が最も透過的。
8. **エラーで読みループを止めない**: パース不能メッセージは「ログして転送」
   (透過プロキシなので捨てる ra-multiplex 方式より強い)。ただしフレーミング
   自体が壊れた場合は回復不能なので接続を落とす。
