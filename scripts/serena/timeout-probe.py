#!/usr/bin/env python3
"""要求への応答が打ち切りより遅れたとき、Serena (solidlsp) に何が見えるかを測る。

oraios/serena#2003 の実測。delay-proxy.py が `textDocument/references` の要求を即座に
サーバーへ渡し、サーバーの応答だけを REQUEST_TIMEOUT より長く止める (サーバーは要求を
受け取って処理を終えている。クライアントから見れば「その要求だけ遅いサーバー」)。
見るのは (1) 呼び出し側に届く例外の型、(2) クライアントが `$/cancelRequest` を送るか
(プロキシの stderr)、(3) 遅れて届いた応答の扱い (`_pending_requests` の変化)、(4) その後の
要求が普通に通るか。測れないもの: 打ち切った要求の計算がサーバー側で続くかどうか
(pyright は 2 ファイルの fixture では即答するので、続く計算がそもそもない)。
reference/serena の環境で動かす:

    cd reference/serena && uv run --frozen python ../../scripts/serena/timeout-probe.py \\
        python /path/to/repo

環境変数:
    REQUEST_TIMEOUT  要求の打ち切り秒数 (既定 5)
    DELAY            プロキシがサーバーの応答を止める秒数 (既定 8。REQUEST_TIMEOUT より長くする)

観測できた実行の終了コードは 0 (受け入れ条件はまだ置かない。観測のみ)。DELAY は
REQUEST_TIMEOUT より長いことを起動前に要求する (満たさなければ 0 以外で終わる)。最初の
横断要求は SolidLSP の latch の 2 秒も払うので、短い DELAY でも合計で打ち切られることは
あるが、それでは止めた応答が打ち切りの原因だと言えない。この検査はそれを切り分ける。
観測の記録は docs/research/serena-integration-measurement.md。
"""

import logging
import os
import sys
import tempfile
import time

from solidlsp.ls import SolidLanguageServer
from solidlsp.ls_config import LanguageServerConfig, LanguageServerId
from solidlsp.ls_exceptions import SolidLSPException
from solidlsp.settings import SolidLSPSettings

REQUEST_TIMEOUT = float(os.environ.get("REQUEST_TIMEOUT", "5"))
DELAY = float(os.environ.get("DELAY", "8"))
# adapter が extra_args で "--stdio" を足すので、ここには書かない。
UPSTREAM = {
    "python": ["pyright-langserver"],
    "typescript": ["typescript-language-server"],
}
PROXY = os.path.join(os.path.dirname(os.path.abspath(__file__)), "delay-proxy.py")


class LogTap(logging.Handler):
    """solidlsp のログのうち、打ち切りと遅延応答に関わる行を (時刻, 文) で記録する。"""

    KEYS = (
        "Request interrupted by user or not found",
        "Cancelling",
        "timed out",
        "Failed to write",
    )

    def __init__(self) -> None:
        super().__init__(level=logging.DEBUG)
        self.lines: list[tuple[float, str]] = []

    def emit(self, record: logging.LogRecord) -> None:
        message = record.getMessage()
        if any(k in message for k in self.KEYS):
            self.lines.append((time.time(), message))


def main() -> None:
    lang, repo = sys.argv[1], sys.argv[2]
    if DELAY <= REQUEST_TIMEOUT:
        sys.exit(
            f"DELAY ({DELAY}) must exceed REQUEST_TIMEOUT ({REQUEST_TIMEOUT}) so that the held response alone causes the timeout (the first cross-file request also pays SolidLSP's 2 s latch)"
        )
    logging.basicConfig(
        level=logging.INFO,
        format="%(relativeCreated)6d %(name)s %(levelname)s %(message)s",
        stream=sys.stderr,
    )
    tap = LogTap()
    ls_logger = logging.getLogger("solidlsp.ls_process")
    ls_logger.setLevel(logging.DEBUG)  # "not found for ID" は debug
    ls_logger.addHandler(tap)
    ls_id = LanguageServerId(lang)
    base_cmd = [sys.executable, PROXY, "--delay", str(DELAY), "--", *UPSTREAM[lang]]
    rel, line, col = {"python": ("a.py", 0, 4), "typescript": ("a.ts", 0, 16)}[lang]
    t0 = time.time()

    def log(message: str) -> None:
        print(f"[{time.time() - t0:7.3f}] PROBE {message}", flush=True)

    with tempfile.TemporaryDirectory(prefix="timeout-probe-") as tmp:
        settings = SolidLSPSettings(
            solidlsp_dir=tmp,
            project_data_path=os.path.join(repo, ".serena"),
            ls_specific_settings={ls_id: {"ls_base_cmd": base_cmd}},
        )
        config = LanguageServerConfig(ls_id=ls_id, workspace_folders=["."])
        ls = SolidLanguageServer.create(
            config, repo, solidlsp_settings=settings, timeout=REQUEST_TIMEOUT
        )
        log(f"request timeout {REQUEST_TIMEOUT} s, proxy delay {DELAY} s")
        with ls.start_server_context():
            t1 = time.time()
            try:
                refs = ls.request_references(rel, line, col)
                log(
                    f"references #1 -> {len(refs)} locations after {time.time() - t1:.2f} s (unexpected: no timeout)"
                )
            except SolidLSPException as e:
                log(
                    f"references #1 raised SolidLSPException after {time.time() - t1:.2f} s: terminated={e.is_language_server_terminated()} cause={type(e.cause).__name__}"
                )
            except Exception as e:  # noqa: BLE001 - 素の TimeoutError を見たい
                log(
                    f"references #1 raised {type(e).__name__} after {time.time() - t1:.2f} s: {e}"
                )
            pending = ls.server._pending_requests
            log(
                f"is_running()={ls.is_running()}; pending request ids right after the timeout: {sorted(pending)}"
            )
            # 遅れた応答が届くまで待ち、届いた瞬間を pending の変化で捉える。
            abandoned = sorted(pending)
            deadline = time.time() + DELAY + 5.0
            while time.time() < deadline and any(i in pending for i in abandoned):
                time.sleep(0.01)
            still_pending = [i for i in abandoned if i in pending]
            if not abandoned:
                log(
                    "nothing was pending after the timeout (unexpected; the late-response observation is void)"
                )
            elif still_pending:
                log(
                    f"no late response within {DELAY + 5.0:.0f} s: request ids {still_pending} are still pending"
                )
            else:
                log(
                    f"late response arrived at +{time.time() - t1:.2f} s from the request: "
                    f"pending request ids now {sorted(pending)} (the abandoned Request was popped "
                    f"and completed into a queue nobody reads; no log line is emitted)"
                )
            t2 = time.time()
            try:
                refs2 = ls.request_references(rel, line, col)
                log(
                    f"references #2 -> {len(refs2)} locations after {time.time() - t2:.2f} s (unexpected: its response should be held)"
                )
            except Exception as e:  # noqa: BLE001
                log(
                    f"references #2 raised {type(e).__name__} after {time.time() - t2:.2f} s"
                )
            t3 = time.time()
            symbols = ls.request_document_symbols(rel)
            log(
                f"documentSymbol (not delayed) -> ok after {time.time() - t3:.2f} s ({type(symbols).__name__})"
            )
    log("solidlsp log lines of interest:")
    for t, line_ in tap.lines:
        log(f"  [{t - t0:7.3f}] {line_}")


if __name__ == "__main__":
    main()
