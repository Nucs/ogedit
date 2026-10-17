#!/usr/bin/env python3
"""Summarize isolated QA process samples without third-party dependencies.

Each PID/generation is reported separately, including per-phase summaries.
First/last medians use up to ten samples; small windows can overlap. A phase's
span is wall-clock time from its first to last sample, including any gaps.
Windows psutil vms is pagefile/commit usage, not reserved virtual address space.
The legacy reserved_virtual_bytes column is therefore deliberately not used.
"""

import argparse
import csv
import json
import math
from pathlib import Path
import statistics
import sys


METRICS = ("private_bytes", "rss", "handles", "threads")
WINDOW = 10


def read_samples(path, pids=None):
    """Validate rows, filtering before aggregation so unrelated runs stay out."""
    samples = []
    with path.open(newline="", encoding="utf-8-sig") as stream:
        reader = csv.DictReader(stream)
        required = {"time", "pid", "phase", *METRICS}
        missing = required.difference(reader.fieldnames or ())
        if missing:
            raise ValueError("Missing CSV columns: " + ", ".join(sorted(missing)))
        for line, row in enumerate(reader, 2):
            try:
                pid = int(row["pid"])
                if pid <= 0:
                    raise ValueError("PID must be positive")
                if pids is not None and pid not in pids:
                    continue
                timestamp = float(row["time"])
                if not math.isfinite(timestamp):
                    raise ValueError("time must be finite")
                sample = {
                    "time": timestamp,
                    "pid": pid,
                    "generation": row.get("generation") or "unknown",
                    "phase": row["phase"] or "idle",
                }
                for name in METRICS:
                    sample[name] = int(row[name])
                    if sample[name] < 0:
                        raise ValueError(name + " must be non-negative")
                samples.append(sample)
            except (TypeError, ValueError) as error:
                raise ValueError(f"Invalid CSV row {line}: {error}") from error
    return sorted(samples, key=lambda sample: sample["time"])


def summarize_window(samples):
    first, last = samples[:WINDOW], samples[-WINDOW:]
    metrics = {}
    for name in METRICS:
        initial = statistics.median(sample[name] for sample in first)
        final = statistics.median(sample[name] for sample in last)
        metrics[name] = {
            "first10_median": initial,
            "last10_median": final,
            "median_delta": final - initial,
            "peak": max(sample[name] for sample in samples),
        }
    return {
        "sample_count": len(samples),
        "first_time": samples[0]["time"],
        "last_time": samples[-1]["time"],
        "span_seconds": samples[-1]["time"] - samples[0]["time"],
        "median_window_samples": len(first),
        "median_windows_overlap": len(samples) < WINDOW * 2,
        "metrics": metrics,
    }


def summarize(samples):
    runs = {}
    for sample in samples:
        runs.setdefault((sample["pid"], sample["generation"]), []).append(sample)
    result = {
        "median_window_limit": WINDOW,
        "memory_unit": "bytes",
        "span_definition": "Last minus first sample timestamp, including gaps.",
        "vms_note": "platform_vms is platform-specific; Windows vms is commit/pagefile usage, not reserved virtual address space. Legacy reserved_virtual_bytes is not summarized.",
        "runs": [],
    }
    for (pid, generation), rows in runs.items():
        phases = {}
        episodes = {}
        previous_phase = None
        for row in rows:
            phase = row["phase"]
            phases.setdefault(phase, []).append(row)
            if phase != previous_phase:
                episodes[phase] = episodes.get(phase, 0) + 1
            previous_phase = phase
        result["runs"].append({
            "pid": pid,
            "generation": generation,
            **summarize_window(rows),
            "phases": [
                {"phase": phase, "episodes": episodes[phase], **summarize_window(values)}
                for phase, values in phases.items()
            ],
        })
    return result


def display_metric(summary, name):
    values = summary["metrics"][name]
    scale = 1024 * 1024 if name in ("private_bytes", "rss") else 1
    first = values["first10_median"] / scale
    last = values["last10_median"] / scale
    peak = values["peak"] / scale
    return f"{first:.2f} -> {last:.2f} (peak {peak:.2f})"


def display(result):
    print("First/last 10-sample medians (or all available); private/RSS in MiB.")
    print("Phase span includes gaps. Median windows overlap below 20 samples.")
    for run in result["runs"]:
        print(f"\nPID {run['pid']} / generation {run['generation']}")
        for label, summary in [("TOTAL", run), *[(p["phase"], p) for p in run["phases"]]]:
            print(f"  {label}: n={summary['sample_count']}, span={summary['span_seconds']:.2f}s")
            print("    " + "; ".join(f"{name}: {display_metric(summary, name)}" for name in METRICS))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("metrics", type=Path, help="CSV from sample_process_memory.py")
    parser.add_argument("--pids", nargs="+", metavar="PID", help="Only these PIDs (space- or comma-separated)")
    parser.add_argument("--json", action="store_true", help="Print JSON instead of a text summary")
    parser.add_argument("--output", type=Path, help="Also write the JSON summary to this file")
    args = parser.parse_args()
    try:
        pids = None
        if args.pids:
            pids = {int(pid) for value in args.pids for pid in value.split(",")}
            if any(pid <= 0 for pid in pids):
                raise ValueError("--pids requires positive process IDs")
        if args.output and args.output.resolve() == args.metrics.resolve():
            raise ValueError("JSON output must not overwrite the input CSV")
        samples = read_samples(args.metrics, pids)
        if not samples:
            raise ValueError("No samples match the selected PIDs")
        if pids is not None:
            missing = pids.difference(sample["pid"] for sample in samples)
            if missing:
                raise ValueError("No samples for requested PID(s): " + ", ".join(map(str, sorted(missing))))
        result = summarize(samples)
        serialized = json.dumps(result, indent=2, allow_nan=False) + "\n"
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(serialized, encoding="utf-8")
        if args.json:
            sys.stdout.write(serialized)
        else:
            display(result)
    except (OSError, ValueError, csv.Error) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
