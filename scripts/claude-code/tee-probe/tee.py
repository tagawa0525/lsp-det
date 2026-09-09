"""Record what an LSP client writes to a language server, then run the server.

Usage (from a Claude Code plugin's .lsp.json):
    python3 ${CLAUDE_PLUGIN_ROOT}/tee.py <log file> <server command> [args...]

Every chunk the client writes to stdin is appended to the log file with a
timestamp (seconds since the server was started) and forwarded to the server
unchanged. The server's stdout is inherited, so the client sees the server as
usual. The log has the raw LSP framing (Content-Length headers and JSON bodies).
"""

import subprocess
import sys
import time


def main() -> int:
    with open(sys.argv[1], "ab", buffering=0) as log:
        server = subprocess.Popen(sys.argv[2:], stdin=subprocess.PIPE)
        assert server.stdin is not None
        t0 = time.monotonic()
        while True:
            chunk = sys.stdin.buffer.read1(65536)
            if not chunk:
                break
            log.write(f"\n### {time.monotonic() - t0:9.4f}s\n".encode())
            log.write(chunk)
            server.stdin.write(chunk)
            server.stdin.flush()
        server.stdin.close()
        return server.wait()


if __name__ == "__main__":
    sys.exit(main())
