//! クライアントと上流言語サーバーを中継するイベントループ (v0.1-design.md 4.8)。
//!
//! M1 の範囲は純粋な素通し (ゲートなし)。全状態はこのモジュールの
//! 単一ループに閉じ、ロックを持たない。読み取りは std スレッド + `mpsc`
//! で行い、判断はループ内でのみ行う。

use std::io::{self, BufReader, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::adapter::RustAnalyzerAdapter;
use crate::framing::{self, RawMessage};
use crate::initialize;
use crate::peek::{self, RequestId};
use crate::process;
use crate::state::{self, ServerState, ServerStateProvider};
use crate::tracker::Tracker;

/// 上流の生死をポーリングする間隔。`Upstream` の所有権 (kill する権利) を
/// main ループ 1 箇所に保つため、別スレッドで `wait()` させず `recv_timeout`
/// のたびに `try_wait()` する (v0.1-design.md 4.8: タイマーは `recv_timeout`)。
const UPSTREAM_POLL_INTERVAL: Duration = Duration::from_millis(20);

enum Event {
    FromClient(RawMessage),
    ClientClosed,
    ClientReadError(io::Error),
    FromUpstream(RawMessage),
    /// 上流の stdout が閉じた。生死のポーリング (`try_wait`) は
    /// `recv_timeout` のタイムアウト時にしか回らないため、クライアントが
    /// 絶え間なく喋っていると死の検出が遅れる。読み手が明示的に知らせる。
    UpstreamClosed,
}

/// クライアントと上流を中継し、プロキシ自身の終了コードを返す。
///
/// `adapter` を渡すと上流の状態を追跡する (v0.1-design.md 5 章)。
/// `None` は純透過 (アダプタなし)。
pub fn run<R, W>(
    client_in: R,
    client_out: W,
    command: &str,
    args: &[String],
    adapter: Option<RustAnalyzerAdapter>,
) -> io::Result<i32>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let mut surface = Surface::new(adapter);
    let mut handles = process::spawn(command, args)?;
    let mut upstream_stdin = handles.stdin;
    let mut client_out = client_out;

    let (tx, rx) = mpsc::channel::<Event>();

    spawn_stderr_relay(handles.stderr);
    spawn_client_reader(client_in, tx.clone());
    spawn_upstream_reader(handles.stdout, tx);

    let exit_code = loop {
        match rx.recv_timeout(UPSTREAM_POLL_INTERVAL) {
            Ok(Event::FromClient(msg)) => match surface.on_client(msg) {
                // 拡張 S のリクエストは中継層が自ら答える。上流は
                // 拡張 S を知らないので転送してはならない (仕様 2 章)。
                ClientAction::AnswerLocally(response) => {
                    let _ = framing::write_message(&mut client_out, &response);
                }
                ClientAction::Forward(msg) => {
                    // 上流の stdin が閉じていても続行する。
                    // 次のポーリングで exit を検出する。
                    let _ = framing::write_message(&mut upstream_stdin, &msg);
                }
            },
            Ok(Event::FromUpstream(msg)) => {
                let (msg, notification) = surface.on_upstream(msg);
                if framing::write_message(&mut client_out, &msg).is_err() {
                    // クライアントが既に読み取りをやめている。無視して続行する
                    // (次のポーリングで上流の exit を検出する)。
                }
                if let Some(notification) = notification {
                    let _ = framing::write_message(&mut client_out, &notification);
                }
            }
            Ok(Event::UpstreamClosed) => {
                // stdout を閉じた上流はもう応答しない。クライアントが
                // 喋り続けていてもここで死を伝える。
                announce_death(&mut surface, &mut client_out);
                break reap(&mut handles.upstream);
            }
            Ok(Event::ClientClosed) => {
                eprintln!("lsp-det: client closed connection, terminating upstream");
                let _ = handles.upstream.kill_and_wait();
                break 0;
            }
            Ok(Event::ClientReadError(err)) => {
                eprintln!("lsp-det: error reading from client: {err}");
                let _ = handles.upstream.kill_and_wait();
                break 0;
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Ok(Some(status)) = handles.upstream.try_wait() {
                    let code = status.code().unwrap_or(1);
                    if code != 0 {
                        eprintln!("lsp-det: upstream exited with status {code}");
                    }
                    announce_death(&mut surface, &mut client_out);
                    break code;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                // client_reader・upstream_reader の両方が終了済み。
                // 上流の最終状態を確認して終了する。
                let code = handles
                    .upstream
                    .wait()
                    .ok()
                    .and_then(|s| s.code())
                    .unwrap_or(1);
                announce_death(&mut surface, &mut client_out);
                break code;
            }
        }
    };

    let _ = client_out.flush();
    Ok(exit_code)
}

/// クライアントから来たメッセージをどう扱うか。
enum ClientAction {
    /// そのまま (あるいは書き換えて) 上流へ流す。
    Forward(RawMessage),
    /// 中継層が自ら応答する。上流へは流さない。
    AnswerLocally(RawMessage),
}

/// クライアントから見た `initialize` リクエストの種類。
///
/// 覗き見の借用を `RawMessage` の所有権から切り離すために、判定結果だけを
/// 所有データとして取り出す。
enum ClientKind {
    ServerStateRequest(RequestId),
    InitializeRequest(Option<RequestId>),
    Other,
}

/// 終了コードを取り、居座る上流は道連れにする。
///
/// stdout を閉じてから実際に exit するまでには僅かな間があるため、
/// 即断せず短く待つ。待ち切っても終わらない上流は kill する
/// (プロキシが吊られるとクライアントも吊られる)。
fn reap(upstream: &mut process::Upstream) -> i32 {
    const ATTEMPTS: u32 = 50; // 20ms x 50 = 最大 1 秒
    for _ in 0..ATTEMPTS {
        match upstream.try_wait() {
            Ok(Some(status)) => return status.code().unwrap_or(1),
            Ok(None) => thread::sleep(UPSTREAM_POLL_INTERVAL),
            Err(_) => break,
        }
    }
    eprintln!("lsp-det: upstream closed stdout but did not exit; killing it");
    let _ = upstream.kill_and_wait();
    1
}

/// 上流の死をクライアントへ知らせる (仕様 6.1)。
///
/// ループを抜ける前に書き切る必要がある。抜けた後では `client_out` が
/// 閉じ、死を伝えられないまま沈黙することになる。
fn announce_death<W: Write>(surface: &mut Surface, client_out: &mut W) {
    if let Some(notification) = surface.mark_dead() {
        let _ = framing::write_message(client_out, &notification);
    }
}

/// 拡張 S のサーバー側 surface (v0.1-design.md 4.1)。
///
/// アダプタの有無によらず存在する。アダプタなしでは両軸 `unknown` と
/// 消失時の `dead` だけを出す (ADR 0008)。
struct Surface {
    tracker: StateTracker,
    provider: ServerStateProvider,
    /// クライアントが仕様 5.2 の宣言をしたか。通知を送る条件。
    client_declared: bool,
    initialize_id: Option<RequestId>,
    /// `InitializeResult` を転送済みか。これより前に通知を送ると
    /// handshake を壊す (7 章チェックリスト #1 と同種の事故)。
    handshake_done: bool,
    /// handshake 前に起きた状態変化。`InitializeResult` の直後に送る。
    ///
    /// 送れないからと捨ててはならない。アダプタ側の状態は既に進んでおり、
    /// 同じ状態は二度と通知されない (仕様 4.2 の重複抑止)。捨てるとその
    /// 遷移は永久に失われ、通知だけを見ているクライアントは初期状態のまま
    /// 取り残される。
    pending_state: Option<ServerState>,
}

impl Surface {
    fn new(adapter: Option<RustAnalyzerAdapter>) -> Self {
        let tracker = StateTracker::new(adapter);
        let provider = tracker.provider();
        Surface {
            tracker,
            provider,
            client_declared: false,
            initialize_id: None,
            handshake_done: false,
            pending_state: None,
        }
    }

    fn on_client(&mut self, msg: RawMessage) -> ClientAction {
        let kind = match peek::peek(&msg.body) {
            Ok(view) if view.is_request() => match (view.method(), view.id.clone()) {
                (Some(state::SERVER_STATE_METHOD), Some(id)) => ClientKind::ServerStateRequest(id),
                (Some("initialize"), id) => ClientKind::InitializeRequest(id),
                _ => ClientKind::Other,
            },
            _ => ClientKind::Other,
        };

        match kind {
            ClientKind::ServerStateRequest(id) => {
                // 仕様 5.2: このリクエストは宣言の有無によらず応答する。
                ClientAction::AnswerLocally(self.state_response(&id))
            }
            ClientKind::InitializeRequest(id) => {
                self.initialize_id = id;
                self.client_declared = initialize::client_declares_server_state(&msg.body);
                let injected = initialize::inject_client_capabilities(
                    &msg.body,
                    self.tracker.required_client_capabilities(),
                );
                ClientAction::Forward(match injected {
                    Some(body) => RawMessage { body },
                    None => msg,
                })
            }
            ClientKind::Other => ClientAction::Forward(msg),
        }
    }

    /// 上流のメッセージを観測し、(転送するメッセージ, 付随して送る通知) を返す。
    fn on_upstream(&mut self, msg: RawMessage) -> (RawMessage, Option<RawMessage>) {
        let is_initialize_response = match peek::peek(&msg.body) {
            Ok(view) => {
                view.method().is_none()
                    && view.id.is_some()
                    && view.id == self.initialize_id
                    && !self.handshake_done
            }
            Err(_) => false,
        };

        if is_initialize_response {
            self.handshake_done = true;
            let forwarded =
                match initialize::declare_server_state_provider(&msg.body, &self.provider) {
                    Some(body) => RawMessage { body },
                    None => {
                        // 応答の形が想定外 (result が無い / capabilities が
                        // オブジェクトでない等)。宣言できないまま拡張 S として
                        // 振る舞うことになるので、黙って進まず理由を残す。
                        eprintln!(
                            "lsp-det: cannot declare serverStateProvider; \
                         the upstream InitializeResult has an unexpected shape"
                        );
                        msg
                    }
                };
            // handshake 前に溜まった遷移をここで 1 通だけ流す。
            let flushed = self
                .pending_state
                .take()
                .filter(|_| self.client_declared)
                .map(|state| changed_notification(&state));
            return (forwarded, flushed);
        }

        let changed = match peek::peek(&msg.body) {
            Ok(view) => self.tracker.observe(&view, &msg.body),
            Err(_) => None,
        };
        let notification = changed.and_then(|state| self.notify_or_stash(state));
        (msg, notification)
    }

    /// 上流の死を伝えるメッセージ。handshake の前後で手段が変わる。
    fn mark_dead(&mut self) -> Option<RawMessage> {
        let state = self.tracker.mark_dead()?;
        if !self.handshake_done {
            // 通知は送れない (LSP は InitializeResult より前のサーバー発
            // 通知を許さない)。宙に浮いた initialize をエラーで閉じる。
            // これをしないとクライアントは応答を永久に待つ。
            return self.initialize_id.take().map(|id| initialize_failed(&id));
        }
        self.notify_or_stash(state)
    }

    /// 仕様 4.2 の通知を作る。宣言していないクライアントには送らない
    /// (仕様 5.2)。handshake 前なら送らずに溜める。
    fn notify_or_stash(&mut self, state: ServerState) -> Option<RawMessage> {
        if !self.client_declared {
            return None;
        }
        if !self.handshake_done {
            self.pending_state = Some(state);
            return None;
        }
        Some(changed_notification(&state))
    }

    fn state_response(&self, id: &RequestId) -> RawMessage {
        // 宣言の有無によらず応答する (仕様 5.2)。
        RawMessage {
            body: serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": self.tracker.state(),
            }))
            .expect("ServerState は常にシリアライズできる"),
        }
    }
}

fn changed_notification(state: &ServerState) -> RawMessage {
    RawMessage {
        body: serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": state::SERVER_STATE_CHANGED_METHOD,
            "params": state,
        }))
        .expect("ServerState は常にシリアライズできる"),
    }
}

/// 上流が `initialize` に答えないまま消えたときの応答。
///
/// 沈黙させるとクライアントは応答を永久に待つ。死を隠さないという
/// 方針 (設計 4.2) は handshake 中にも適用される。
fn initialize_failed(id: &RequestId) -> RawMessage {
    RawMessage {
        body: serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32603, // JSON-RPC InternalError
                "message": "lsp-det: the upstream language server exited before answering initialize",
            },
        }))
        .expect("固定の構造なので常にシリアライズできる"),
    }
}

/// 上流の状態追跡と、その遷移の記録。
///
/// 遷移の時刻と直前の状態の滞在時間を stderr に出す。ゲート導入後は
/// 「どの状態にどれだけ留まったか」が保留時間そのものになるため、
/// 待たされたときに原因を追える唯一の記録になる。
struct StateTracker {
    tracker: Tracker,
    started: Instant,
    entered_state: Instant,
}

impl StateTracker {
    fn new(adapter: Option<RustAnalyzerAdapter>) -> Self {
        let now = Instant::now();
        let mut tracker = StateTracker {
            tracker: Tracker::new(adapter),
            started: now,
            entered_state: now,
        };
        // 開始状態を最初の 1 行に出す。これがないと滞在時間の系列が
        // 起点を失い、フラップの実測に使えない。
        let initial = tracker.tracker.state().clone();
        tracker.log(&initial);
        tracker
    }

    fn state(&self) -> &ServerState {
        self.tracker.state()
    }

    fn provider(&self) -> ServerStateProvider {
        self.tracker.provider()
    }

    fn required_client_capabilities(&self) -> &'static [&'static str] {
        self.tracker.required_client_capabilities()
    }

    /// 状態が変わったらログして新しい状態を返す。
    fn observe(&mut self, view: &peek::MessageView, body: &[u8]) -> Option<ServerState> {
        let state = self.tracker.observe_upstream(view, body)?;
        self.log(&state);
        Some(state)
    }

    fn mark_dead(&mut self) -> Option<ServerState> {
        let state = self.tracker.mark_dead()?;
        self.log(&state);
        Some(state)
    }

    fn log(&mut self, state: &ServerState) {
        let now = Instant::now();
        let rendered =
            serde_json::to_string(state).unwrap_or_else(|_| "<unserializable>".to_string());
        eprintln!(
            "lsp-det: [{:.3}s] server state -> {rendered} (previous held {:.3}s)",
            now.duration_since(self.started).as_secs_f64(),
            now.duration_since(self.entered_state).as_secs_f64(),
        );
        self.entered_state = now;
    }
}

fn spawn_stderr_relay(stderr: std::process::ChildStderr) {
    thread::spawn(move || {
        let mut reader = stderr;
        let mut stderr_out = io::stderr();
        let _ = io::copy(&mut reader, &mut stderr_out);
    });
}

fn spawn_client_reader<R>(client_in: R, tx: Sender<Event>)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(client_in);
        loop {
            match framing::read_message(&mut reader) {
                Ok(Some(msg)) => {
                    if tx.send(Event::FromClient(msg)).is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    let _ = tx.send(Event::ClientClosed);
                    return;
                }
                Err(err) => {
                    let _ = tx.send(Event::ClientReadError(io::Error::other(err)));
                    return;
                }
            }
        }
    });
}

fn spawn_upstream_reader(stdout: std::process::ChildStdout, tx: Sender<Event>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match framing::read_message(&mut reader) {
                Ok(Some(msg)) => {
                    if tx.send(Event::FromUpstream(msg)).is_err() {
                        return;
                    }
                }
                Ok(None) | Err(_) => {
                    let _ = tx.send(Event::UpstreamClosed);
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::write_message;
    use std::time::Duration;

    /// `cat` を上流にすると client -> proxy -> cat -> proxy -> client と
    /// 往復するため、「プロキシが上流へ書いたバイト列」をそのまま観測できる。
    fn spawn_with_cat(
        adapter: Option<RustAnalyzerAdapter>,
    ) -> (
        io::PipeWriter,
        BufReader<io::PipeReader>,
        thread::JoinHandle<i32>,
    ) {
        let (client_out_reader, client_out_writer) = io::pipe().unwrap();
        let (client_in_reader, client_in_writer) = io::pipe().unwrap();
        let handle = thread::spawn(move || {
            run(client_in_reader, client_out_writer, "cat", &[], adapter).unwrap()
        });
        (client_in_writer, BufReader::new(client_out_reader), handle)
    }

    fn send(writer: &mut io::PipeWriter, body: &str) {
        write_message(
            writer,
            &RawMessage {
                body: body.as_bytes().to_vec(),
            },
        )
        .unwrap();
    }

    #[test]
    fn injects_the_status_capability_into_the_initialize_it_forwards() {
        // 設計 4.5: rust-analyzer は宣言がないと serverStatus を送らない。
        let (mut client_in, mut client_out, handle) =
            spawn_with_cat(Some(RustAnalyzerAdapter::new()));

        send(
            &mut client_in,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
        );

        let forwarded = framing::read_message(&mut client_out).unwrap().unwrap();
        let value: serde_json::Value = serde_json::from_slice(&forwarded.body).unwrap();
        assert_eq!(
            value["params"]["capabilities"]["experimental"]["serverStatusNotification"],
            serde_json::Value::Bool(true)
        );

        drop(client_in);
        handle.join().unwrap();
    }

    #[test]
    fn leaves_the_initialize_alone_when_no_adapter_is_selected() {
        let (mut client_in, mut client_out, handle) = spawn_with_cat(None);

        let original =
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
        send(&mut client_in, original);

        let forwarded = framing::read_message(&mut client_out).unwrap().unwrap();
        assert_eq!(forwarded.body, original.as_bytes());

        drop(client_in);
        handle.join().unwrap();
    }

    #[test]
    fn forwards_observed_upstream_messages_byte_for_byte() {
        // 状態を追跡しても、クライアントへ届くのは原文のまま (設計 4.6)。
        let (mut client_in, mut client_out, handle) =
            spawn_with_cat(Some(RustAnalyzerAdapter::new()));

        let status = r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true,"message":null}}"#;
        send(&mut client_in, status);

        let forwarded = framing::read_message(&mut client_out).unwrap().unwrap();
        assert_eq!(forwarded.body, status.as_bytes());

        drop(client_in);
        handle.join().unwrap();
    }

    #[test]
    fn only_rewrites_the_initialize_request() {
        // 同じ method でも通知なら書き換えない。
        let (mut client_in, mut client_out, handle) =
            spawn_with_cat(Some(RustAnalyzerAdapter::new()));

        let notification =
            r#"{"jsonrpc":"2.0","method":"initialize","params":{"capabilities":{}}}"#;
        send(&mut client_in, notification);

        let forwarded = framing::read_message(&mut client_out).unwrap().unwrap();
        assert_eq!(forwarded.body, notification.as_bytes());

        drop(client_in);
        handle.join().unwrap();
    }

    #[test]
    fn round_trips_a_message_through_a_real_upstream_process() {
        // upstream に `cat` を使う: client -> proxy -> cat(echo) -> proxy -> client
        // というラウンドトリップで、バイト列が非破壊で往復することを検証する。
        let (client_out_reader, client_out_writer) = io::pipe().unwrap();
        let (client_in_reader, mut client_in_writer) = io::pipe().unwrap();

        let handle = thread::spawn(move || {
            run(client_in_reader, client_out_writer, "cat", &[], None).unwrap()
        });

        let sent = RawMessage {
            body: br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_vec(),
        };
        write_message(&mut client_in_writer, &sent).unwrap();

        let mut reader = BufReader::new(client_out_reader);
        let received = framing::read_message(&mut reader).unwrap().unwrap();
        assert_eq!(received.body, sent.body);

        drop(client_in_writer); // クライアント切断 -> プロキシは終了するはず
        let code = handle.join().unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn propagates_upstream_exit_code_to_client() {
        let (client_out_reader, client_out_writer) = io::pipe().unwrap();
        let (client_in_reader, client_in_writer) = io::pipe().unwrap();

        let handle = thread::spawn(move || {
            run(
                client_in_reader,
                client_out_writer,
                "sh",
                &["-c".to_string(), "exit 7".to_string()],
                None,
            )
            .unwrap()
        });

        // client_in_writer を drop せず保持したまま upstream が自然終了するのを待つ。
        let code = handle.join().unwrap();
        assert_eq!(code, 7);
        drop(client_in_writer);

        // client_out はプロキシの終了とともに閉じられ、読み取り側は EOF になる。
        let mut buf = Vec::new();
        let mut reader = client_out_reader;
        reader.read_to_end(&mut buf).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn client_disconnect_kills_upstream_and_exits_cleanly() {
        let (client_out_reader, client_out_writer) = io::pipe().unwrap();
        let (client_in_reader, client_in_writer) = io::pipe().unwrap();
        drop(client_out_reader); // クライアント側は読まない (関心の対象外)

        let handle = thread::spawn(move || {
            run(
                client_in_reader,
                client_out_writer,
                "sleep",
                &["30".to_string()],
                None,
            )
            .unwrap()
        });

        drop(client_in_writer); // クライアントが接続を切る

        // プロキシは上流(sleep 30)を kill して速やかに終了するはず。
        // 30秒待たされたら kill が効いていない。
        let start = std::time::Instant::now();
        let code = handle.join().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "proxy should kill upstream promptly on client disconnect"
        );
        assert_eq!(code, 0);
    }
}
