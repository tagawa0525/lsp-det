#!/usr/bin/env python3
"""Metals で、最初の横断問い合わせの後に増えた作業を後続の問い合わせが待つかを測る。

oraios/serena#1858 (Scala: セッション最初の find_referencing_symbols が部分結果) は最初の
問い合わせの前で Metals の progress を待つ形で閉じられたが、同じ latch
(`_has_waited_for_cross_file_references`) が Scala の adapter にも残る。ここでは:

    1. `A.target` の references (期待: B.scala の 1 件)
    2. `A.target` を使う C.scala をディスクに作り、didOpen する (Metals が compile を走らせる)
    3. references を 4 回問う。間隔は直前の呼び出しから +0 ms、+1 s、+3 s、+6 s
       (open からの経過では 0、1、4、10 秒)。C.scala が入るまでの時間を見る

scala-cli の単一プロジェクト (project.scala、A.scala、B.scala) を fixture にする。
`nix develop .#servers` (metals、scala-cli、jdk21) の中で reference/serena の環境から動かす:

    cd reference/serena && uv run --frozen python ../../scripts/serena/metals-later-query-probe.py /path/to/fixture

環境変数:
    SEQUENCE   C.scala を開いた後の各問い合わせの、直前の呼び出しからの間隔 (秒。既定 "0,1,3,6")

観測の記録は docs/research/serena-request-path-state-prototype.md。
"""

import logging
import os
import sys
import tempfile
import time

from solidlsp.ls import SolidLanguageServer
from solidlsp.ls_config import LanguageServerConfig, LanguageServerId
from solidlsp.settings import SolidLSPSettings

SEQUENCE = [float(x) for x in os.environ.get("SEQUENCE", "0,1,3,6").split(",")]
TARGET = ("A.scala", 1, 6)  # `def target` の target
C_SCALA = "object C {\n  val y: Int = A.target\n}\n"


class Tap(logging.Handler):
    def __init__(self) -> None:
        super().__init__(level=logging.INFO)
        self.lines: list[tuple[float, str]] = []

    def emit(self, record: logging.LogRecord) -> None:
        m = record.getMessage()
        if "Metals" in m or "progress" in m.lower() or "cross-file" in m:
            self.lines.append((time.time(), m))


def main() -> None:
    repo = os.path.abspath(sys.argv[1])
    logging.basicConfig(
        level=logging.INFO,
        format="%(relativeCreated)6d %(name)s %(levelname)s %(message)s",
        stream=sys.stderr,
    )
    tap = Tap()
    logging.getLogger("solidlsp").addHandler(tap)
    ls_id = LanguageServerId("scala")
    t0 = time.time()

    def log(message: str) -> None:
        print(f"[{time.time() - t0:7.3f}] PROBE {message}", flush=True)

    def refs(label: str) -> int:
        t1 = time.time()
        r = ls.request_references(*TARGET)
        files = sorted({x.get("relativePath", x.get("uri", "")) for x in r})
        log(
            f"{label}: {len(r)} locations in {len(files)} files {files}, {time.time() - t1:.2f} s"
        )
        return len(files)

    c_path = os.path.join(repo, "C.scala")
    if os.path.lexists(c_path):  # 壊れた symlink も含めて、あれば止まる
        # fixture のものではないファイルを消さない。前回の中断で残ったものも手で確かめて消す。
        sys.exit(f"{c_path} already exists; refusing to overwrite it")
    counts: list[int] = []
    created = False
    try:
        with tempfile.TemporaryDirectory(prefix="metals-later-") as tmp:
            settings = SolidLSPSettings(
                solidlsp_dir=tmp, project_data_path=os.path.join(repo, ".serena")
            )
            config = LanguageServerConfig(ls_id=ls_id, workspace_folders=["."])
            ls = SolidLanguageServer.create(
                config, repo, solidlsp_settings=settings, timeout=120
            )
            with ls.start_server_context():
                log("server started (Serena's startup wait finished)")
                refs("call 1 (before C.scala)")
                created = True  # 書き込みの途中で失敗しても finally で消せるよう、先に印を付ける
                with open(c_path, "w", encoding="utf-8") as f:
                    f.write(C_SCALA)
                with ls.open_file("C.scala"):
                    log("created and opened C.scala")
                    for wait in SEQUENCE:
                        if wait:
                            time.sleep(wait)
                        counts.append(refs(f"call after +{wait:g} s"))
    finally:
        if created and os.path.lexists(c_path):
            os.remove(
                c_path
            )  # この probe が作ったものだけを消す (途中で失敗しても残さない)
    log("progress log:")
    for t, line in tap.lines:
        log(f"  [{t - t0:7.3f}] {line[:140]}")
    log(f"files per call after C.scala: {counts} (complete = 2)")


if __name__ == "__main__":
    main()
