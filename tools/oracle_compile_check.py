#!/usr/bin/env python3
"""Which Java oracles (mods/horizon/src/test/java) compile against the prepared jars of the pinned version.

    python3 tools/oracle_compile_check.py            # per-file error counts, worst first
    python3 tools/oracle_compile_check.py --errors   # and the first error of each failing file

The oracles are written against one Minecraft version's classes. For another version (1.21.11) a file that does
not compile is a vanilla API that differs there: its harness cannot run until the oracle, and the Rust side it
checks, are ported. Server-side files need only the server libraries; client files also need the client's
(HORIZON_ALLOW_MISSING_CLIENT_LIBRARIES lets prepare go on without them, then client files report spurious
errors about lwjgl and the like: read those as unknown, not as differences)."""
import argparse
import collections
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

try:
    from . import lab
except ImportError:
    import lab


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--errors", action="store_true")
    args = parser.parse_args()
    prepared = json.loads((lab.GAME / "prepared.json").read_text(encoding="utf-8"))
    classpath = [lab.ROOT / prepared["jars"][side] for side in ("server", "client")]
    classpath += [lab.ROOT / p for side in ("server", "client") for p in prepared["libraries"][side]]
    source = lab.ROOT / "mods/horizon/src/test/java"
    files = sorted(source.rglob("*.java"))
    with tempfile.TemporaryDirectory() as out:
        run = subprocess.run(["javac", "--release", str(json.loads(lab.LOCK.read_text(encoding="utf-8"))["minecraft"]["java_major"]), "-proc:none", "-nowarn",
                              "-Xmaxerrs", "1000000", "-sourcepath", str(source), "-cp", ":".join(map(str, classpath)),
                              "-d", out, *map(str, files)], capture_output=True, text=True)
    errors = collections.defaultdict(list)
    for match in re.finditer(r"^(.*\.java):\d+: error: (.*)$", run.stderr, re.MULTILINE):
        errors[Path(match[1]).name].append(match[2])
    print(f"{len(files) - len(errors)} of {len(files)} oracle files compile; "
          f"{sum(map(len, errors.values()))} errors in {len(errors)} files")
    for name, found in sorted(errors.items(), key=lambda item: -len(item[1])):
        print(f"{len(found):5}  {name}" + (f"  {found[0][:100]}" if args.errors else ""))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
