#!/usr/bin/env python3
"""同じシンボルへの references を連続で問い、件数が揺れるか (oraios/serena#1937 の形) を測る。

ts-monorepo-fixture.py が作った workspace で、`formatDisplay` の references を #1937 の
6 連続 (起動待ちの後 → +0 ms → +0 ms → +2 s → +0 ms → +2 s) で問い、各回の件数と所要時間、
その間の tsls の `$/progress` の begin / end を出す。期待する件数 (EXPECTED) に満たない回が
あればコード 1、全部揃えば 0。reference/serena の環境で動かす:

    cd reference/serena && EXPECTED=16 uv run --frozen python \\
        ../../scripts/serena/repeat-references-probe.py /path/to/fixture

環境変数:
    EXPECTED   期待する参照ファイル数 (fixture 生成時の表示)。一致だけを OK とする (多くても不合格)。
               未指定なら判定せず観測のみ
    SEQUENCE   呼び出し間の待ち秒数をカンマ区切りで (既定 "0,0,2,0,2" = 6 回)
    TARGET     問い合わせる位置 "relative/path.ts:line:col" (0 始まり。既定は fixture の formatDisplay)
    PREOPEN    6 連続の前に didOpen して開いたままにする相対パス。references を多く持つ package の
               ファイルを指定すると、tsserver がその project (と referenced project のソース) を
               読み込み始めた直後に問い合わせることになる (#1937 の報告者の環境に近づける)
    PREOPEN_ONCE  6 連続の前に didOpen してすぐ didClose する相対パス (project を読み込ませて孤児にする)
    MODE       "references" (既定。`request_references` の生の結果) か "referencing_symbols"
               (MCP ツール find_referencing_symbols と同じ `request_referencing_symbols`)
    TSLS_CMD   typescript-language-server の起動コマンド (空白区切り。既定は PATH の
               "typescript-language-server"。別の版を試すときに
               "/path/to/typescript-language-server --tsserver-path /path/to/typescript/lib" 等)

言語サーバーは `ls_base_cmd` で指定する (adapter が "--stdio" を足す)。
観測の記録は docs/research/serena-request-path-state-prototype.md。
"""

import contextlib
import logging
import os
import sys
import tempfile
import time

from solidlsp.ls import SolidLanguageServer
from solidlsp.ls_config import LanguageServerConfig, LanguageServerId
from solidlsp.settings import SolidLSPSettings

EXPECTED = int(os.environ["EXPECTED"]) if os.environ.get("EXPECTED") else None
SEQUENCE = [float(x) for x in os.environ.get("SEQUENCE", "0,0,2,0,2").split(",")]
_target = os.environ.get("TARGET", "packages/leaf/src/index.ts:0:16").rsplit(":", 2)
TARGET = (_target[0], int(_target[1]), int(_target[2]))
TSLS_CMD = os.environ.get("TSLS_CMD", "typescript-language-server").split()
MODE = os.environ.get("MODE", "references")
if MODE not in ("references", "referencing_symbols"):
    sys.exit(f"MODE must be references or referencing_symbols, not {MODE!r}")
# 6 連続の前に開いたままにするファイル (大きな project を読み込ませる)
PREOPEN = os.environ.get("PREOPEN")
# 6 連続の前に開いてすぐ閉じるファイル (project を読み込ませて孤児にする)
PREOPEN_ONCE = os.environ.get("PREOPEN_ONCE")


class ProgressTap(logging.Handler):
    """adapter が出す "TypeScript LSP progress [token]: started/ended" を (時刻, 文) で記録する。"""

    def __init__(self) -> None:
        super().__init__(level=logging.INFO)
        self.lines: list[tuple[float, str]] = []

    def emit(self, record: logging.LogRecord) -> None:
        message = record.getMessage()
        if "TypeScript LSP progress" in message or "cross-file" in message:
            self.lines.append((time.time(), message))


def main() -> None:
    repo = os.path.abspath(sys.argv[1])
    logging.basicConfig(
        level=logging.INFO,
        format="%(relativeCreated)6d %(name)s %(levelname)s %(message)s",
        stream=sys.stderr,
    )
    tap = ProgressTap()
    logging.getLogger("solidlsp").addHandler(tap)
    ls_id = LanguageServerId("typescript")
    t0 = time.time()

    def log(message: str) -> None:
        print(f"[{time.time() - t0:7.3f}] PROBE {message}", flush=True)

    counts: list[int] = []
    with tempfile.TemporaryDirectory(prefix="repeat-refs-") as tmp:
        settings = SolidLSPSettings(
            solidlsp_dir=tmp,
            project_data_path=os.path.join(repo, ".serena"),
            ls_specific_settings={ls_id: {"ls_base_cmd": TSLS_CMD}},
        )
        config = LanguageServerConfig(ls_id=ls_id, workspace_folders=["."])
        ls = SolidLanguageServer.create(
            config, repo, solidlsp_settings=settings, timeout=120
        )
        with (
            ls.start_server_context(),
            ls.open_file(PREOPEN) if PREOPEN else contextlib.nullcontext(),
        ):
            if PREOPEN:
                log(f"opened {PREOPEN} and kept it open")
            if PREOPEN_ONCE:
                with ls.open_file(PREOPEN_ONCE):
                    pass
                log(f"opened and closed {PREOPEN_ONCE}")
            log(
                f"server started (Serena's startup wait finished); mode={MODE}; tsls command: {TSLS_CMD}"
            )
            waits = [0.0, *SEQUENCE]
            for i, wait in enumerate(waits, start=1):
                if wait:
                    time.sleep(wait)
                t1 = time.time()
                if MODE == "referencing_symbols":
                    # MCP ツール find_referencing_symbols と同じ経路。参照ごとにファイルを open して
                    # containing symbol を引き、見つからなければ落ちる。
                    found = ls.request_referencing_symbols(
                        *TARGET,
                        include_imports=False,
                        include_self=False,
                        include_file_symbols=True,
                    )
                    refs = found
                    files = sorted(
                        {
                            f.symbol["location"].get(
                                "relativePath", f.symbol["location"].get("uri", "")
                            )
                            for f in found
                        }
                    )
                else:
                    refs = ls.request_references(*TARGET)
                    files = sorted(
                        {r.get("relativePath", r.get("uri", "")) for r in refs}
                    )
                counts.append(len(files))
                verdict = (
                    ""
                    if EXPECTED is None
                    else (
                        " OK"
                        if len(files) == EXPECTED
                        else f" MISMATCH (expected {EXPECTED} files)"
                    )
                )
                log(
                    f"call {i} (after +{wait:.2g} s): {len(refs)} locations in {len(files)} files, {time.time() - t1:.2f} s{verdict}"
                )
                if EXPECTED is not None and len(files) < EXPECTED:
                    log(f"  files: {files}")
    log("progress / cross-file wait log:")
    for t, line in tap.lines:
        log(f"  [{t - t0:7.3f}] {line}")
    log(f"files per call: {counts}")
    if EXPECTED is not None and any(c != EXPECTED for c in counts):
        sys.exit(1)


if __name__ == "__main__":
    main()
