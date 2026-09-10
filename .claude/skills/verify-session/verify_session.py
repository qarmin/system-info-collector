#!/usr/bin/env python3
"""Check a recorded session for internal consistency and summarise what it says.

Covers the checks that need only the file itself: structural invariants, the
rate-to-total reconciliation, the recorder's own cost, and the device-versus-process
attribution. Ground-truth checks against a synthetic writer and the HTTP surface
need a live machine and stay in SKILL.md.

    verify_session.py <session.json> [more.json ...]

Exit code is the number of failed checks, so it can gate a script.
"""

import json
import sys
from pathlib import Path

MB = 1024 * 1024
# Per-tick arrays that must all line up with the tick index array.
SAMPLE_ARRAYS = (
    "cpu", "rss_mb", "rchar", "wchar", "read_bytes", "written_bytes",
    "inherited_wchar", "inherited_written", "threads", "vol_ctx", "nonvol_ctx",
)
SYSTEM_SERIES = (
    "t_ms", "cpu_pct", "mem_used_mb", "disk_read_mbs", "disk_write_mbs",
    "proc_wchar_mbs", "proc_written_mbs",
)


class Checker:
    def __init__(self):
        self.failed = 0

    def check(self, ok, label, detail=""):
        mark = "ok  " if ok else "FAIL"
        print(f"  [{mark}] {label}" + (f" - {detail}" if detail else ""))
        if not ok:
            self.failed += 1
        return ok

    def note(self, text):
        print(f"         {text}")


def fmt(byte_count):
    if byte_count >= MB * 1024:
        return f"{byte_count / MB / 1024:.2f} GB"
    return f"{byte_count / MB:.2f} MB"


def structural(d, c):
    print("\nStructure")
    meta, system, procs, samples = d["meta"], d["system"], d["procs"], d["samples"]
    ticks = len(system["t_ms"])

    c.check(meta["format_version"] == 1, "format version", str(meta["format_version"]))
    c.check(meta["ticks"] == ticks, "meta.ticks matches the system series", f"{meta['ticks']} vs {ticks}")

    bad = [k for k in SYSTEM_SERIES if len(system.get(k, [])) != ticks]
    c.check(not bad, "every system series has one entry per tick", ", ".join(bad))

    c.check(
        all(system["t_ms"][i] <= system["t_ms"][i + 1] for i in range(ticks - 1)),
        "tick timestamps ascend",
    )

    misaligned, unsorted_, out_of_range = [], [], []
    for pid, s in samples.items():
        n = len(s["t"])
        if any(len(s.get(f, [])) != n for f in SAMPLE_ARRAYS) or len(s.get("status", "")) != n:
            misaligned.append(pid)
        if s["t"] != sorted(s["t"]):
            unsorted_.append(pid)
        if n and (s["t"][-1] >= ticks or s["t"][0] < 0):
            out_of_range.append(pid)
    c.check(not misaligned, "per-tick arrays aligned with tick indices", f"{len(misaligned)} bad")
    c.check(not unsorted_, "sample tick indices ascend", f"{len(unsorted_)} bad")
    c.check(not out_of_range, "sample tick indices within range", f"{len(out_of_range)} bad")

    ids = {p["id"] for p in procs}
    c.check(all(int(k) in ids for k in samples), "every sample belongs to a listed process")
    c.check(len(ids) == len(procs), "process ids are unique")

    paths = d.get("paths", [])
    dangling = [i for snaps in d.get("fds", {}).values() for s in snaps for i in s["paths"] if not 0 <= i < len(paths)]
    c.check(not dangling, "interned file paths resolve", f"{len(dangling)} dangling")

    lifetimes = [p for p in procs if p["first_tick"] > p["last_tick"]]
    c.check(not lifetimes, "first_tick <= last_tick for every process", f"{len(lifetimes)} inverted")


def rates_vs_totals(d, c):
    """A tall spike next to a small total is the most-reported non-bug; assert it."""
    print("\nRates reconcile with totals")
    meta, system = d["meta"], d["system"]
    tick = 1.0 / meta["hz"]

    for series, total_key in (
        ("disk_write_mbs", "device_written_bytes"),
        ("disk_read_mbs", "device_read_bytes"),
        ("proc_wchar_mbs", "proc_wchar_bytes"),
        ("proc_written_mbs", "proc_written_bytes"),
    ):
        from_rates = sum(system[series]) * tick
        from_bytes = meta[total_key] / MB
        tolerance = max(0.05, from_bytes * 0.02)
        c.check(
            abs(from_rates - from_bytes) <= tolerance,
            f"{series} sums to {total_key}",
            f"{from_rates:.2f} MB vs {from_bytes:.2f} MB",
        )


def peaks(d, c):
    print("\nPeaks in context")
    meta, system = d["meta"], d["system"]
    tick = 1.0 / meta["hz"]
    seconds = max(meta["recorded_duration_secs"], tick)

    for label, series, total in (
        ("device write", "disk_write_mbs", meta["device_written_bytes"]),
        ("device read", "disk_read_mbs", meta["device_read_bytes"]),
        ("process write (syscall)", "proc_wchar_mbs", meta["proc_wchar_bytes"]),
    ):
        values = system[series]
        if not values:
            continue
        peak = max(values)
        over_half = sum(1 for v in values if v >= peak / 2)
        c.note(
            f"{label:24} peak {peak:7.2f} MB/s = {peak * tick:5.2f} MB in one {tick * 1000:.0f} ms sample | "
            f"avg {total / MB / seconds:6.2f} MB/s | total {fmt(total):>9} | {over_half} samples at >= half peak"
        )


def coverage(d, c):
    """Unreadable counters look exactly like idle ones, so check this before
    drawing any conclusion from the attribution figures."""
    print("\nI/O coverage")
    meta, procs = d["meta"], d["procs"]
    total = len(procs)

    # Older recordings have no io_readable field; fall back to "any counter moved".
    if any("io_readable" in p for p in procs):
        readable = [p for p in procs if p.get("io_readable")]
        missing = meta.get("processes_without_io", total - len(readable))
    else:
        readable = [
            p for p in procs
            if any(p["io_total"][f] > 0 for f in ("rchar", "wchar", "read_bytes", "written_bytes"))
        ]
        missing = total - len(readable)
        c.note("recording predates the io_readable flag - inferring coverage from nonzero counters")

    share = len(readable) / total * 100 if total else 0.0
    c.note(f"counters readable for {len(readable)} of {total} processes ({share:.1f}%)")

    kernel = [p for p in procs if p["kernel"]]
    kernel_with_io = [p for p in kernel if any(p["io_total"][f] > 0 for f in ("rchar", "wchar", "read_bytes", "written_bytes"))]
    c.note(f"kernel threads: {len(kernel)}, of which any counter moved: {len(kernel_with_io)}")

    ok = missing == 0
    c.check(
        ok,
        "I/O readable for every process",
        "" if ok else f"{missing} unreadable - their zeros mean UNKNOWN, re-record as root",
    )
    if not ok:
        c.note("without this, the device-versus-process gap below is not evidence of anything")
        uids = {p.get("uid") for p in readable if p.get("uid") is not None}
        if uids:
            c.note(f"readable processes belong to uid(s) {sorted(uids)}; recorder ran as uid {meta.get('recorder_uid')}")
    return ok


def attribution(d, c, coverage_ok=True):
    print("\nAttribution")
    meta, procs = d["meta"], d["procs"]
    device_w = meta["device_written_bytes"]
    proc_w = meta["proc_wchar_bytes"]
    unclaimed = device_w - proc_w

    c.note(f"device wrote {fmt(device_w)}, processes claimed {fmt(proc_w)} through write()")
    if device_w > 0:
        share = unclaimed / device_w * 100
        cause = (
            "unreadable counters - fix coverage before reading anything into this"
            if not coverage_ok
            else "write amplification, swap, metadata, or a process too short-lived to sample"
        )
        c.note(f"unclaimed {fmt(unclaimed)} ({share:+.0f}%) - {cause}")

    recorder = next((p for p in procs if p["pid"] == meta.get("recorder_pid")), None)
    if recorder is None:
        c.check(False, "recorder process present in the recording", f"pid {meta.get('recorder_pid')}")
        return

    own_r = recorder["io_total"]["rchar"] - recorder["io_at_start"]["rchar"]
    own_dev_r = recorder["io_total"]["read_bytes"] - recorder["io_at_start"]["read_bytes"]
    seconds = max(meta["recorded_duration_secs"], 1e-6)
    c.check(True, "recorder identified", f"{recorder['name']} pid {recorder['pid']}")
    c.note(f"its syscall reads {fmt(own_r)} ({own_r / MB / seconds:.2f} MB/s) - /proc, not disk")
    c.check(
        own_dev_r < 8 * MB,
        "recorder did almost no device reads",
        f"{fmt(own_dev_r)}",
    )
    if meta["proc_rchar_bytes"] > 0:
        c.note(f"it is {own_r / meta['proc_rchar_bytes'] * 100:.0f}% of all syscall reads in the recording")

    # Per-process cumulative counters must never run backwards.
    backwards = [
        p["pid"] for p in procs
        for field in ("rchar", "wchar", "read_bytes", "written_bytes")
        if p["io_total"][field] < p["io_at_start"][field]
    ]
    c.check(not backwards, "cumulative counters never decrease", f"pids {backwards[:5]}")


def device_table(d, c):
    """A machine total that looks impossible is usually one write counted twice."""
    devs = d["meta"].get("devices")
    if devs is None:
        return
    print("\nDevices")
    for dev in sorted(devs, key=lambda x: -(x["written_bytes"] + x["read_bytes"])):
        tag = "counted" if dev["counted"] else f"skipped: {dev['skipped_because']}"
        c.note(f"{dev['name']:<18} write {fmt(dev['written_bytes']):>10}  read {fmt(dev['read_bytes']):>10}  {tag}")
    counted = sum(dev["written_bytes"] for dev in devs if dev["counted"])
    c.check(
        counted == d["meta"]["device_written_bytes"],
        "counted devices sum to the machine total",
        f"{fmt(counted)} vs {fmt(d['meta']['device_written_bytes'])}",
    )


def top_movers(d, count=8):
    print("\nBiggest movers over the whole recording")
    meta, procs, samples = d["meta"], d["procs"], d["samples"]
    seconds = max(meta["recorded_duration_secs"], 1e-6)
    rows = []
    for p in procs:
        s = samples.get(str(p["id"]))
        rows.append({
            "name": p["name"][:24],
            "pid": p["pid"],
            "kernel": p["kernel"],
            "recorder": p["pid"] == meta.get("recorder_pid"),
            "noio": p.get("io_readable") is False,
            "w": sum(s["wchar"]) if s else 0,
            "wdev": sum(s["written_bytes"]) if s else 0,
            "r": sum(s["rchar"]) if s else 0,
            "cpu": (sum(s["cpu"]) / meta["ticks"]) if s and meta["ticks"] else 0.0,
        })
    rows.sort(key=lambda r: -(r["w"] + r["wdev"]))

    print(f"  {'process':<26}{'pid':>8}{'write':>11}{'write dev':>11}{'read':>11}{'cpu avg':>9}  flags")
    for r in rows[:count]:
        flags = " ".join(
            f for f, on in (("kernel", r["kernel"]), ("RECORDER", r["recorder"]), ("NO-IO-DATA", r["noio"])) if on
        )
        print(
            f"  {r['name']:<26}{r['pid']:>8}{fmt(r['w']):>11}{fmt(r['wdev']):>11}"
            f"{fmt(r['r']):>11}{r['cpu']:>8.1f}%  {flags}"
        )
    print(f"  (device total {fmt(meta['device_written_bytes'])} written, "
          f"{meta['device_written_bytes'] / MB / seconds:.2f} MB/s average)")


def summarise(d):
    meta = d["meta"]
    print(f"  {meta['ticks']} samples at {meta['hz']} Hz over {meta['recorded_duration_secs']:.1f} s"
          f"{' (stopped early)' if meta['aborted'] else ''}")
    print(f"  {meta['process_count']} processes, {sum(1 for p in d['procs'] if p['kernel'])} of them kernel threads")
    print(f"  {meta['cpu_cores']} cores, {meta['total_memory_mb'] / 1024:.1f} GB RAM, "
          f"app {meta['app_version']}, procfs {meta['proc_fs']}")


def main(paths):
    total_failed = 0
    for path in paths:
        print(f"\n{'=' * 78}\n{path}\n{'=' * 78}")
        d = json.loads(Path(path).read_text())
        summarise(d)
        c = Checker()
        structural(d, c)
        rates_vs_totals(d, c)
        peaks(d, c)
        coverage_ok = coverage(d, c)
        device_table(d, c)
        attribution(d, c, coverage_ok)
        top_movers(d)
        print(f"\n  {c.failed} failed check(s)")
        total_failed += c.failed
    return total_failed


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    sys.exit(main(sys.argv[1:]))
