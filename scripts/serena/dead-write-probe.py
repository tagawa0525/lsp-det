#!/usr/bin/env python3
"""保留中の要求がないときに言語サーバーを落とし、その後の要求がどう終わるかを見る。

oraios/serena#2004 (stdin への書き込み失敗の握りつぶし) の実測。読み取りスレッドは
プロセスの終了を見た瞬間に保留中の要求を一度だけキャンセルする (`_cancel_pending_requests`)。
その後に登録された要求は、死んだ stdin への書き込みがログだけで握りつぶされ、誰も失敗
させないので、要求の打ち切りいっぱい待って素の `TimeoutError` になる (再起動の経路に
乗らない)。上流が直れば (oraios/serena#2030 の形) 要求 #2 は打ち切りの前に
`LanguageServerTerminatedException` を原因とする `SolidLSPException` になる。これが
受け入れ条件で、満たせばコード 0、素の `TimeoutError` か例外なしならコード 1、
前提が崩れた (kill する相手がいない、kill の時点で要求が保留中だった) ならコード 2 で
終わる。reference/serena の環境で動かす:

    cd reference/serena && uv run --frozen python ../../scripts/serena/dead-write-probe.py \\
        python /path/to/repo

環境変数:
    REQUEST_TIMEOUT  要求の打ち切り秒数 (既定 8。Serena の既定は 235 で待ちが長いだけ)

要求 #2 を送る前に、stdout の読み取りスレッド (`LSP-stdout-reader:<ls_id>`) の終了を
join で待ち、そのキャンセルが "Cancelling 0 pending" だったことをログで確かめる。
これで #2 が読み取りスレッドのキャンセルに拾われる競合 (偽陽性) を除く。
言語サーバーは PATH で解決される。自分の子孫のプロセスを全部 SIGKILL するので、
lsp-det は挟まない (挟むと lsp-det も落ちて別の経路になる)。プロセス探索は pgrep に
依存する (Linux 専用)。観測の記録は docs/research/serena-integration-measurement.md。
"""

import logging
import os
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time

from solidlsp.ls import SolidLanguageServer
from solidlsp.ls_config import LanguageServerConfig, LanguageServerId
from solidlsp.ls_exceptions import SolidLSPException
from solidlsp.settings import SolidLSPSettings

REQUEST_TIMEOUT = float(os.environ.get("REQUEST_TIMEOUT", "8"))
READER_THREAD_PREFIX = "LSP-stdout-reader:"
CANCEL_LOG = re.compile(r"^Cancelling (\d+) pending language server requests$")


def children(pid: int) -> list[int]:
    out = subprocess.run(
        ["pgrep", "-P", str(pid)], capture_output=True, text=True, check=False
    )
    return [int(p) for p in out.stdout.split()]


def descendants(pid: int) -> list[int]:
    found: list[int] = []
    for child in children(pid):
        found.append(child)
        found += descendants(child)
    return found


class CancelLog(logging.Handler):
    """`_cancel_pending_requests` の "Cancelling N pending …" を (時刻, N) で記録する。"""

    def __init__(self) -> None:
        super().__init__(level=logging.INFO)
        self.events: list[tuple[float, int]] = []

    def emit(self, record: logging.LogRecord) -> None:
        # 同じ関数が要求ごとに "Cancelling Request[...]" も出すので、集計の行だけを読む。
        match = CANCEL_LOG.match(record.getMessage())
        if match:
            self.events.append((time.time(), int(match.group(1))))


def reader_threads() -> list[threading.Thread]:
    return [t for t in threading.enumerate() if t.name.startswith(READER_THREAD_PREFIX)]


def main() -> None:
    lang, repo = sys.argv[1], sys.argv[2]
    logging.basicConfig(
        level=logging.INFO,
        format="%(relativeCreated)6d %(name)s %(levelname)s %(message)s",
        stream=sys.stderr,
    )
    cancels = CancelLog()
    logging.getLogger("solidlsp.ls_process").addHandler(cancels)
    ls_id = LanguageServerId(lang)
    t0 = time.time()

    def log(message: str) -> None:
        print(f"[{time.time() - t0:7.3f}] PROBE {message}", flush=True)

    # 言語ごとの参照点。fixture は a.py (`def target()`) と b.py (import と呼び出し)。
    rel, line, col = {"python": ("a.py", 0, 4), "typescript": ("a.ts", 0, 16)}[lang]
    verdict = 1
    with tempfile.TemporaryDirectory(prefix="dead-write-") as tmp:
        settings = SolidLSPSettings(
            solidlsp_dir=tmp, project_data_path=os.path.join(repo, ".serena")
        )
        config = LanguageServerConfig(ls_id=ls_id, workspace_folders=["."])
        ls = SolidLanguageServer.create(
            config, repo, solidlsp_settings=settings, timeout=REQUEST_TIMEOUT
        )
        with ls.start_server_context():
            refs = ls.request_references(rel, line, col)
            log(f"references #1 -> {len(refs)} locations")

            victims = descendants(os.getpid())
            if not ls.is_running() or not victims or not reader_threads():
                log(
                    f"precondition failed: is_running={ls.is_running()} "
                    f"victims={victims} reader_threads={len(reader_threads())}"
                )
                sys.exit(2)
            # 時刻は最初の kill の前に取る。後に取ると、読み取りスレッドのキャンセルが
            # その前に走って first_cancels が空になる競合がある。
            t_kill = time.time()
            for pid in victims:
                os.kill(pid, signal.SIGKILL)
            log(f"SIGKILL {victims} (no request pending)")

            # 読み取りスレッドの終了 (= キャンセルの完了) を待つ。sleep では同期にならない。
            for thread in reader_threads():
                thread.join(timeout=10)
            alive = [t.name for t in reader_threads()]
            first_cancels = [(t, n) for t, n in cancels.events if t >= t_kill]
            log(
                f"reader thread exited after {time.time() - t_kill:.3f} s "
                f"(still alive: {alive}); cancel events since kill: "
                f"{[n for _, n in first_cancels]}; ls.is_running()={ls.is_running()}"
            )
            if (
                alive or first_cancels != [(first_cancels[0][0], 0)]
                if first_cancels
                else True
            ):
                log(
                    "precondition failed: expected exactly one 'Cancelling 0 pending' before request #2"
                )
                sys.exit(2)

            t1 = time.time()
            try:
                refs2 = ls.request_references(rel, line, col)
            except SolidLSPException as e:
                elapsed = time.time() - t1
                terminated = e.is_language_server_terminated()
                later_cancels = [n for t, n in cancels.events if t >= t1]
                log(
                    f"references #2 raised SolidLSPException after {elapsed:.2f} s: "
                    f"terminated={terminated} cause={type(e.cause).__name__}; "
                    f"cancel events after request #2: {later_cancels}"
                )
                # 受け入れ: 打ち切りの前に terminated で失敗し、それが #2 の後のキャンセル
                # (書き込み失敗の経路) によるもの。
                if terminated and elapsed < REQUEST_TIMEOUT and later_cancels:
                    verdict = 0
            except Exception as e:  # noqa: BLE001 - 素の TimeoutError がここに来る (不合格)
                log(
                    f"references #2 raised {type(e).__name__} after {time.time() - t1:.2f} s: "
                    f"{str(e)[:200]}"
                )
            else:
                log(f"references #2 -> {len(refs2)} locations (NO ERROR SURFACED)")
    log(f"done (exit {verdict})")
    sys.exit(verdict)


if __name__ == "__main__":
    main()
