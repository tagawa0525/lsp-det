#!/usr/bin/env python3
"""言語サーバーの前に立ち、指定したメソッドの要求への応答だけを一定時間止めてから流す stdio プロキシ。

oraios/serena#2003 (打ち切りが素の `TimeoutError` で、`$/cancelRequest` が送られず、
遅れた応答が捨てられる) の実測に使う。要求はそのまま即座にサーバーへ渡し、その応答を
サーバーから受け取った後で止める。サーバーは要求を受け取って処理を終えているので、
クライアントから見れば「生きているが、その要求だけ遅い」サーバーになる。ボディは
原文のまま流し、止めた応答の id と、クライアントから来た `$/cancelRequest` (id と、
その id の応答を止めている最中か) を stderr に記す。

    delay-proxy.py --delay 8 --method textDocument/references -- pyright-langserver --stdio

クライアントが stdin を閉じたら、止めている応答を捨て、サーバーの stdin を閉じて
終了を待つ (5 秒で kill)。観測は stderr の "DELAY-PROXY" 行。相棒は timeout-probe.py。
"""

import argparse
import json
import subprocess
import sys
import threading
import time

LOG = sys.stderr


def log(message: str) -> None:
    LOG.write(f"DELAY-PROXY {time.time():.3f} {message}\n")
    LOG.flush()


def read_message(stream) -> bytes | None:
    """Content-Length フレーム 1 つを読み、ヘッダ込みの原文バイト列を返す。EOF なら None。"""
    headers = b""
    length = None
    while True:
        line = stream.readline()
        if not line:
            return None
        headers += line
        if line in (b"\r\n", b"\n"):
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1].strip())
    if length is None:
        return None
    # BufferedReader.read(n) はパイプでは n バイトか EOF まで読むが、それに頼らず揃える。
    body = b""
    while len(body) < length:
        chunk = stream.read(length - len(body))
        if not chunk:
            return None
        body += chunk
    return headers + body


def body_of(message: bytes) -> dict:
    return json.loads(message.split(b"\r\n\r\n", 1)[1])


class Proxy:
    def __init__(self, server: subprocess.Popen, method: str, delay: float) -> None:
        self.server = server
        self.method = method
        self.delay = delay
        self.watched: set[object] = (
            set()
        )  # 止める対象の要求の id (クライアント → サーバーで観測)
        self.held: dict[
            object, threading.Timer
        ] = {}  # 止めている応答 (id → 解放のタイマー)
        self.lock = threading.Lock()
        self.client_out = sys.stdout.buffer
        self.client_closed = threading.Event()

    def client_to_server(self) -> None:
        client_in = sys.stdin.buffer
        server_in = self.server.stdin
        assert server_in
        while True:
            message = read_message(client_in)
            if message is None:
                log("client closed")
                break
            payload = body_of(message)
            if payload.get("method") == "$/cancelRequest":
                target = payload.get("params", {}).get("id")
                with self.lock:
                    holding = target in self.held
                log(
                    f"client sent $/cancelRequest for id={target} (response held={holding})"
                )
            if payload.get("method") == self.method and "id" in payload:
                with self.lock:
                    self.watched.add(payload["id"])
                log(
                    f"forwarding {self.method} id={payload['id']} to the server at once; its response will be held"
                )
            server_in.write(message)
            server_in.flush()
        self.client_closed.set()
        with self.lock:
            timers = list(self.held.values())
            self.held.clear()
        for timer in timers:
            timer.cancel()
        try:
            server_in.close()
        except OSError:
            pass

    def server_to_client(self) -> None:
        server_out = self.server.stdout
        assert server_out
        while True:
            message = read_message(server_out)
            if message is None:
                log("server closed")
                break
            payload = body_of(message)
            response_id = payload.get("id") if "method" not in payload else None
            with self.lock:
                watched = response_id is not None and response_id in self.watched
            if watched:
                log(
                    f"server answered id={response_id}; holding the response for {self.delay:.1f} s"
                )
                timer = threading.Timer(
                    self.delay, self.release, args=(response_id, message)
                )
                with self.lock:
                    self.held[response_id] = timer
                timer.start()
                continue
            self.write_to_client(message)

    def release(self, response_id: object, message: bytes) -> None:
        with self.lock:
            if self.held.pop(response_id, None) is None:
                return
        log(f"releasing the response for id={response_id}")
        self.write_to_client(message)

    def write_to_client(self, message: bytes) -> None:
        if self.client_closed.is_set():
            return
        with self.lock:
            try:
                self.client_out.write(message)
                self.client_out.flush()
            except (BrokenPipeError, OSError):
                self.client_closed.set()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--delay", type=float, required=True)
    parser.add_argument("--method", default="textDocument/references")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = (
        args.command[1:] if args.command and args.command[0] == "--" else args.command
    )
    if not command:
        parser.error("server command required after --")

    server = subprocess.Popen(
        command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sys.stderr
    )
    log(f"started {command} pid={server.pid}")
    proxy = Proxy(server, args.method, args.delay)
    up = threading.Thread(target=proxy.client_to_server, daemon=True)
    down = threading.Thread(target=proxy.server_to_client, daemon=True)
    up.start()
    down.start()
    up.join()
    try:
        server.wait(timeout=5)
    except subprocess.TimeoutExpired:
        log("server did not exit after stdin closed; killing it")
        server.kill()
        server.wait()
    log(f"server exited with {server.returncode}")


if __name__ == "__main__":
    main()
