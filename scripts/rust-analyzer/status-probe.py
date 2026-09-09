#!/usr/bin/env python3
"""rust-analyzer serverStatus probe (docs/research/rust-analyzer-quiescent-measurement.md).

What does `experimental/serverStatus` say between `initialized` and the end of the first
load? Drives a real rust-analyzer over stdio on a one-crate project (or on an empty
directory) and prints every status notification with a timestamp. Stops at the first status
that is quiescent with the load settled (`health` error, or `readiness` ready when the build
reports the field, or plain quiescence otherwise). Nothing is judged by time.

usage: status-probe.py RUST_ANALYZER [--empty]
"""

import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time


class Server:
    def __init__(self, program, cwd):
        self.process = subprocess.Popen(
            [program],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            cwd=cwd,
        )
        self.started = time.monotonic()

    def elapsed(self):
        return f"{time.monotonic() - self.started:6.3f}s"

    def send(self, message):
        body = json.dumps(message).encode()
        self.process.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        self.process.stdin.flush()

    def read(self):
        length = None
        while True:
            line = self.process.stdout.readline().decode()
            if line == "":
                raise RuntimeError("rust-analyzer closed stdout")
            if line in ("\r\n", "\n"):
                break
            name, _, value = line.partition(":")
            if name.strip().lower() == "content-length":
                length = int(value)
        if length is None:
            raise RuntimeError("a message without Content-Length")
        return json.loads(self.process.stdout.read(length))


def settled(params, empty):
    if not params.get("quiescent"):
        return False
    if empty:
        return params.get("health") == "error"
    return params.get("readiness", "ready") == "ready"


def main():
    program = sys.argv[1]
    empty = "--empty" in sys.argv[2:]
    root = tempfile.mkdtemp(prefix="ra-probe-")
    try:
        if not empty:
            os.makedirs(f"{root}/src")
            with open(f"{root}/Cargo.toml", "w") as f:
                f.write(
                    '[package]\nname = "probe"\nversion = "0.1.0"\nedition = "2021"\n'
                )
            with open(f"{root}/src/lib.rs", "w") as f:
                f.write(
                    "pub fn alpha() -> u32 { 1 }\npub fn beta() -> u32 { alpha() + 1 }\n"
                )
        server = Server(program, root)
        uri = pathlib.Path(root).as_uri()
        server.send(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": os.getpid(),
                    "rootUri": uri,
                    "workspaceFolders": [{"uri": uri, "name": "probe"}],
                    "capabilities": {
                        "experimental": {"serverStatusNotification": True}
                    },
                },
            }
        )
        while True:
            message = server.read()
            if message.get("id") == 1:
                info = message["result"].get("serverInfo")
                print(f"{server.elapsed()} initialize result: serverInfo={info}")
                break
        server.send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
        while True:
            message = server.read()
            method = message.get("method")
            if method == "experimental/serverStatus":
                params = message["params"]
                print(f"{server.elapsed()} serverStatus {json.dumps(params)}")
                if settled(params, empty):
                    break
            elif method == "workspace/configuration":
                # One entry per requested item (the client declares no `workspace.configuration`,
                # so this is not expected, but a wrong shape would derail the server).
                items = message["params"]["items"]
                server.send(
                    {
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": [None] * len(items),
                    }
                )
            elif "id" in message and method is not None:
                server.send({"jsonrpc": "2.0", "id": message["id"], "result": None})
        server.send({"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": None})
        while server.read().get("id") != 2:
            pass
        server.send({"jsonrpc": "2.0", "method": "exit", "params": None})
        server.process.wait(timeout=10)
    finally:
        shutil.rmtree(root)


if __name__ == "__main__":
    main()
