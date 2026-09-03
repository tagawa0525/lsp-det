#!/usr/bin/env python3
"""Serena (solidlsp) を被験者にして、lsp-det 経由の言語サーバーで references を取る。

上流への提案 (Serena が experimental/serverState を読む変更) をローカルで
確かめるための道具。reference/serena の環境で動かす:

    cd reference/serena && uv run --frozen python ../../scripts/serena/probe.py \\
        python  /path/to/repo a.py 0 4
    cd reference/serena && uv run --frozen python ../../scripts/serena/probe.py \\
        typescript /path/to/repo a.ts 0 16

環境変数:
    VIA_LSP_DET=0  lsp-det を挟まず上流を直接起動する (比較用)
    CRASH=1        references の後に tsserver を SIGKILL し、直後の references の
                   見え方を出す (typescript のみ意味がある)

lsp-det と上流 (pyright-langserver / typescript-language-server) は PATH で
解決される。target/upstream/bin を先頭に置けばソースビルドの上流を使う。
観測の記録は docs/research/serena-integration-measurement.md。
"""

import logging
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time

from solidlsp.ls import SolidLanguageServer
from solidlsp.ls_config import LanguageServerConfig, LanguageServerId
from solidlsp.ls_exceptions import SolidLSPException
from solidlsp.settings import SolidLSPSettings

UPSTREAM = {
    "python": ["pyright-langserver", "--stdio"],
    "typescript": ["typescript-language-server", "--stdio"],
}


def children(pid: int) -> list[int]:
    """`pid` の直接の子プロセス。"""
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


def cmdline(pid: int) -> str:
    path = f"/proc/{pid}/cmdline"
    if not os.path.exists(path):
        return ""
    with open(path, encoding="utf-8", errors="replace") as f:
        return f.read().replace("\0", " ")


def kill_tsserver_descendants() -> list[int]:
    """自分の子孫のうち tsserver (typescript-language-server の子の node) を落とす。"""
    victims = [pid for pid in descendants(os.getpid()) if "tsserver.js" in cmdline(pid)]
    for pid in victims:
        os.kill(pid, signal.SIGKILL)
    return victims


def main() -> None:
    lang, repo, rel = sys.argv[1], sys.argv[2], sys.argv[3]
    line, col = int(sys.argv[4]), int(sys.argv[5])
    crash = os.environ.get("CRASH") == "1"
    via_lsp_det = os.environ.get("VIA_LSP_DET", "1") == "1"
    logging.basicConfig(
        level=logging.INFO,
        format="%(relativeCreated)6d %(name)s %(levelname)s %(message)s",
        stream=sys.stderr,
    )

    ls_id = LanguageServerId(lang)
    upstream = UPSTREAM[lang]
    base_cmd = ["lsp-det", "--", *upstream] if via_lsp_det else upstream
    tmp = tempfile.mkdtemp(prefix="serena-probe-")
    settings = SolidLSPSettings(
        solidlsp_dir=tmp,
        project_data_path=os.path.join(repo, ".serena"),
        ls_specific_settings={ls_id: {"ls_base_cmd": base_cmd}},
    )
    config = LanguageServerConfig(ls_id=ls_id, workspace_folders=["."])
    ls = SolidLanguageServer.create(config, repo, solidlsp_settings=settings)

    t0 = time.time()

    def log(message: str) -> None:
        print(f"[{time.time() - t0:7.3f}] PROBE {message}", flush=True)

    log(f"base_cmd={base_cmd}")
    with ls.start_server_context():
        log("server started (Serena's readiness wait finished)")
        refs = ls.request_references(rel, line, col)
        where = [
            (r.get("relativePath", r.get("uri")), r["range"]["start"]["line"])
            for r in refs
        ]
        log(f"references -> {len(refs)} locations: {where}")
        if crash:
            victims = kill_tsserver_descendants()
            log(f"killed tsserver processes {victims}")
            time.sleep(0.5)
            try:
                refs2 = ls.request_references(rel, line, col)
                log(
                    f"references after crash -> {len(refs2)} locations (NO ERROR SURFACED)"
                )
            except SolidLSPException as e:
                log(f"references after crash raised {type(e).__name__}: {str(e)[:900]}")
    log("done")
    shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
