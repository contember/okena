#!/usr/bin/env python3
"""Merge Okena desktop and daemon terminal-latency probe logs."""

from __future__ import annotations

import argparse
import csv
import math
import re
import statistics
import sys
from pathlib import Path

FIELD_RE = re.compile(r"([a-z_]+)=([^\s]+)")

STAGES = [
    ("client_to_daemon_ms", "Client send → daemon WebSocket receive"),
    ("daemon_bridge_ms", "Daemon WebSocket → command loop"),
    ("daemon_to_pty_queue_ms", "Command loop → PTY writer queue"),
    ("pty_writer_queue_ms", "PTY writer queue wait"),
    ("pty_write_ms", "PTY write"),
    ("pty_echo_ms", "dtach/PTY/application echo"),
    ("daemon_fanout_ms", "PTY read → daemon stream queue"),
    ("return_transport_ms", "Daemon stream queue → client receive"),
    ("client_parse_ms", "Client receive → terminal parse"),
    ("activity_emit_ms", "Parse → TerminalActivity emit"),
    ("activity_delivery_ms", "TerminalActivity emit → Okena"),
    ("throttle_ms", "Okena activity → repaint dispatch"),
    ("notify_fanout_ms", "Repaint dispatch → pane notify"),
    ("notify_to_paint_ms", "Pane notify → paint"),
    ("input_to_paint_ms", "Client send → terminal paint"),
    ("paint_to_frame_ms", "Paint → next GPUI frame callback"),
    ("total_ms", "Client send → next GPUI frame callback"),
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("client_log", type=Path)
    parser.add_argument("daemon_log", type=Path)
    parser.add_argument("--terminal", help="only include one normalized terminal id")
    parser.add_argument("--last", type=int, help="only include the last N matched samples")
    parser.add_argument("--csv", action="store_true", help="print matched samples as CSV")
    return parser.parse_args()


def parse_records(path: Path, marker: str) -> list[dict[str, str]]:
    records = []
    for line in path.read_text(errors="replace").splitlines():
        if marker not in line:
            continue
        fields = dict(FIELD_RE.findall(line))
        if fields:
            records.append(fields)
    return records


def normalized_terminal(terminal_id: str) -> str:
    if terminal_id.startswith("remote:"):
        parts = terminal_id.split(":", 2)
        if len(parts) == 3:
            return parts[2]
    return terminal_id


def record_signature(record: dict[str, str]) -> tuple[str, str, str]:
    return (
        normalized_terminal(record["terminal"]),
        record["input_len"],
        record["input_hash"],
    )


def timestamp(record: dict[str, str], name: str) -> int | None:
    value = int(record.get(name, "0"))
    return value if value > 0 else None


def elapsed_ms(start: int | None, end: int | None) -> float | None:
    if start is None or end is None or end < start:
        return None
    return (end - start) / 1_000


def merge_samples(
    clients: list[dict[str, str]], daemons: list[dict[str, str]]
) -> list[dict[str, object]]:
    daemon_by_signature: dict[tuple[str, str, str], list[dict[str, str]]] = {}
    for daemon in daemons:
        daemon_by_signature.setdefault(record_signature(daemon), []).append(daemon)

    rows = []
    for client in clients:
        signature = record_signature(client)
        input_us = timestamp(client, "input_us")
        candidates = daemon_by_signature.get(signature, [])
        daemon = min(
            (
                candidate
                for candidate in candidates
                if input_us is not None
                and timestamp(candidate, "ws_receive_us") is not None
                and timestamp(candidate, "ws_receive_us") >= input_us
            ),
            key=lambda candidate: timestamp(candidate, "ws_receive_us") - input_us,
            default=None,
        )
        if daemon is None:
            continue
        candidates.remove(daemon)

        output_receive_us = timestamp(client, "output_receive_us")
        parsed_us = timestamp(client, "parsed_us")
        activity_emit_us = timestamp(client, "activity_emit_us")
        activity_receive_us = timestamp(client, "activity_receive_us")
        throttle_fire_us = timestamp(client, "throttle_fire_us")
        notify_us = timestamp(client, "notify_us")
        paint_us = timestamp(client, "paint_us")
        frame_us = timestamp(client, "frame_us")

        ws_receive_us = timestamp(daemon, "ws_receive_us")
        bridge_us = timestamp(daemon, "bridge_us")
        pty_queue_us = timestamp(daemon, "pty_queue_us")
        pty_write_start_us = timestamp(daemon, "pty_write_start_us")
        pty_write_end_us = timestamp(daemon, "pty_write_end_us")
        pty_output_us = timestamp(daemon, "pty_output_us")
        stream_us = timestamp(daemon, "stream_us")

        rows.append(
            {
                "terminal": signature[0],
                "sample": int(client["sample"]),
                "daemon_sample": int(daemon["sample"]),
                "client_to_daemon_ms": elapsed_ms(input_us, ws_receive_us),
                "daemon_bridge_ms": elapsed_ms(ws_receive_us, bridge_us),
                "daemon_to_pty_queue_ms": elapsed_ms(bridge_us, pty_queue_us),
                "pty_writer_queue_ms": elapsed_ms(pty_queue_us, pty_write_start_us),
                "pty_write_ms": elapsed_ms(pty_write_start_us, pty_write_end_us),
                "pty_echo_ms": elapsed_ms(pty_write_end_us, pty_output_us),
                "daemon_fanout_ms": elapsed_ms(pty_output_us, stream_us),
                "return_transport_ms": elapsed_ms(stream_us, output_receive_us),
                "client_parse_ms": elapsed_ms(output_receive_us, parsed_us),
                "activity_emit_ms": elapsed_ms(parsed_us, activity_emit_us),
                "activity_delivery_ms": elapsed_ms(activity_emit_us, activity_receive_us),
                "throttle_ms": elapsed_ms(activity_receive_us, throttle_fire_us),
                "notify_fanout_ms": elapsed_ms(throttle_fire_us, notify_us),
                "notify_to_paint_ms": elapsed_ms(notify_us, paint_us),
                "input_to_paint_ms": elapsed_ms(input_us, paint_us),
                "paint_to_frame_ms": elapsed_ms(paint_us, frame_us),
                "total_ms": elapsed_ms(input_us, frame_us),
            }
        )
    return rows


def percentile(values: list[float], percentile_value: float) -> float:
    ordered = sorted(values)
    index = max(0, math.ceil(len(ordered) * percentile_value) - 1)
    return ordered[index]


def print_summary(rows: list[dict[str, object]]) -> None:
    print(f"Matched samples: {len(rows)}")
    print()
    print(f"{'Stage':43} {'median':>9} {'p95':>9} {'max':>9}")
    for field, label in STAGES:
        values = [row[field] for row in rows if isinstance(row[field], float)]
        if not values:
            continue
        print(
            f"{label:43} "
            f"{statistics.median(values):8.3f} "
            f"{percentile(values, 0.95):8.3f} "
            f"{max(values):8.3f}"
        )


def print_csv(rows: list[dict[str, object]]) -> None:
    fields = [
        "terminal",
        "sample",
        "daemon_sample",
        *(field for field, _label in STAGES),
    ]
    writer = csv.DictWriter(sys.stdout, fieldnames=fields)
    writer.writeheader()
    writer.writerows(rows)


def main() -> int:
    args = parse_args()
    clients = parse_records(args.client_log, "terminal_latency_client ")
    daemons = parse_records(args.daemon_log, "terminal_latency_daemon ")
    rows = merge_samples(clients, daemons)
    if args.terminal:
        rows = [row for row in rows if row["terminal"] == args.terminal]
    if args.last is not None:
        rows = rows[-args.last :]
    if not rows:
        print("No matching latency samples found.", file=sys.stderr)
        return 1
    if args.csv:
        print_csv(rows)
    else:
        print_summary(rows)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
