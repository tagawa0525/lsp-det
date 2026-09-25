#!/usr/bin/env python3
"""typescript-language-server coverage probe (docs/research/typescript-language-server-no-solution-coverage.md).

Does `ready` from lsp-det's typescript-language-server mapping mean what its `coverage`
declaration says, whatever the workspace layout? Starts the server command (by default lsp-det in
front of typescript-language-server) over stdio as a client that declares
`experimental.serverState`, prints the declaration from `InitializeResult`, opens the file the
target symbol is declared in, waits for `ready`, and asks `textDocument/references`. With
--open-after it then opens a second file, waits for the load that opening triggers to finish
(`indexing` followed by `ready`), and asks again: a declaration of `coverage.scope` "workspace"
promises that the second answer is not larger than the first (spec 6.1). Nothing is judged by
time; --observe only bounds how long the probe waits for a state it expects. A second open that
triggers no load cannot be told apart from a load whose signal never came, so it is accepted only
with --no-load (the caller asserts the file is already in a loaded project); without it, no
`indexing` within --observe voids the measurement.

usage: coverage-probe.py --workspace DIR --target PATH:LINE:COL [--open-after PATH]
       [--no-load] [--expected N] [--server CMD] [--observe SECS]

PATH is relative to DIR; LINE and COL are 0-based. --expected is the number of files the complete
answer contains (the declaration's own file excluded), when known. The exit status is 0 when the
probe ran to the end, whatever it observed.
"""

import argparse
import json
import os
import queue
import shlex
import subprocess
import threading
import time
from pathlib import Path
from urllib.parse import unquote, urlparse
from urllib.request import url2pathname

ap = argparse.ArgumentParser()
ap.add_argument("--workspace", required=True)
ap.add_argument("--target", required=True)
ap.add_argument("--open-after")
ap.add_argument("--no-load", action="store_true")
ap.add_argument("--expected", type=int)
ap.add_argument("--server", default="lsp-det -- typescript-language-server --stdio")
ap.add_argument("--observe", type=float, default=120)
args = ap.parse_args()

root = Path(args.workspace).resolve()
target_path, target_line, target_col = args.target.rsplit(":", 2)
target_path = os.path.normpath(
    target_path
)  # compared with os.path.relpath of the answers
target_line, target_col = int(target_line), int(target_col)
T0 = time.time()


def log(message):
    print(f"[{time.time() - T0:8.3f}] {message}", flush=True)


def uri(relative):
    return (root / relative).as_uri()


proc = subprocess.Popen(
    shlex.split(args.server),
    cwd=root,
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.DEVNULL,
)
inbox = queue.Queue()


def reader():
    out = proc.stdout
    while True:
        length = None
        while True:
            line = out.readline()
            if not line:
                inbox.put(None)
                return
            if line in (b"\r\n", b"\n"):
                break
            if line.lower().startswith(b"content-length:"):
                length = int(line.split(b":", 1)[1])
        body = b""
        while len(body) < length:
            chunk = out.read(length - len(body))
            if not chunk:
                inbox.put(None)
                return
            body += chunk
        inbox.put(json.loads(body))


threading.Thread(target=reader, daemon=True).start()
next_id = 0
readiness = None  # latest readiness from experimental/serverStateChanged
health = None  # latest health; "error" (tsserver exited) voids the measurement
transitions = []  # (time, readiness) in arrival order


def send(message):
    body = json.dumps(message).encode()
    proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    proc.stdin.flush()


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


def handle(message):
    """Answers server-to-client requests and records state notifications."""
    global readiness, health
    if "method" in message and "id" in message:
        # workspace/configuration, window/workDoneProgress/create, client/registerCapability
        result = (
            [None] * len(message["params"].get("items", []))
            if message["method"] == "workspace/configuration"
            else None
        )
        send({"jsonrpc": "2.0", "id": message["id"], "result": result})
    elif message.get(
        "method"
    ) == "window/logMessage" and "Using Typescript version" in str(
        message["params"].get("message")
    ):
        # which TypeScript tsserver runs; the mapping declares guarantees only for tested versions
        log(f"logMessage {message['params']['message'].splitlines()[0]}")
    elif message.get("method") == "experimental/serverStateChanged":
        readiness = message["params"].get("readiness")
        health = message["params"].get("health")
        transitions.append((time.time() - T0, readiness))
        log(f"serverStateChanged {json.dumps(message['params'])}")


def request(method, params):
    global next_id
    next_id += 1
    my_id = next_id
    send({"jsonrpc": "2.0", "id": my_id, "method": method, "params": params})
    deadline = time.time() + args.observe
    while time.time() < deadline:
        try:
            message = inbox.get(timeout=deadline - time.time())
        except queue.Empty:
            break
        if message is None:
            raise SystemExit("server closed the connection")
        if message.get("id") == my_id and "method" not in message:
            return message
        handle(message)
    raise SystemExit(f"no response to {method} within {args.observe}s")


def wait_until(predicate, label):
    """Pumps messages until predicate() holds. Returns False if --observe runs out first."""
    deadline = time.time() + args.observe
    while not predicate():
        if health == "error":
            raise SystemExit(
                f"{label}: health is error (tsserver exited); the measurement is void"
            )
        remaining = deadline - time.time()
        if remaining <= 0:
            log(f"{label}: not observed within {args.observe}s")
            return False
        try:
            message = inbox.get(timeout=remaining)
        except queue.Empty:
            continue
        if message is None:
            raise SystemExit("server closed the connection")
        handle(message)
    return True


def open_file(relative):
    text = (root / relative).read_text(encoding="utf-8")
    language = "typescriptreact" if relative.endswith(".tsx") else "typescript"
    notify(
        "textDocument/didOpen",
        {
            "textDocument": {
                "uri": uri(relative),
                "languageId": language,
                "version": 1,
                "text": text,
            }
        },
    )
    log(f"didOpen {relative}")


def path_of(file_uri):
    """A local path from a file URI (percent-decoded, with the drive letter on Windows)."""
    return Path(url2pathname(unquote(urlparse(file_uri).path)))


def references(label):
    answer = request(
        "textDocument/references",
        {
            "textDocument": {"uri": uri(target_path)},
            "position": {"line": target_line, "character": target_col},
            "context": {"includeDeclaration": False},
        },
    )
    if health == "error":
        raise SystemExit(
            f"{label}: health is error (tsserver exited); the measurement is void"
        )
    if "error" in answer:
        log(f"{label}: error {json.dumps(answer['error'])}")
        return None
    files = sorted(
        {
            os.path.relpath(path_of(loc["uri"]), root)
            for loc in answer.get("result") or []
        }
    )
    files = [f for f in files if f != target_path]
    expected = "" if args.expected is None else f" of {args.expected} expected"
    log(
        f"{label}: {len(answer.get('result') or [])} locations in {len(files)} files{expected} (readiness {readiness})"
    )
    return files


init = request(
    "initialize",
    {
        "processId": os.getpid(),
        "rootUri": root.as_uri(),
        "workspaceFolders": [{"uri": root.as_uri(), "name": root.name}],
        "capabilities": {
            "window": {"workDoneProgress": True},
            "workspace": {"workspaceFolders": True, "configuration": True},
            "textDocument": {"references": {}},
            "experimental": {"serverState": True},
        },
    },
)
provider = (
    ((init.get("result") or {}).get("capabilities") or {})
    .get("experimental", {})
    .get("serverStateProvider")
)
log(f"declaration serverStateProvider = {json.dumps(provider)}")
notify("initialized", {})

open_file(target_path)
if not wait_until(lambda: readiness == "ready", "ready after opening the target"):
    raise SystemExit("the target's load never reached ready; nothing to measure")
first = references("references after the target's load")

if args.open_after:
    mark = len(transitions)
    open_file(args.open_after)
    loaded = wait_until(
        lambda: any(r == "indexing" for _, r in transitions[mark:]),
        "indexing after the second open",
    )
    if not loaded and not args.no_load:
        raise SystemExit(
            "no indexing after the second open; the measurement is void"
            " (pass --no-load if the file is in an already loaded project)"
        )
    if not loaded:
        log(
            f"no indexing within {args.observe}s after the second open (--no-load);"
            " asking in the same ready as the first question"
        )
    if loaded and not wait_until(
        lambda: readiness == "ready", "ready after the second open"
    ):
        raise SystemExit(
            "the second open's load never reached ready; nothing to measure"
        )
    if readiness != "ready":
        raise SystemExit(
            f"readiness is {readiness} before the second question; nothing to measure"
        )
    # tsserver loads the projects one after another and lsp-det reports ready between two loads,
    # so this answer is taken at the first ready after the first indexing: a lower bound of what
    # the open eventually adds. The question here is only whether it grows at all.
    second = references(
        f"references after opening {args.open_after} (at the first ready; a lower bound)"
    )
    if first is not None and second is not None:
        added = sorted(set(second) - set(first))
        log(
            f"files added by opening {args.open_after}: {len(added)}"
            + (f" (first: {added[:5]})" if added else "")
        )

request("shutdown", None)
notify("exit", None)
proc.wait(timeout=10)
log("done")
