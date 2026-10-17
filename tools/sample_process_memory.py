#!/usr/bin/env python3
"""Read-only metrics for the workspace's isolated browser QA editor.

Requires psutil. The browser harness still drives every UI action; this sampler
only records process counters. Private bytes measure committed private memory,
not the much larger reserved virtual address space used by OGEdit's arenas.
platform_vms preserves psutil's platform-specific vms counter: on Windows it is
pagefile/commit usage, not reserved virtual address space.
"""
import argparse
import csv
import json
from pathlib import Path
import time
import urllib.request

import psutil


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--phase", type=Path, required=True)
    parser.add_argument("--stop", type=Path, required=True)
    parser.add_argument("--url", default="http://127.0.0.1:8766/state")
    parser.add_argument("--seconds", type=float, default=7200)
    args = parser.parse_args()
    workspace = Path(__file__).resolve().parents[1]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + args.seconds
    with args.output.open("w", newline="", encoding="utf-8") as stream:
        fields = ["time", "pid", "generation", "phase", "private_bytes", "rss", "platform_vms", "handles", "threads", "cpu_seconds"]
        writer = csv.DictWriter(stream, fieldnames=fields)
        writer.writeheader()
        while time.monotonic() < deadline and not args.stop.exists():
            try:
                with urllib.request.urlopen(args.url, timeout=2) as response:
                    state = json.load(response)
                if state.get("alive"):
                    expected = Path(state["exe"]).resolve()
                    if expected.name != "ogedit.exe" or not expected.is_relative_to(workspace / "target"):
                        raise ValueError("Refusing to sample an unrelated process")
                    process = psutil.Process(state["pid"])
                    if Path(process.exe()).resolve() != expected:
                        raise ValueError("QA process identity changed")
                    memory = process.memory_info()
                    private = getattr(memory, "private", None)
                    if private is None:
                        private = process.memory_full_info().uss
                    cpu = process.cpu_times()
                    writer.writerow(dict(time=time.time(), pid=process.pid, generation=state["generation"],
                        phase=args.phase.read_text(encoding="utf-8").strip() if args.phase.exists() else "idle",
                        private_bytes=private, rss=memory.rss, platform_vms=memory.vms,
                        handles=process.num_handles() if hasattr(process, "num_handles") else process.num_fds(),
                        threads=process.num_threads(), cpu_seconds=cpu.user + cpu.system))
                    stream.flush()
            except (OSError, ValueError, KeyError, psutil.Error):
                pass  # Startup/exit transitions do not create misleading zero samples.
            time.sleep(0.5)


if __name__ == "__main__":
    main()
