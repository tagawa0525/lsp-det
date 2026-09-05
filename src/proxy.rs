//! クライアントと上流言語サーバーを中継するイベントループ (v0.1-design.md 4.6)。
//!
//! 全状態はこのモジュールの単一ループに閉じ、ロックを持たない。読み取りは
//! std スレッド + `mpsc` で行い、判断はループ内でのみ行う。上流側
//! ([`UpstreamSide`]) がここに住む。下流側 (M3) も同じループに載せる。

use std::io::{self, BufReader, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::adapter;
use crate::framing::{self, RawMessage};
use crate::gate::{self, Decision, DrainReason, Gate, Release};
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
/// 写像は上流が `InitializeResult.serverInfo` で名乗る名前で選ぶ
/// (v0.1-design.md 4.2)。既知でなければ上流側は保証なしで宣言し、両軸
/// `unknown` を報告する (仕様 8.2 の 3)。上流の消失は通知ではなく接続の
/// 終了で伝える (仕様 8.2 の 7)。下流側 ([`Gate`]) は境界の上の状態を見て
/// 横断リクエストを代行する (設計 4.3)。
pub fn run<R, W>(client_in: R, client_out: W, command: &str, args: &[String]) -> io::Result<i32>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let mut upstream_side = UpstreamSide::new();
    let mut gate = Gate::new();
    let mut handles = process::spawn(command, args)?;
    let mut upstream_stdin = handles.stdin;
    let mut client_out = client_out;

    let (tx, rx) = mpsc::channel::<Event>();

    spawn_stderr_relay(handles.stderr);
    spawn_client_reader(client_in, tx.clone());
    spawn_upstream_reader(handles.stdout, tx);

    // 書き込みの失敗は無視して続行する。クライアントが読み取りをやめていても、
    // 上流の stdin が閉じていても、次のポーリングで exit を検出する。
    let mut deliver = |outs: Vec<Out>| {
        for out in outs {
            match out {
                Out::ToClient(msg) => {
                    let _ = framing::write_message(&mut client_out, &msg);
                }
                Out::ToUpstream(msg) => {
                    let _ = framing::write_message(&mut upstream_stdin, &msg);
                }
            }
        }
    };

    let exit_code = loop {
        match rx.recv_timeout(UPSTREAM_POLL_INTERVAL) {
            Ok(Event::FromClient(msg)) => deliver(upstream_side.on_client(msg, &mut gate)),
            Ok(Event::FromUpstream(msg)) => deliver(upstream_side.on_upstream(msg, &mut gate)),
            Ok(Event::UpstreamClosed) => {
                // stdout を閉じた上流はもう応答しない。クライアントが
                // 喋り続けていてもここで閉じる。
                deliver(close_pending(&mut upstream_side, &mut gate));
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
                    deliver(close_pending(&mut upstream_side, &mut gate));
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
                deliver(close_pending(&mut upstream_side, &mut gate));
                break code;
            }
        }
    };

    let _ = client_out.flush();
    Ok(exit_code)
}

/// 中継の出力。どちらへ書くか。
enum Out {
    ToClient(RawMessage),
    ToUpstream(RawMessage),
}

/// クライアントから来たメッセージの種類。
///
/// 覗き見の借用を `RawMessage` の所有権から切り離すために、判定結果だけを
/// 所有データとして取り出す。
enum ClientKind {
    ServerStateRequest(RequestId),
    InitializeRequest(Option<RequestId>),
    ShutdownRequest,
    CancelRequest(RequestId),
    Request {
        id: RequestId,
        method: String,
    },
    /// クライアントの `initialized` 通知。恒等写像の初期状態の問い合わせは
    /// これを流した後に送る。
    Initialized,
    Other,
}

/// 注入した `window.workDoneProgress` に由来するサーバー発リクエスト。
/// クライアントが自分で宣言していなければ上流側が答える (設計 4.2)。
const WORK_DONE_PROGRESS_CREATE: &str = "window/workDoneProgress/create";

/// 上流への `initialize` に注入する、本プロトコルの購読宣言 (設計 4.2)。
/// 上流が自ら本プロトコルを話すとき、その通知を下流側が読むために要る。
/// クライアントが宣言していなければ、届いた通知はクライアントへ流さない。
const SERVER_STATE_CLIENT_CAPABILITY: &str = "experimental.serverState";

/// 恒等写像のとき、上流の初期状態を lsp-det 自身が問い合わせるリクエストの
/// id。上流の通知は変化のときにしか来ないので、最初の状態は聞くしかない。
/// JSON-RPC の id は文字列も許すので衝突しない保証はないが、`lsp-det:` を
/// 予約 prefix として使うことで実用上衝突しない。
const SELF_STATE_REQUEST_ID: &str = "lsp-det:serverState";

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

/// 上流が消えた。未応答のリクエストにエラーを応答してから接続を閉じる
/// (仕様 8.2 の 7、設計 4.2「上流の消失」)。`initialize` に答えないまま
/// 消えたならそれも、下流側が保留していた横断リクエストもすべて閉じる。
///
/// 死を表す通知は送らない。プロセスの消失は本プロトコルの値ではなく、
/// この後の EOF が伝える。ループを抜ける前に書き切る必要がある。
fn close_pending(upstream_side: &mut UpstreamSide, gate: &mut Gate) -> Vec<Out> {
    let mut outs = Vec::new();
    if let Some(response) = upstream_side.fail_pending_initialize() {
        outs.push(Out::ToClient(response));
    }
    outs.extend(
        gate.drain(DrainReason::UpstreamExited)
            .into_iter()
            .map(Out::ToClient),
    );
    outs
}

/// 上流側: 言語サーバーを代行して本プロトコルを提供する (v0.1-design.md 4.2)。
///
/// アダプタの有無によらず存在する。アダプタなしでは両軸 `unknown` を
/// 報告する (仕様 8.2 の 3)。境界の上の状態 ([`Self::boundary_state`]) を
/// 下流側に渡す役目も持つ。
struct UpstreamSide {
    tracker: StateTracker,
    /// クライアントが仕様 5.2 の宣言をしたか。通知を送る条件。
    client_declared: bool,
    /// クライアントが `window.workDoneProgress` を自分で宣言していたか。
    /// していなければ `window/workDoneProgress/create` は上流側が答える。
    client_declared_progress: bool,
    initialize_id: Option<RequestId>,
    /// `InitializeResult` を転送済みか。写像の選択もこの時点で行うので、
    /// これより前に状態が動くことはない (LSP はサーバー発の通知を
    /// `InitializeResult` の後に限っている)。
    handshake_done: bool,
    /// 上流自身が本プロトコルを宣言している (`InitializeResult` に
    /// `serverStateProvider` がある)。以後、上流側は恒等写像になる: 宣言を
    /// 足さず、リクエストを転送し、自前の通知を出さない。同一接続に送信者の
    /// 異なる 2 系統を流さないため (仕様 8.2 の 6、ADR 0009 決定 D-1)。
    identity: bool,
    /// 恒等写像のときの境界の状態。上流の `serverStateChanged` と、lsp-det
    /// 自身の問い合わせの応答で更新する。最初の応答が届くまでは
    /// 「initialize 直後」の `initializing` (下流側は待つ)。
    identity_state: ServerState,
    /// 恒等写像になったが、初期状態の問い合わせをまだ送っていない。送るのは
    /// クライアントの `initialized` を上流へ流した後 (LSP はサーバーが
    /// `initialized` まで他のリクエストを受けないことを許し、rust-analyzer は
    /// 規約違反として終了する)。
    identity_query_pending: bool,
}

impl UpstreamSide {
    fn new() -> Self {
        UpstreamSide {
            tracker: StateTracker::new(),
            client_declared: false,
            client_declared_progress: false,
            initialize_id: None,
            handshake_done: false,
            identity: false,
            identity_state: ServerState::initializing(),
            identity_query_pending: false,
        }
    }

    /// 境界の上の状態。下流側はこれだけを見る (設計 4.1)。
    fn boundary_state(&self) -> &ServerState {
        if self.identity {
            &self.identity_state
        } else {
            self.tracker.state()
        }
    }

    fn on_client(&mut self, msg: RawMessage, gate: &mut Gate) -> Vec<Out> {
        let kind = match peek::peek(&msg.body) {
            Ok(view) if view.is_request() => match (view.method(), view.id.clone()) {
                (Some(state::SERVER_STATE_METHOD), Some(id)) => ClientKind::ServerStateRequest(id),
                (Some("initialize"), id) => ClientKind::InitializeRequest(id),
                (Some("shutdown"), _) => ClientKind::ShutdownRequest,
                (Some(method), Some(id)) => ClientKind::Request {
                    id,
                    method: method.to_string(),
                },
                _ => ClientKind::Other,
            },
            Ok(view) if view.is_notification() && view.method() == Some("initialized") => {
                ClientKind::Initialized
            }
            Ok(view) if view.is_notification() && view.method() == Some("$/cancelRequest") => {
                match gate::cancel_target(&msg.body) {
                    Some(id) => ClientKind::CancelRequest(id),
                    None => ClientKind::Other,
                }
            }
            _ => ClientKind::Other,
        };

        match kind {
            ClientKind::ServerStateRequest(id) => {
                if self.identity {
                    // 上流が本プロトコルを話す。上流の仕事 (仕様 8.2 の 6)。
                    vec![Out::ToUpstream(msg)]
                } else {
                    // 仕様 5.2: このリクエストは宣言の有無によらず応答する。
                    vec![Out::ToClient(self.state_response(&id))]
                }
            }
            ClientKind::InitializeRequest(id) => {
                self.initialize_id = id;
                self.tracker.remember_initialize(&msg.body);
                self.client_declared = initialize::client_declares_server_state(&msg.body);
                self.client_declared_progress =
                    initialize::client_declares_work_done_progress(&msg.body);
                gate.set_client_decides(self.client_declared);
                // 上流が誰かはまだ分からない。既知の写像ぶんと、本プロトコルの
                // 購読宣言を全部注入する。
                let mut paths: Vec<&str> = adapter::CLIENT_CAPABILITIES_FOR_ALL_MAPPINGS.to_vec();
                paths.push(SERVER_STATE_CLIENT_CAPABILITY);
                let injected = initialize::inject_client_capabilities(&msg.body, &paths);
                vec![Out::ToUpstream(match injected {
                    Some(body) => RawMessage { body },
                    None => msg,
                })]
            }
            ClientKind::ShutdownRequest => {
                // 保留分すべてにエラーを応答してから shutdown を流す
                // (仕様 9 章 6 項)。応答を返さないリクエストを作らない。
                let mut outs: Vec<Out> = gate
                    .drain(DrainReason::Shutdown)
                    .into_iter()
                    .map(Out::ToClient)
                    .collect();
                outs.push(Out::ToUpstream(msg));
                outs
            }
            ClientKind::CancelRequest(id) => match gate.on_cancel(&id) {
                // 保留中だった。キューから外して応答し、上流には送らない。
                Some(response) => vec![Out::ToClient(response)],
                None => vec![Out::ToUpstream(msg)],
            },
            ClientKind::Request { id, method } => {
                match gate.on_request(msg, id, &method, self.boundary_state()) {
                    Decision::Forward(msg) => vec![Out::ToUpstream(msg)],
                    Decision::Held => Vec::new(),
                    Decision::Reject(response) => vec![Out::ToClient(response)],
                }
            }
            ClientKind::Initialized => {
                let mut outs = vec![Out::ToUpstream(msg)];
                if self.identity_query_pending {
                    self.identity_query_pending = false;
                    outs.push(Out::ToUpstream(self_state_request()));
                }
                outs
            }
            ClientKind::Other => {
                // 写像がクライアントの通知から先読みする (ADR 0014 追補 決定 D)。
                // 通知は先に上流へ流し、状態の変化はその後に伝える。
                let predicted = match peek::peek(&msg.body) {
                    Ok(view) if view.is_notification() && !self.identity => {
                        self.tracker.observe_client(&view, &msg.body)
                    }
                    _ => None,
                };
                let mut outs = vec![Out::ToUpstream(msg)];
                if let Some(state) = predicted {
                    outs.extend(releases(gate, &state));
                    if let Some(notification) = self.notify(&state) {
                        outs.push(Out::ToClient(notification));
                    }
                }
                outs
            }
        }
    }

    /// 上流のメッセージを観測し、出力列を返す。
    fn on_upstream(&mut self, msg: RawMessage, gate: &mut Gate) -> Vec<Out> {
        // handshake 後、恒等写像でなく progress の肩代わりも要らず、上流が
        // 既知の写像で観測されているなら、覗き見はその写像のために要る。
        // 写像がまだないときも省けない: 名乗りは `initialize` 応答の後に届く
        // ことがある (typescript-language-server の `$/typescriptVersion`。
        // ADR 0011 決定 A-3)。省けるのは、名乗りを読んで既知でないと分かった後。
        if self.handshake_done
            && !self.identity
            && self.tracker.upstream_is_unmapped()
            && self.client_declared_progress
        {
            return vec![Out::ToClient(msg)];
        }

        // 覗き見は 1 回だけ。上流のメッセージは大きくなりうる (diagnostics
        // 等) ので、判定ごとにパースし直すと透過経路の負荷が倍になる。
        let Ok(view) = peek::peek(&msg.body) else {
            return vec![Out::ToClient(msg)];
        };

        if view.is_request()
            && view.method() == Some(WORK_DONE_PROGRESS_CREATE)
            && !self.client_declared_progress
        {
            // 注入した宣言に由来するリクエスト。クライアントは扱えないので
            // 上流側が成功応答する。id は上流のものをそのまま返す。
            let id = view.id.clone().expect("is_request は id を持つ");
            return vec![Out::ToUpstream(null_response(&id))];
        }

        let is_initialize_response = !self.handshake_done
            && view.method().is_none()
            && view.id.is_some()
            && view.id == self.initialize_id;

        if is_initialize_response {
            return self.on_initialize_result(msg);
        }

        if self.identity {
            let is_self_response = view.method().is_none()
                && matches!(&view.id, Some(RequestId::String(id)) if id == SELF_STATE_REQUEST_ID);
            let is_state_changed =
                view.is_notification() && view.method() == Some(state::SERVER_STATE_CHANGED_METHOD);
            return self.on_upstream_under_identity(msg, is_self_response, is_state_changed, gate);
        }

        let mut outs = Vec::new();
        let changed = self.tracker.observe(&view, &msg.body);
        outs.push(Out::ToClient(msg));
        if let Some(state) = changed {
            if let Some(notification) = self.notify(&state) {
                outs.push(Out::ToClient(notification));
            }
            outs.extend(releases(gate, &state));
        }
        outs
    }

    /// 上流の `InitializeResult`。写像を選び、宣言を足すか恒等写像に切り替える。
    fn on_initialize_result(&mut self, msg: RawMessage) -> Vec<Out> {
        use initialize::InitializeResultAction::*;
        // 上流が名乗った名前で写像を選ぶ。宣言する保証はその写像に聞く。
        self.tracker
            .select_mapping(initialize::server_info(&msg.body).as_ref());
        let provider = self.tracker.provider();
        match initialize::declare_server_state_provider(&msg.body, &provider) {
            NotASuccess => {
                // エラー応答。handshake は完了しておらず、クライアントは
                // initialize を再試行しうる。この id には応答済みなので、
                // 宙に浮いたリクエストではなくなる (上流が消えても二重に
                // 応答しない)。
                self.initialize_id = None;
                vec![Out::ToClient(msg)]
            }
            UpstreamDeclares => {
                self.handshake_done = true;
                self.identity = true;
                eprintln!(
                    "lsp-det: the upstream declares serverStateProvider itself; \
                     the upstream side becomes an identity mapping"
                );
                // 上流の通知は変化のときにしか来ない。初期状態は自分で聞くが、
                // クライアントの `initialized` を流してから。
                self.identity_query_pending = true;
                vec![Out::ToClient(msg)]
            }
            Unrewritable => {
                // capabilities / experimental がオブジェクトでない。宣言
                // できないまま上流側として振る舞うことになるので、黙って
                // 進まず理由を残す。
                self.handshake_done = true;
                eprintln!(
                    "lsp-det: cannot declare serverStateProvider; \
                     the upstream InitializeResult has an unexpected shape"
                );
                vec![Out::ToClient(msg)]
            }
            Declared(body) => {
                self.handshake_done = true;
                vec![Out::ToClient(RawMessage { body })]
            }
        }
    }

    /// 恒等写像のとき。上流の状態は上流の通知と自分の問い合わせから読む。
    /// 通知はクライアントが宣言していれば流し、していなければ下流側だけが読む
    /// (仕様 5.2)。自分の問い合わせの応答はクライアントに見せない。
    fn on_upstream_under_identity(
        &mut self,
        msg: RawMessage,
        is_self_response: bool,
        is_state_changed: bool,
        gate: &mut Gate,
    ) -> Vec<Out> {
        if is_self_response {
            match parse_state_response(&msg.body) {
                Some(state) => return self.adopt_identity_state(state, gate),
                None => {
                    // 準拠を名乗る上流が初期状態に答えなかった。待つ根拠が
                    // ないので、観測できない状態に落とす。
                    eprintln!(
                        "lsp-det: the upstream did not answer {}; treating its state as unknown",
                        state::SERVER_STATE_METHOD
                    );
                    return self.adopt_identity_state(ServerState::unobserved(), gate);
                }
            }
        }

        if is_state_changed {
            // 先に状態を読み、流す必要があるときだけ元のメッセージを渡す
            // (ボディを複製しない)。
            let state = parse_state_notification(&msg.body);
            let mut outs = Vec::new();
            if self.client_declared {
                outs.push(Out::ToClient(msg));
            }
            if let Some(state) = state {
                outs.extend(self.adopt_identity_state(state, gate));
            }
            return outs;
        }

        vec![Out::ToClient(msg)]
    }

    /// 恒等写像のときの境界の状態を更新し、下流側に再評価させる。
    fn adopt_identity_state(&mut self, state: ServerState, gate: &mut Gate) -> Vec<Out> {
        self.tracker.log_boundary(&state);
        self.identity_state = state.clone();
        releases(gate, &state)
    }

    /// handshake が終わる前に上流が消えた。宙に浮いた `initialize` を
    /// エラーで閉じる。これをしないとクライアントは応答を永久に待つ。
    /// handshake 後なら何も返さない (EOF が伝える)。
    fn fail_pending_initialize(&mut self) -> Option<RawMessage> {
        if self.handshake_done {
            return None;
        }
        self.initialize_id.take().map(|id| initialize_failed(&id))
    }

    /// 仕様 4.2 の通知を作る。宣言していないクライアントには送らない
    /// (仕様 5.2)。恒等写像中は上流が送信者なので送らない。
    fn notify(&self, state: &ServerState) -> Option<RawMessage> {
        if !self.client_declared || self.identity {
            return None;
        }
        Some(changed_notification(state))
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

/// 状態変化で下流側が解放・拒否した保留分を出力列にする。
fn releases(gate: &mut Gate, state: &ServerState) -> Vec<Out> {
    gate.on_state(state)
        .into_iter()
        .map(|release| match release {
            Release::Forward(msg) => Out::ToUpstream(msg),
            Release::Reject(response) => Out::ToClient(response),
        })
        .collect()
}

/// 恒等写像のとき、上流の初期状態を問い合わせるリクエスト。
fn self_state_request() -> RawMessage {
    RawMessage {
        body: serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": SELF_STATE_REQUEST_ID,
            "method": state::SERVER_STATE_METHOD,
        }))
        .expect("固定の構造なので常にシリアライズできる"),
    }
}

fn parse_state_response(body: &[u8]) -> Option<ServerState> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        result: ServerState,
    }
    serde_json::from_slice::<Envelope>(body)
        .ok()
        .map(|e| e.result)
}

fn parse_state_notification(body: &[u8]) -> Option<ServerState> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        params: ServerState,
    }
    serde_json::from_slice::<Envelope>(body)
        .ok()
        .map(|e| e.params)
}

/// 上流発リクエストへの成功応答 (`result: null`)。
fn null_response(id: &RequestId) -> RawMessage {
    RawMessage {
        body: serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": null,
        }))
        .expect("固定の構造なので常にシリアライズできる"),
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
/// 沈黙させるとクライアントは応答を永久に待つ。応答を返さないリクエストを
/// 作らない (設計 4.2「上流の消失」)。
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
    fn new() -> Self {
        let now = Instant::now();
        let mut tracker = StateTracker {
            tracker: Tracker::new(),
            started: now,
            entered_state: now,
        };
        // 開始状態を最初の 1 行に出す。これがないと滞在時間の系列が
        // 起点を失い、フラップの実測に使えない。
        let initial = tracker.tracker.state().clone();
        tracker.log(&initial);
        tracker
    }

    /// 上流が名乗った。写像が選べたら開始状態をログして返す。
    fn select_mapping(&mut self, info: Option<&initialize::ServerInfo>) -> Option<ServerState> {
        match self.tracker.select_mapping(info) {
            Some(state) => {
                self.log_selected_mapping("is");
                self.log(&state);
                Some(state)
            }
            None if info.is_none() && self.tracker.observes_upstream() => {
                // serverInfo を返さない上流 (pyright)。起動ログで選んだ写像を保つ。
                eprintln!(
                    "lsp-det: the upstream InitializeResult has no serverInfo; \
                     keeping the mapping selected from its startup log"
                );
                None
            }
            None => {
                eprintln!(
                    "lsp-det: upstream is {:?}; no known mapping, reporting unknown",
                    info.map(|i| i.name.as_str()).unwrap_or("<unnamed>")
                );
                None
            }
        }
    }

    fn state(&self) -> &ServerState {
        self.tracker.state()
    }

    fn provider(&self) -> ServerStateProvider {
        self.tracker.provider()
    }

    fn upstream_is_unmapped(&self) -> bool {
        self.tracker.upstream_is_unmapped()
    }

    /// クライアントの `initialize` を写像に渡す (`initializationOptions`)。
    fn remember_initialize(&mut self, body: &[u8]) {
        self.tracker.remember_initialize(body);
    }

    /// クライアントの通知から写像が先読みした変化をログして返す。
    fn observe_client(&mut self, view: &peek::MessageView, body: &[u8]) -> Option<ServerState> {
        let state = self.tracker.observe_client(view, body)?;
        self.log(&state);
        Some(state)
    }

    /// 恒等写像のとき、上流から読んだ境界の状態をログする。
    fn log_boundary(&mut self, state: &ServerState) {
        self.log(state);
    }

    /// 状態が変わったらログして新しい状態を返す。写像がこの通知で選ばれた
    /// (上流が起動ログで名乗った) ときはその旨も残す。
    fn observe(&mut self, view: &peek::MessageView, body: &[u8]) -> Option<ServerState> {
        let had_mapping = self.tracker.observes_upstream();
        let changed = self.tracker.observe_upstream(view, body);
        if !had_mapping && self.tracker.observes_upstream() {
            self.log_selected_mapping("introduced itself in its startup log as");
            let initial = self.tracker.state().clone();
            self.log(&initial);
        }
        let state = changed?;
        self.log(&state);
        Some(state)
    }

    fn log_selected_mapping(&self, how: &str) {
        let provider = serde_json::to_string(&self.tracker.provider())
            .unwrap_or_else(|_| "<unserializable>".to_string());
        let identity = self.tracker.identity();
        eprintln!(
            "lsp-det: upstream {how} {:?} version {:?}; using its mapping, declaring {provider}",
            identity.map(|i| i.name.as_str()).unwrap_or(""),
            identity
                .and_then(|i| i.version.as_deref())
                .unwrap_or("<none>")
        );
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
    fn spawn_with_cat() -> (
        io::PipeWriter,
        BufReader<io::PipeReader>,
        thread::JoinHandle<i32>,
    ) {
        let (client_out_reader, client_out_writer) = io::pipe().unwrap();
        let (client_in_reader, client_in_writer) = io::pipe().unwrap();
        let handle =
            thread::spawn(move || run(client_in_reader, client_out_writer, "cat", &[]).unwrap());
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
    fn injects_the_capabilities_of_every_known_mapping_before_knowing_the_upstream() {
        // 設計 4.2: serverInfo は initialize の応答で分かるので、注入は
        // 上流が誰か分かる前に、既知の写像ぶんを無条件に行う。
        let (mut client_in, mut client_out, handle) = spawn_with_cat();

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
        assert_eq!(
            value["params"]["capabilities"]["window"]["workDoneProgress"],
            serde_json::Value::Bool(true)
        );

        drop(client_in);
        handle.join().unwrap();
    }

    #[test]
    fn forwards_observed_upstream_messages_byte_for_byte() {
        // 状態を追跡しても、クライアントへ届くのは原文のまま (設計 4.4)。
        let (mut client_in, mut client_out, handle) = spawn_with_cat();

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
        let (mut client_in, mut client_out, handle) = spawn_with_cat();

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

        let handle =
            thread::spawn(move || run(client_in_reader, client_out_writer, "cat", &[]).unwrap());

        // initialize は capability 注入で書き換わる (設計 4.2) ので、それ以外の
        // リクエストで測る。
        let sent = RawMessage {
            body: br#"{"jsonrpc":"2.0","id":1,"method":"textDocument/hover","params":{}}"#.to_vec(),
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
