"""Record what an LSP client writes to a language server, then run the server.

Usage (from a Claude Code plugin's .lsp.json):
    python3 ${CLAUDE_PLUGIN_ROOT}/tee.py <log file> <server command> [args...]

Every chunk the client writes to stdin is appended to the log file with a
timestamp (seconds since this wrapper started the server) and forwarded to the
server unchanged. The server's stdout is inherited, so the client sees the
server as usual. The log has the raw LSP framing (Content-Length headers and
JSON bodies), which includes the full text of every opened document, so the
file is created readable by the owner only.
"""

import os
import subprocess
import sys
import time


def main() -> int:
    if len(sys.argv) < 3:
        sys.exit("usage: tee.py <log file> <server command> [args...]")
    fd = os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    with open(fd, "ab", buffering=0) as log:
        server = subprocess.Popen(sys.argv[2:], stdin=subprocess.PIPE)
        if server.stdin is None:
            raise RuntimeError("the server's stdin is not a pipe")
        t0 = time.monotonic()
        forwarding = True
        while True:
            chunk = sys.stdin.buffer.read1(65536)
            if not chunk:
                break
            log.write(f"\n### {time.monotonic() - t0:9.4f}s\n".encode())
            log.write(chunk)
            if not forwarding:
                continue
            try:
                server.stdin.write(chunk)
                server.stdin.flush()
            except BrokenPipeError:
                # The server is gone. Keep draining and recording the client's
                # writes so the client is not blocked on a full pipe.
                log.write(b"\n### server closed its stdin; recording only\n")
                forwarding = False
        try:
            server.stdin.close()
        except BrokenPipeError:
            pass  # closing flushes; nothing is listening any more
        return server.wait()


if __name__ == "__main__":
    sys.exit(main())
