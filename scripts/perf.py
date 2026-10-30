#!/usr/bin/env python3
"""
Benchmark the hook and MCP tools, or summarise the perf.log they write.

Usage:
    python3 perf.py report [--log PATH] [--since HOURS]
    python3 perf.py bench  [--bin PATH] [--db PATH] [--runs N]

`report` reads <data_dir>/perf.log (default ~/.claude-memory-manager) and prints
latency percentiles and lock waits per hook/tool, plus the slowest phases.

`bench` snapshots the live DB into a temp dir (sqlite3 .backup), points the
binary at it with CLAUDE_MEMORY_DB_DIR, and times the hook and MCP tools. The
live DB is only read. Bench writes (memory_add, co-access edges) land in the
snapshot, which is deleted afterwards.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import subprocess
import tempfile
import time
import uuid
from collections import defaultdict
from datetime import datetime, timedelta, timezone
from pathlib import Path

DATA_DIR = Path(os.environ.get("CLAUDE_MEMORY_DB_DIR") or Path.home() / ".claude-memory-manager")
DEFAULT_BIN = "/Applications/Claude Memory Manager.app/Contents/MacOS/claude-memory-manager"


def percentile(values: list[float], p: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(len(ordered) * p))]


def print_stats(label: str, values: list[float], lock_waits: list[float] | None = None) -> None:
    line = (
        f"{label:<28} n={len(values):<5} p50={percentile(values, .5):7.1f}ms "
        f"p90={percentile(values, .9):7.1f}ms max={max(values):7.1f}ms"
    )
    if lock_waits is not None:
        waited = [w for w in lock_waits if w > 0]
        line += f"  lock-waited={len(waited)} (max {max(lock_waits):.0f}ms)"
    print(line)


def report(args: argparse.Namespace) -> None:
    log = Path(args.log)
    if not log.exists():
        raise SystemExit(f"no perf log at {log} — run a prompt with the new build first")

    cutoff = datetime.now(timezone.utc) - timedelta(hours=args.since) if args.since else None
    totals: dict[str, list[float]] = defaultdict(list)
    waits: dict[str, list[float]] = defaultdict(list)
    phases: dict[str, list[float]] = defaultdict(list)
    slowest: list[dict] = []

    for line in log.read_text().splitlines():
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if cutoff and datetime.fromisoformat(entry["ts"].replace("Z", "+00:00")) < cutoff:
            continue
        key = f"{entry['kind']}:{entry['name']}"
        totals[key].append(entry["total_ms"])
        waits[key].append(entry.get("lock_wait_ms", 0))
        for phase, ms in entry.get("phases", {}).items():
            phases[f"{key}/{phase}"].append(ms)
        slowest.append(entry)

    if not totals:
        raise SystemExit("no entries in range")

    print("== latency per call")
    for key in sorted(totals):
        print_stats(key, totals[key], waits[key])

    print("\n== phases (by p90)")
    for key in sorted(phases, key=lambda k: -percentile(phases[k], .9))[:12]:
        print_stats(key, phases[key])

    print("\n== slowest calls")
    for entry in sorted(slowest, key=lambda e: -e["total_ms"])[:5]:
        print(json.dumps(entry))


class McpClient:
    def __init__(self, binary: str, env: dict[str, str]):
        self.proc = subprocess.Popen(
            [binary, "--mcp-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1, env=env,
        )
        self.next_id = 0
        self.request("initialize", {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "perf-bench", "version": "0"},
        })
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, msg: dict) -> None:
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def request(self, method: str, params: dict) -> dict:
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        while line := self.proc.stdout.readline():
            msg = json.loads(line)
            if msg.get("id") == self.next_id:
                return msg
        raise SystemExit("MCP server exited")

    def timed_tool(self, name: str, arguments: dict) -> float:
        started = time.perf_counter()
        self.request("tools/call", {"name": name, "arguments": arguments})
        return (time.perf_counter() - started) * 1000

    def close(self) -> None:
        self.proc.terminate()


def run_hook(binary: str, env: dict[str, str], event: dict) -> float:
    started = time.perf_counter()
    subprocess.run([binary, "--hook"], input=json.dumps(event), text=True,
                   capture_output=True, env=env, check=False)
    return (time.perf_counter() - started) * 1000


def bench(args: argparse.Namespace) -> None:
    snapshot_dir = Path(tempfile.mkdtemp(prefix="cmm-bench-"))
    try:
        source = sqlite3.connect(f"file:{args.db}?mode=ro", uri=True)
        target = sqlite3.connect(snapshot_dir / "memories.db")
        source.backup(target)
        memories, edges = target.execute(
            "SELECT (SELECT COUNT(*) FROM memories), (SELECT COUNT(*) FROM memory_edges)"
        ).fetchone()
        source.close()
        target.close()
        print(f"snapshot: {memories} memories, {edges} edges, "
              f"{(snapshot_dir / 'memories.db').stat().st_size / 1e6:.1f} MB\n")

        env = {**os.environ, "CLAUDE_MEMORY_DB_DIR": str(snapshot_dir)}
        empty_transcript = snapshot_dir / "transcript.jsonl"
        empty_transcript.write_text("{}\n")
        prompts = [
            "deploy the app and roll back if it fails",
            "the add and read in the memory manager is getting slower, investigate sqlite fts",
            "where do we keep the database migrations",
            "release tauri latest.json signed bundle",
            "write tests for the invoice wizard",
        ]

        def hook_event(prompt: str, transcript: Path) -> dict:
            return {"prompt": prompt, "session_id": "perf-bench", "cwd": str(Path.cwd()),
                    "transcript_path": str(transcript), "hook_event_name": "UserPromptSubmit"}

        run_hook(args.bin, env, hook_event(prompts[0], empty_transcript))
        print_stats("hook", [run_hook(args.bin, env, hook_event(prompts[i % len(prompts)], empty_transcript))
                             for i in range(args.runs)])

        transcripts = sorted(Path.home().glob(".claude*/projects/*/*.jsonl"), key=lambda p: -p.stat().st_size)
        if transcripts:
            big = transcripts[0]
            print_stats(f"hook ({big.stat().st_size / 1e6:.0f} MB transcript)",
                        [run_hook(args.bin, env, hook_event(prompts[i % len(prompts)], big))
                         for i in range(args.runs)])

        started = time.perf_counter()
        client = McpClient(args.bin, env)
        print_stats("mcp spawn+init", [(time.perf_counter() - started) * 1000])
        print_stats("memory_search", [client.timed_tool("memory_search", {"query": prompts[i % len(prompts)], "limit": 10})
                                      for i in range(args.runs)])
        print_stats("memory_list", [client.timed_tool("memory_list", {"limit": 50}) for _ in range(args.runs)])
        for scope in ("/tmp/perf-bench", "global"):
            print_stats(f"memory_add ({'project' if scope != 'global' else 'global'})", [
                client.timed_tool("memory_add", {
                    "title": f"perf bench {uuid.uuid4()}",
                    "content": f"Benchmark content {uuid.uuid4().hex} about widget frobnication {i}",
                    "type": "project" if scope != "global" else "reference",
                    "project": scope,
                })
                for i in range(args.runs)
            ])
        client.close()

        if (snapshot_dir / "perf.log").exists():
            print("\n== in-process breakdown of the runs above")
            report(argparse.Namespace(log=snapshot_dir / "perf.log", since=None))
    finally:
        shutil.rmtree(snapshot_dir, ignore_errors=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    report_cmd = sub.add_parser("report", help="summarise perf.log")
    report_cmd.add_argument("--log", default=str(DATA_DIR / "perf.log"))
    report_cmd.add_argument("--since", type=float, help="only entries from the last N hours")
    report_cmd.set_defaults(func=report)

    bench_cmd = sub.add_parser("bench", help="benchmark against a snapshot of the DB")
    bench_cmd.add_argument("--bin", default=DEFAULT_BIN)
    bench_cmd.add_argument("--db", default=str(DATA_DIR / "memories.db"))
    bench_cmd.add_argument("--runs", type=int, default=20)
    bench_cmd.set_defaults(func=bench)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
