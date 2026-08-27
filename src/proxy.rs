//! クライアントと上流言語サーバーを中継するイベントループ (v0.1-design.md 4.8)。
//!
//! M1 の範囲は純粋な素通し (ゲートなし)。全状態はこのモジュールの
//! 単一ループに閉じ、ロックを持たない。読み取りは std スレッド + `mpsc`
//! で行い、判断はループ内でのみ行う。

use std::io::{self, BufReader, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::framing::{self, RawMessage};
use crate::process;

/// 上流の生死をポーリングする間隔。`Upstream` の所有権 (kill する権利) を
/// main ループ 1 箇所に保つため、別スレッドで `wait()` させず `recv_timeout`
/// のたびに `try_wait()` する (v0.1-design.md 4.8: タイマーは `recv_timeout`)。
const UPSTREAM_POLL_INTERVAL: Duration = Duration::from_millis(20);

enum Event {
    FromClient(RawMessage),
    ClientClosed,
    ClientReadError(io::Error),
    FromUpstream(RawMessage),
}

/// クライアントと上流を中継し、プロキシ自身の終了コードを返す。
pub fn run<R, W>(client_in: R, client_out: W, command: &str, args: &[String]) -> io::Result<i32>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let mut handles = process::spawn(command, args)?;
    let mut upstream_stdin = handles.stdin;
    let mut client_out = client_out;

    let (tx, rx) = mpsc::channel::<Event>();

    spawn_stderr_relay(handles.stderr);
    spawn_client_reader(client_in, tx.clone());
    spawn_upstream_reader(handles.stdout, tx);

    let exit_code = loop {
        match rx.recv_timeout(UPSTREAM_POLL_INTERVAL) {
            Ok(Event::FromClient(msg)) => {
                if framing::write_message(&mut upstream_stdin, &msg).is_err() {
                    // 上流の stdin が既に閉じている。次のポーリングで exit を検出する。
                }
            }
            Ok(Event::FromUpstream(msg)) => {
                if framing::write_message(&mut client_out, &msg).is_err() {
                    // クライアントが既に読み取りをやめている。無視して続行する
                    // (次のポーリングで上流の exit を検出する)。
                }
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
                break code;
            }
        }
    };

    let _ = client_out.flush();
    Ok(exit_code)
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
                    // 上流の stdout が閉じた。実際の終了コード検出は
                    // main ループの try_wait ポーリングに任せる。
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

    #[test]
    fn round_trips_a_message_through_a_real_upstream_process() {
        // upstream に `cat` を使う: client -> proxy -> cat(echo) -> proxy -> client
        // というラウンドトリップで、バイト列が非破壊で往復することを検証する。
        let (client_out_reader, client_out_writer) = io::pipe().unwrap();
        let (client_in_reader, mut client_in_writer) = io::pipe().unwrap();

        let handle =
            thread::spawn(move || run(client_in_reader, client_out_writer, "cat", &[]).unwrap());

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
