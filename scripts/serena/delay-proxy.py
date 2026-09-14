#!/usr/bin/env python3
"""言語サーバーの前に立ち、指定したメソッドの要求だけを一定時間止めてから流す stdio プロキシ。

oraios/serena#2003 (打ち切りが素の `TimeoutError` で、`$/cancelRequest` が送られず、
遅れた応答が捨てられる) の実測に使う。サーバーは生きていて遅いだけ、という状況を
実サーバー (pyright 等) の上に作る。ボディは原文のまま流し、止める要求の id と、
クライアントから来た `$/cancelRequest` を stderr に記す。

    delay-proxy.py --delay 8 --method textDocument/references -- pyright-langserver --stdio

観測は stderr の "DELAY-PROXY" 行。相棒は dead-write-probe.py と同じ流儀の
timeout-probe.py。
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
    body = stream.read(length)
    return headers + body


def body_of(message: bytes) -> dict:
    return json.loads(message.split(b"\r\n\r\n", 1)[1])


def pump_client_to_server(
    client_in, server_in, method: str, delay: float, lock: threading.Lock
) -> None:
    held: dict[object, threading.Timer] = {}
    while True:
        message = read_message(client_in)
        if message is None:
            log("client closed")
            break
        payload = body_of(message)
        if payload.get("method") == "$/cancelRequest":
            target = payload.get("params", {}).get("id")
            log(f"client sent $/cancelRequest for id={target} (held={target in held})")
        if payload.get("method") == method and "id" in payload:
            request_id = payload["id"]
            log(f"holding {method} id={request_id} for {delay:.1f} s")

            def release(msg: bytes = message, rid: object = request_id) -> None:
                with lock:
                    server_in.write(msg)
                    server_in.flush()
                held.pop(rid, None)
                log(f"released {method} id={rid}")

            timer = threading.Timer(delay, release)
            held[request_id] = timer
            timer.start()
            continue
        with lock:
            server_in.write(message)
            server_in.flush()


def pump_server_to_client(server_out, client_out) -> None:
    while True:
        message = read_message(server_out)
        if message is None:
            log("server closed")
            break
        payload = body_of(message)
        if "id" in payload and "method" not in payload:
            log(f"forwarding response id={payload['id']}")
        client_out.write(message)
        client_out.flush()


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
    assert server.stdin and server.stdout
    log(f"started {command} pid={server.pid}")
    client_in = sys.stdin.buffer
    client_out = sys.stdout.buffer
    lock = threading.Lock()
    up = threading.Thread(
        target=pump_client_to_server,
        args=(client_in, server.stdin, args.method, args.delay, lock),
        daemon=True,
    )
    down = threading.Thread(
        target=pump_server_to_client,
        args=(server.stdout, client_out),
        daemon=True,
    )
    up.start()
    down.start()
    down.join()
    server.wait()
    log(f"server exited with {server.returncode}")


if __name__ == "__main__":
    main()
