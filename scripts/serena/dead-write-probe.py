#!/usr/bin/env python3
"""保留中の要求がないときに言語サーバーを落とし、その後の要求がどう終わるかを見る。

oraios/serena#2004 (stdin への書き込み失敗の握りつぶし) の実測。読み取りスレッドは
プロセスの終了を見た瞬間に保留中の要求を一度だけキャンセルする (`_cancel_pending_requests`)。
その後に登録された要求は、死んだ stdin への書き込みがログだけで握りつぶされ、誰も失敗
させないので、要求の打ち切りいっぱい待って素の `TimeoutError` になる (再起動の経路に
乗らない)。上流が直れば要求 #2 は打ち切りの前に `LanguageServerTerminatedException` を
原因とする `SolidLSPException` になる。これが受け入れ条件で、満たせばコード 0、素の
`TimeoutError` か例外なしならコード 1 で終わる。reference/serena の環境で動かす:

    cd reference/serena && uv run --frozen python ../../scripts/serena/dead-write-probe.py \\
        python /path/to/repo

環境変数:
    REQUEST_TIMEOUT  要求の打ち切り秒数 (既定 8。Serena の既定は 235 で待ちが長いだけ)

言語サーバーは PATH で解決される。自分の子孫のプロセスを全部 SIGKILL するので、
lsp-det は挟まない (挟むと lsp-det も落ちて別の経路になる)。プロセス探索は pgrep に
依存する (Linux 専用)。観測の記録は docs/research/serena-integration-measurement.md。
"""

import logging
import os
import signal
import subprocess
import sys
import tempfile
import time

from solidlsp.ls import SolidLanguageServer
from solidlsp.ls_config import LanguageServerConfig, LanguageServerId
from solidlsp.ls_exceptions import SolidLSPException
from solidlsp.settings import SolidLSPSettings

REQUEST_TIMEOUT = float(os.environ.get("REQUEST_TIMEOUT", "8"))


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


def main() -> None:
    lang, repo = sys.argv[1], sys.argv[2]
    logging.basicConfig(
        level=logging.INFO,
        format="%(relativeCreated)6d %(name)s %(levelname)s %(message)s",
        stream=sys.stderr,
    )
    ls_id = LanguageServerId(lang)
    t0 = time.time()

    def log(message: str) -> None:
        print(f"[{time.time() - t0:7.3f}] PROBE {message}", flush=True)

    # 言語ごとの参照点。fixture は a.py (`def target()`) と b.py (import と呼び出し)。
    rel, line, col = {"python": ("a.py", 0, 4), "typescript": ("a.ts", 0, 16)}[lang]
    accepted = False
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
            for pid in victims:
                os.kill(pid, signal.SIGKILL)
            log(f"SIGKILL {victims} (no request pending)")
            time.sleep(1.0)
            log(f"after 1 s: ls.is_running()={ls.is_running()}")
            t1 = time.time()
            try:
                refs2 = ls.request_references(rel, line, col)
            except SolidLSPException as e:
                elapsed = time.time() - t1
                terminated = e.is_language_server_terminated()
                log(
                    f"references #2 raised SolidLSPException after {elapsed:.2f} s: "
                    f"terminated={terminated} cause={type(e.cause).__name__}"
                )
                accepted = terminated and elapsed < REQUEST_TIMEOUT
            except Exception as e:  # noqa: BLE001 - 素の TimeoutError がここに来る (不合格)
                log(
                    f"references #2 raised {type(e).__name__} after {time.time() - t1:.2f} s: "
                    f"{str(e)[:200]}"
                )
            else:
                log(f"references #2 -> {len(refs2)} locations (NO ERROR SURFACED)")
    log("done")
    if not accepted:
        sys.exit(1)


if __name__ == "__main__":
    main()
