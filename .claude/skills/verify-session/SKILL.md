---
name: verify-session
description: Verify that a process session recording reports the truth - records a session with a synthetic workload of known size, then checks the recorded numbers against /proc read independently, checks the internal invariants of the file, and exercises the viewer's aggregation headless. Use after changing session_recorder.rs, session_viewer.html, session_store.rs or session_service.rs, or whenever a recorded figure looks implausible.
---

# Verifying a session recording

The session recorder is easy to break silently. Its numbers look plausible whatever
they are, so "it compiled and produced a file" says almost nothing. Every check
below exists because something actually went wrong that way once.

Run the checks in order and stop at the first failure - the later ones assume the
earlier ones hold.

## 0. Preconditions

```bash
cargo build
cargo clippy --all-targets 2>&1 | grep -c '^error'   # must be 0
cargo test 2>&1 | grep 'test result'
```

Reading `/proc` for other processes may need the sandbox off. If `ls /proc | grep -c
'^[0-9]'` returns a handful on a machine that clearly runs more, you are in a
container with its own PID namespace: the checks still work, but process counts and
thread multipliers will not resemble a desktop, so do not draw conclusions about
cost from them.

## 1. Ground truth: does an attributed byte count match `/proc`?

The point of the tool is attribution. Verify it against a writer whose volume you
chose yourself, and read `/proc` yourself rather than trusting the recording.

Write a long-lived writer - long-lived matters, a process shorter than one sampling
interval is legitimately invisible:

```python
# /tmp/verify_writer.py - 2 MB every 0.5 s, the shape of a real complaint
import os, time
fd = os.open('/tmp/verify/target.bin', os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
print(os.getpid(), flush=True)
blob = b'x' * (2 * 1024 * 1024)
deadline = time.time() + 20
while time.time() < deadline:
    os.lseek(fd, 0, os.SEEK_SET)
    os.write(fd, blob)
    os.fsync(fd)
    time.sleep(0.5)
```

Start it, note the pid, read `wchar` from `/proc/<pid>/io` before and after, and
record a session across the middle. Then assert:

- the writer's pid appears in `procs`
- `io_total.wchar` equals the `wchar` you read from `/proc` after the session
- the sum of that process's per-tick `wchar` is close to the bytes written *during*
  the recording

Beware the timing trap that has produced three false alarms already: bytes written
between starting the writer and the recorder's first sample belong to
`io_at_start`, not to the session. Compute the expected session share as
`io_total - io_at_start` and compare against that, never against what you told the
writer to write.

## 2. Internal invariants, rates, totals and attribution - scripted

Most of what follows is automated. Run it first on any recording, including one a
user hands over:

```bash
python3 .claude/skills/verify-session/verify_session.py <recording.json> [...]
```

It exits with the number of failed checks and prints, for each file: structural
invariants, the rate-to-total reconciliation for all four series, peaks in context
(peak rate, the bytes that peak represents, average, total, how many samples are
near the peak), the device-versus-process attribution gap, the recorder's own cost
and share, and the biggest movers. Read the numbers - a clean run is not the point,
the numbers are.

The remaining sections cover what the script cannot do: ground truth against a
writer you control, the viewer's behaviour, and the HTTP surface.

### What the script checks, for reference

```python
import json
d = json.load(open(path))
s = next(iter(d['samples'].values()))

# Every per-tick array is aligned with the tick index array.
lens = {k: len(v) for k, v in s.items() if isinstance(v, list)}
assert len(set(lens.values())) == 1, lens
assert len(s['status']) == len(s['t'])          # status is a char per tick

# System series are one entry per recorded tick.
n = len(d['system']['t_ms'])
assert d['meta']['ticks'] == n
for key, series in d['system'].items():
    assert len(series) == n, key

# Tick indices ascend and stay in range.
assert s['t'] == sorted(s['t']) and max(s['t']) < n

# Interned paths resolve.
for snaps in d['fds'].values():
    for snap in snaps:
        for i in snap['paths']:
            assert 0 <= i < len(d['paths'])
```

## 3. Device totals against `/proc/diskstats`

`meta.devices` carries the per-device breakdown, including the rows deliberately
left out of the machine total and why. When a device total looks impossibly large,
read that list before suspecting anything else: `/proc/diskstats` shows the same
hardware under several names - a disk and its partitions, a device-mapper or
software-RAID target and its members, and on kernels with NVMe multipath a
`nvme0c0n1` alias beside `nvme0n1`. Only one view of each is counted; the sum of
the rows marked `counted` must equal `device_written_bytes`.

If a duplicate family shows up that `duplicate_reason()` does not recognise, that
is a real bug and the machine total is inflated by a whole multiple.

Expect `device_written_bytes` to exceed `proc_wchar_bytes` on a copy-on-write or
journalling filesystem - that is write amplification, not a bug. But a gap of two
orders of magnitude is not amplification: look for a periodic pattern in the
spike timing (the script prints the near-peak sample count), and remember that
writes performed by the kernel on nobody's behalf - swap, RAID resync, filesystem
journalling - are charged to no task at all and will never appear in the table.

## 3b. Rates must reconcile with totals

The single most reported "bug" in this tool is not one: a chart touching 10 MB/s
next to a 7 MB total looks contradictory, but each point covers one sampling
interval, so that peak is worth `10 MB/s x 0.2 s = 2 MB`. Assert the invariant
rather than eyeballing it:

```python
tick = 1.0 / d['meta']['hz']
from_rates = sum(r * tick for r in d['system']['disk_write_mbs'])
from_bytes = d['meta']['device_written_bytes'] / 2**20
assert abs(from_rates - from_bytes) < max(0.05, from_bytes * 0.02), (from_rates, from_bytes)
```

If this fails, the rate and the total have drifted apart - look at `elapsed_secs`
in the tick loop, since both figures are supposed to derive from the same byte
delta.

## 3c. Coverage before conclusions

`/proc/<pid>/io` is mode 0400 and needs permission over the target, so a recording
made as an ordinary user has counters for its own processes and **nothing else** -
no system daemons, no kernel threads, therefore no writeback. Reporting zeros there
is indistinguishable from "wrote nothing" and has already sent one investigation
down the wrong path for two rounds.

The script checks this first. Signature of the problem, from a real recording:

```
counters readable for 20 of 1386 processes (1.4%)
kernel threads: 915, of which any counter moved: 0
```

`postgres: checkpointer` sitting at zero while the disk takes 47 MB is the tell.

When coverage is incomplete, the device-versus-process gap means nothing - do not
report it as unattributed traffic. Re-record with privilege:

```bash
sudo ./system_info_collector session --duration 60
# or grant the binary both capabilities once - either one alone still fails:
sudo setcap cap_dac_read_search,cap_sys_ptrace+ep ./system_info_collector
# or, for a deployed service, let systemd do it (already in the shipped unit):
#   AmbientCapabilities=CAP_DAC_READ_SEARCH CAP_SYS_PTRACE
```

`setcap` reports `Invalid argument` for a misspelt capability and `Operation not
permitted` when it merely lacks privilege - useful for telling a typo from a
sandbox. The capabilities are xattrs, so they are lost on every `scp`; `just
grant_caps <ip>` reapplies them and the send recipes already do.

After re-recording, confirm the fix rather than assuming it: coverage should read
close to 100%, and kernel threads should stop being uniformly zero.

## 4. The recorder must not look like a culprit

The recorder reads several `/proc` files per process per sample; all of it lands on
its own `rchar`. Check that:

- `meta.recorder_pid` is present and matches a row in `procs`
- that row's `rchar` over the session is large while its `read_bytes` is ~0 -
  procfs reads are not disk traffic
- the volume is proportionate. Measure it directly:

```python
r = (p['io_total']['rchar'] - p['io_at_start']['rchar']) / 2**20
print(f"{r:.2f} MB, {r / d['meta']['recorded_duration_secs']:.2f} MB/s")
```

If this jumps after a change to the refresh flags, suspect `ProcessRefreshKind`.
`ProcessRefreshKind::nothing()` leaves `tasks` **enabled**, which makes sysinfo read
stat and statm for every thread - several thousand extra files per tick on a
desktop, all discarded afterwards. `.without_tasks()` is required, and dropping it
cost a measured 4.4x.

To quantify a regression, patch the suspect line out, build, record, measure,
restore. Do not estimate.

## 5. Viewer aggregation, headless

The viewer holds all the aggregation logic and none of it is covered by
`cargo test`. The CPU part of that is scripted - it loads a recording, runs
`buildRows()` grouped and ungrouped, and checks the figures against the sample
arrays read independently:

```bash
node .claude/skills/verify-session/verify_viewer.js <recording.json>
```

It asserts what went wrong once already: that no row reports a CPU max below its
own CPU avg (a grouped row's average is its members added together, so its peak
has to be their combined peak, not the largest single member), that a grouped
row's max equals the peak of the members' per-tick sum, and that a system CPU
series of all zeros is labelled as missing rather than drawn as an idle machine.

For anything it does not cover, extract the script and run it under node with a
stub DOM the same way:

```bash
python3 -c "
import re
h = open('crates/system_info_collector/src/serving/session_viewer.html').read()
m = re.search(r'<script>\n(.*)</script>', h, re.S)
open('/tmp/v.js','w').write(m.group(1))
"
node --check /tmp/v.js
```

Then `eval` it with `document.getElementById` returning stubs, `Chart` a class
exposing `scales.x.getPixelForValue`, and `window.SESSION_DATA` set to a real
recording, and publish the internals onto `globalThis` at the end of the eval -
`const` bindings created inside `eval` are not visible to the enclosing scope.

Worth asserting:

- `buildRows()` row count changes with the `showKernel` checkbox
- sorting by any column key does not throw, and `sortBy` preserves the scroll
  position of `.table-scroll`
- a selection narrower than the session produces a narrower band: drive
  `drawBrush()` and record the `fillRect` calls
- the band is identical across all three charts - the first `fillRect` of each is
  the band, the next two are label backgrounds, so take every third one
- `Write session` is independent of the selection while `Write syscall` shrinks
  when the selection does

Give the stub a realistic pixel scale. A stub where 1 s maps to 10 px makes every
selection look narrow and will produce false failures on width thresholds.

## 6. HTTP surface

With `collect --serve` running:

| Check | Expectation |
| --- | --- |
| `POST /api/session/start` twice | second returns `409` |
| `POST` with `hz` above the maximum | `400`, message names the limit |
| `GET /api/session/status` mid-recording | `ticks_done` climbing, `remaining_secs` falling |
| `POST /api/session/stop` mid-recording | file still written, `meta.aborted` true, ticks kept |
| `GET /api/session/file/<name>` | inline JSON |
| `GET /api/session/file/<name>?download=1` | `Content-Disposition: attachment` |
| `GET /api/session/export/<name>` | self-contained HTML, no `src="/static` |
| `GET /api/export/json` | attachment, parses, `metadata` + `data` |
| `../../etc/passwd` as a name | `404` |

Then check the panel behaviour that broke twice: opening the record panel on a
fresh page must **not** download anything, because the server keeps
`finished_file` set until the next recording starts. Only a recording started from
that page may be acted on.

## 7. Cleaning up

Kill any server you started and remove the temporary recordings, writers and
harnesses. `pgrep -af 'target/debug/system_info_collector'` should come back empty.

Do not commit `sessions/` - recordings contain full command lines and open file
paths, which routinely include tokens and passwords.

## Reporting

State what you measured, not that it passed. A number next to the fact that
produced it is reviewable; "verified" is not. When a check fails, separate a broken
expectation in the check from a broken behaviour in the code - in this area they
have been about equally likely.
