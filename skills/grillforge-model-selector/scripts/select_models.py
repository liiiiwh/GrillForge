#!/usr/bin/env python3
"""Run GrillForge's validated, credential-free Worker selector."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import sys


def resolve_executable() -> str:
    configured = os.environ.get("GRILLFORGE_BIN")
    if configured:
        return configured
    discovered = shutil.which("grillforge")
    if discovered:
        return discovered
    if sys.platform == "darwin":
        for candidate in (
            Path("/Applications/GrillForge.app/Contents/MacOS/grillforge"),
            Path.home() / "Applications/GrillForge.app/Contents/MacOS/grillforge",
        ):
            if candidate.is_file():
                return str(candidate)
    return "grillforge"


def main() -> int:
    executable = resolve_executable()
    arguments = [executable, "selector", *sys.argv[1:]]
    entrypoint = os.environ.get("CLAUDE_CODE_ENTRYPOINT")
    if entrypoint:
        arguments.extend(("--claude-entrypoint", entrypoint))
    try:
        return subprocess.run(
            arguments,
            check=False,
        ).returncode
    except OSError as error:
        print(f"could not execute GrillForge selector: {error}", file=sys.stderr)
        return 127


if __name__ == "__main__":
    raise SystemExit(main())
