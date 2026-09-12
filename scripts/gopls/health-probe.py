#!/usr/bin/env python3
"""gopls health probe (docs/research/gopls-health-measurement.md).

What does gopls tell a client when the workspace fails to load, and what do requests
return meanwhile? Drives a real `gopls serve` over stdio with a two-file module and prints
the server-to-client messages with a timestamp (`window/logMessage` lines that mention
neither "error" nor "loading" are dropped to keep the trace readable). Nothing is judged by
time; the waits only bound how long the probe looks.

usage: health-probe.py --scenario NAME [--observe SECS] [--diagnostics-delay DUR]
       [--after-go-mod-diagnostics | --did-change-before-request | --toggle-b-before-request]

--after-go-mod-diagnostics makes the reload-window and recover-window scenarios send their
first request only after gopls has published diagnostics for go.mod following the change,
instead of immediately after didChangeWatchedFiles. A server that debounces the notification
answers an immediate request from the snapshot before the change; the diagnostics come after
the change has been applied.

--did-change-before-request makes the same scenarios send a textDocument/didChange for a.go
(full text, unchanged) between didChangeWatchedFiles and the first request. A server that
debounces watched-file notifications flushes them inside the editor operation, so the request
that follows sees the snapshot after the change.

--toggle-b-before-request does the same with a textDocument/didOpen of b.go (didClose when it
is already open) instead of a didChange of a.go, so the editor operation touches a file other
than the one the request is about.

--diagnostics-delay sets gopls's diagnosticsDelay (default 1s) through initializationOptions, to
see whether a window's length follows it.

scenarios:
  broken-start   go.mod has a syntax error from the start
  nowdp          same, but the client declares no window.workDoneProgress
  break-later    healthy load, then go.mod is broken on disk (+didChangeWatchedFiles), then fixed
  reload-window  healthy load, then go.mod changes; references repeated with 50ms pauses
  recover-window healthy, broken, fixed; then go.mod changes; references repeated with 50ms pauses
  missing-dep    go.mod requires a module that cannot be fetched (GOPROXY=off)
  stale-after-watched-change
                 healthy load; b.go loses its call to Target on disk (+didChangeWatchedFiles),
                 then references on Target every 10ms: does the answer still count the call?
                 Then b.go gets the call back through a burst of 20 notifications 30ms apart
"""

import argparse
import json
import os
import queue
import subprocess
import tempfile
import threading
import time

ap = argparse.ArgumentParser()
ap.add_argument("--scenario", required=True)
ap.add_argument("--observe", type=float, default=6)
timing = ap.add_mutually_exclusive_group()
timing.add_argument("--after-go-mod-diagnostics", action="store_true")
timing.add_argument("--did-change-before-request", action="store_true")
timing.add_argument("--toggle-b-before-request", action="store_true")
ap.add_argument(
    "--diagnostics-delay",
    help="gopls diagnosticsDelay via initializationOptions, e.g. 300ms",
)
a = ap.parse_args()
wdp = a.scenario != "nowdp"

GOOD = "module fixture\n\ngo 1.21\n"
BROKEN = "module fixture\n\ngo 1.21\n\nrequire (\n"  # unterminated require block
A_GO = "package fixture\n\nfunc Target() {}\n"
B_GO = "package fixture\n\nfunc Caller() { Target() }\n"

root = tempfile.mkdtemp(prefix=f"gopls-health-{a.scenario}-")


def write(name, text):
    with open(os.path.join(root, name), "w") as f:
        f.write(text)


def read(name):
    with open(os.path.join(root, name)) as f:
        return f.read()


write("go.mod", BROKEN if a.scenario in ("broken-start", "nowdp") else GOOD)
write("a.go", A_GO)
write("b.go", B_GO)
if a.scenario == "missing-dep":
    write("go.mod", GOOD + "\nrequire example.com/nonexistent v1.0.0\n")
    write(
        "a.go",
        'package fixture\n\nimport _ "example.com/nonexistent"\n\nfunc Target() {}\n',
    )
    os.environ["GOPROXY"] = "off"
    os.environ["GOFLAGS"] = "-mod=mod"

uri = "file://" + root
t0 = time.time()
p = subprocess.Popen(
    ["gopls", "serve"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.DEVNULL,
    cwd=root,
)
q = queue.Queue()


def reader():
    while True:
        hdr = b""
        while not hdr.endswith(b"\r\n\r\n"):
            c = p.stdout.read(1)
            if not c:
                q.put(None)
                return
            hdr += c
        n = int(hdr.split(b"Content-Length:")[1].split(b"\r\n")[0])
        m = json.loads(p.stdout.read(n))
        m["_recv"] = (
            time.time()
        )  # receive time, for correlating a message with a change
        q.put(m)


threading.Thread(target=reader, daemon=True).start()
seq = [0]


send_lock = threading.Lock()


def send(m):
    b = json.dumps(m).encode()
    with send_lock:
        p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
        p.stdin.flush()


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


def req(method, params):
    seq[0] += 1
    send({"jsonrpc": "2.0", "id": seq[0], "method": method, "params": params})
    return seq[0]


def log(m):
    t = time.time() - t0
    if "method" in m and "id" in m:  # a request from the server
        print(
            f"[{t:6.3f}s] <- request {m['method']} {json.dumps(m.get('params'))[:200]}"
        )
        result = None
        if m["method"] == "workspace/configuration":
            result = [None] * len(m["params"]["items"])
        send({"jsonrpc": "2.0", "id": m["id"], "result": result})
    elif "method" in m:
        s = json.dumps(m.get("params"))
        if (
            m["method"] == "window/logMessage"
            and "error" not in s.lower()
            and "loading" not in s.lower()
        ):
            return
        if m["method"] == "textDocument/publishDiagnostics":
            d = m["params"]["diagnostics"]
            name = os.path.basename(m["params"]["uri"])
            print(
                f"[{t:6.3f}s] <- publishDiagnostics {name} n={len(d)} {[x['message'][:80] for x in d]}"
            )
            return
        print(f"[{t:6.3f}s] <- {m['method']} {s[:300]}")
    else:
        print(
            f"[{t:6.3f}s] <- response id={m.get('id')} {json.dumps(m.get('result', m.get('error')))[:300]}"
        )


def pump(secs):
    end = time.time() + secs
    while time.time() < end:
        try:
            m = q.get(timeout=max(0.01, end - time.time()))
        except queue.Empty:
            return
        if m is None:
            print("EOF")
            return
        log(m)


def request_and_wait(method, params, secs=15):
    i = req(method, params)
    t1 = time.time()
    print(f"[{time.time() - t0:6.3f}s] -> {method} id={i}")
    end = time.time() + secs
    while time.time() < end:
        try:
            m = q.get(timeout=max(0.01, end - time.time()))
        except queue.Empty:
            print("   (no response within budget)")
            return
        if m is None:
            print("EOF")
            return
        log(m)
        if m.get("id") == i and "method" not in m:
            print(f"   (answered after {time.time() - t1:.3f}s)")
            return


def go_mod_changed(content):
    write("go.mod", content)
    notify(
        "workspace/didChangeWatchedFiles",
        {"changes": [{"uri": uri + "/go.mod", "type": 2}]},
    )


def poll(label, content, method="textDocument/references", params=None):
    """Change go.mod, then repeat `method` (waiting for each answer and pausing 50ms between
    attempts) and print the runs of outcomes."""
    params = params or REFS
    after = "immediately"
    if a.after_go_mod_diagnostics:
        after = "after publishDiagnostics for go.mod"
    elif a.did_change_before_request:
        after = "after a didChange for a.go"
    elif a.toggle_b_before_request:
        after = "after a didOpen/didClose of b.go"
    print(
        f"=== go.mod change ({label}) + didChangeWatchedFiles, then {method} {after}, repeated with 50ms pauses"
    )
    # Log everything already received, so that a go.mod diagnostic left over from
    # initialization or the previous change cannot satisfy the wait below.
    while True:
        try:
            m = q.get_nowait()
        except queue.Empty:
            break
        if m is None:
            raise SystemExit("gopls exited: EOF on stdout")
        log(m)
    tc = time.time()
    go_mod_changed(content)
    t_notified = time.time()
    if a.did_change_before_request:
        a_go_version[0] += 1
        notify(
            "textDocument/didChange",
            {
                "textDocument": {"uri": uri + "/a.go", "version": a_go_version[0]},
                "contentChanges": [{"text": read("a.go")}],
            },
        )
        print(
            f"   didChange a.go (unchanged text, version {a_go_version[0]}) at t+{time.time() - tc:.3f}s"
        )
    if a.toggle_b_before_request:
        if b_open[0]:
            notify("textDocument/didClose", {"textDocument": {"uri": uri + "/b.go"}})
        else:
            notify(
                "textDocument/didOpen",
                {
                    "textDocument": {
                        "uri": uri + "/b.go",
                        "languageId": "go",
                        "version": 1,
                        "text": read("b.go"),
                    }
                },
            )
        b_open[0] = not b_open[0]
        print(
            f"   {'didOpen' if b_open[0] else 'didClose'} b.go at t+{time.time() - tc:.3f}s"
        )
    if a.after_go_mod_diagnostics:
        end = time.time() + 10
        while True:
            if time.time() >= end:
                raise SystemExit(
                    f"no publishDiagnostics for go.mod within 10s of the change ({label})"
                )
            try:
                m = q.get(timeout=max(0.01, end - time.time()))
            except queue.Empty:
                continue
            if m is None:
                raise SystemExit(
                    "gopls exited before publishing diagnostics for go.mod"
                )
            log(m)
            if (
                m.get("method") == "textDocument/publishDiagnostics"
                and m["params"]["uri"].endswith("/go.mod")
                and m["_recv"]
                >= t_notified  # not one still arriving for an earlier change
            ):
                break
        print(f"   go.mod diagnostics arrived at t+{time.time() - tc:.3f}s")
    outcomes = []
    for _ in range(60):
        i = req(method, params)
        got = None
        end = time.time() + 2
        while time.time() < end:
            try:
                m = q.get(timeout=max(0.01, end - time.time()))
            except queue.Empty:
                break
            if m is None:
                raise SystemExit("gopls exited: EOF on stdout")
            if m.get("id") == i and "method" not in m:
                got = m
                break
            log(m)
        if got is None:
            kind = "timeout"
        elif "error" in got:
            kind = "ERR:" + got["error"]["message"][:30]
        else:
            kind = f"OK n={len(got['result'] or [])}"
        outcomes.append((round(time.time() - tc, 3), kind))
        time.sleep(0.05)
    prev = None
    for t, kind in outcomes:
        if kind != prev:
            print(f"   t+{t:.3f}s {kind}")
            prev = kind
    pump(1)


caps = {
    "workspace": {
        "workspaceFolders": True,
        "didChangeWatchedFiles": {"dynamicRegistration": True},
    }
}
if wdp:
    caps["window"] = {"workDoneProgress": True}
request_and_wait(
    "initialize",
    {
        "processId": os.getpid(),
        "rootUri": uri,
        "capabilities": caps,
        "workspaceFolders": [{"uri": uri, "name": "fixture"}],
        **(
            {"initializationOptions": {"diagnosticsDelay": a.diagnostics_delay}}
            if a.diagnostics_delay
            else {}
        ),
    },
)
notify("initialized", {})
notify(
    "textDocument/didOpen",
    {
        "textDocument": {
            "uri": uri + "/a.go",
            "languageId": "go",
            "version": 1,
            "text": read("a.go"),
        }
    },
)
a_go_version = [1]
b_open = [False]
print(f"[{time.time() - t0:6.3f}s] -> didOpen a.go; observing {a.observe}s")
pump(a.observe)
target_line = 4 if a.scenario == "missing-dep" else 2
REFS = {
    "textDocument": {"uri": uri + "/a.go"},
    "position": {"line": target_line, "character": 5},
    "context": {"includeDeclaration": False},
}
DEFN = {"textDocument": {"uri": uri + "/a.go"}, "position": {"line": 0, "character": 8}}
request_and_wait("textDocument/references", REFS)
request_and_wait("textDocument/definition", DEFN)
request_and_wait("workspace/symbol", {"query": "Target"})

if a.scenario == "break-later":
    print(
        "=== breaking go.mod on disk + didChangeWatchedFiles, then references immediately"
    )
    go_mod_changed(BROKEN)
    request_and_wait("textDocument/references", REFS)
    pump(a.observe)
    request_and_wait("textDocument/references", REFS)
    request_and_wait("workspace/symbol", {"query": "Target"})
    print(
        "=== fixing go.mod on disk + didChangeWatchedFiles, then references immediately"
    )
    go_mod_changed(GOOD)
    request_and_wait("textDocument/references", REFS)
    pump(a.observe)
    print("=== after the fix settled: references")
    request_and_wait("textDocument/references", REFS)
    for label, content in (
        ("comment", GOOD + "// touched\n"),
        ("require without import", GOOD + "\nrequire golang.org/x/text v0.14.0\n"),
    ):
        print(
            f"=== go.mod change ({label}), then references immediately, then after 3s"
        )
        go_mod_changed(content)
        request_and_wait("textDocument/references", REFS)
        pump(3)
        request_and_wait("textDocument/references", REFS)


def watch_references(label, seconds):
    """Repeat references on Target every 10ms for `seconds` and print the runs of answers."""
    tc = time.time()
    outcomes = []
    while time.time() - tc < seconds:
        i = req("textDocument/references", REFS)
        got = None
        end = time.time() + 2
        while time.time() < end:
            try:
                m = q.get(timeout=max(0.01, end - time.time()))
            except queue.Empty:
                break
            if m is None:
                raise SystemExit("gopls exited: EOF on stdout")
            if m.get("id") == i and "method" not in m:
                got = m
                break
            log(m)
        if got is None:
            kind = "timeout"
        elif "error" in got:
            kind = "ERR:" + got["error"]["message"][:30]
        else:
            kind = f"OK n={len(got['result'] or [])}"
        outcomes.append((round(time.time() - tc, 3), kind))
        time.sleep(0.01)
    prev = None
    for t, kind in outcomes:
        if kind != prev:
            print(f"   {label} t+{t:.3f}s {kind}")
            prev = kind
    return outcomes


if a.scenario == "stale-after-watched-change":
    print(
        "=== b.go loses the call on disk + didChangeWatchedFiles(b.go), then references every 10ms"
    )
    write("b.go", "package fixture\n\nfunc Caller() {}\n")
    notify(
        "workspace/didChangeWatchedFiles",
        {"changes": [{"uri": uri + "/b.go", "type": 2}]},
    )
    watch_references("single", 1.2)
    pump(1)
    print(
        "=== b.go gets the call back on disk + 20 didChangeWatchedFiles(b.go) 30ms apart, references every 10ms"
    )
    write("b.go", B_GO)
    send_times = []

    def burst():
        # 20 notifications 30ms apart, on their own thread so that a slow request cannot
        # bunch them up; the actual send times are reported below.
        for k in range(20):
            while (d := tb + k * 0.03 - time.time()) > 0:
                time.sleep(d)
            notify(
                "workspace/didChangeWatchedFiles",
                {"changes": [{"uri": uri + "/b.go", "type": 2}]},
            )
            send_times.append(time.time() - tb)

    tb = time.time()
    burst_thread = threading.Thread(target=burst, daemon=True)
    burst_thread.start()

    tc = tb
    outcomes = []
    while time.time() - tc < 1.2:
        i = req("textDocument/references", REFS)
        got = None
        end = time.time() + 2
        while time.time() < end:
            try:
                m = q.get(timeout=max(0.01, end - time.time()))
            except queue.Empty:
                break
            if m is None:
                raise SystemExit("gopls exited: EOF on stdout")
            if m.get("id") == i and "method" not in m:
                got = m
                break
            log(m)
        kind = (
            "timeout"
            if got is None
            else ("ERR" if "error" in got else f"OK n={len(got['result'] or [])}")
        )
        outcomes.append((round(time.time() - tc, 3), kind))
        time.sleep(0.01)
    burst_thread.join(timeout=2)
    deviation = max(abs(s - k * 0.03) for k, s in enumerate(send_times))
    print(
        f"   burst: {len(send_times)} notifications sent 30ms apart, last at t+{send_times[-1]:.3f}s, "
        f"max deviation from schedule {deviation * 1000:.1f}ms"
    )
    prev = None
    for t, kind in outcomes:
        if kind != prev:
            print(f"   burst t+{t:.3f}s {kind}")
            prev = kind

if a.scenario == "reload-window":
    poll("comment", GOOD + "// touched\n")
    poll("require without import", GOOD + "\nrequire golang.org/x/text v0.14.0\n")

if a.scenario == "recover-window":
    print("=== breaking go.mod, waiting, fixing, waiting")
    go_mod_changed(BROKEN)
    pump(3)
    go_mod_changed(GOOD)
    pump(3)
    request_and_wait("textDocument/references", REFS)
    poll("comment after recovery", GOOD + "// touched\n")
    poll("second comment after recovery", GOOD + "// touched twice\n")
    poll(
        "third change, definition",
        GOOD + "// thrice\n",
        "textDocument/definition",
        DEFN,
    )

req("shutdown", None)
pump(2)
notify("exit", None)
p.wait(timeout=5)
