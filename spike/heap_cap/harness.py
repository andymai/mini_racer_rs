"""Runs heap cap cases, one child process each, at most JOBS at once, and
writes one JSON line per attempted hit plus the process-level records."""

import json
import os
import re
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from threading import Lock, Semaphore

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = "/var/home/andy/Git/mini_racer_rs/.worktrees/heap-cap"
CASE = os.path.join(HERE, "case.rb")
ENV_SH = "/tmp/claude-1000/-var-home-andy/9840488a-f188-474e-bb47-4c700a1bb0d4/scratchpad/env.sh"

POLICIES = {
    "f16": {"MINI_RACER_RS_HEAP_HEADROOM": "16m", "MINI_RACER_RS_HEAP_EXTENSIONS": "1"},
    "f64": {"MINI_RACER_RS_HEAP_HEADROOM": "64m", "MINI_RACER_RS_HEAP_EXTENSIONS": "1"},
    "p25": {"MINI_RACER_RS_HEAP_HEADROOM": "25%", "MINI_RACER_RS_HEAP_EXTENSIONS": "1"},
    "e50x4": {"MINI_RACER_RS_HEAP_HEADROOM": "50%", "MINI_RACER_RS_HEAP_EXTENSIONS": "4"},
}

FATAL = re.compile(r"^(# Fatal .*|.*FATAL ERROR.*|.*Check failed.*|.*panicked.*|.*Segmentation fault.*)$")
CALLBACK = re.compile(r"HEAPCAP callback calls=(\d+) episode=(\d+) current=(\d+) initial=(\d+) next=(\d+) refused=(\w+) collecting=(\w+) thread=(\d+) runner=(\d+)")
AFTER = re.compile(r"HEAPCAP after episode=(\d+) used_before=(\d+) used_after=(\d+) total_after=(\d+) limit_after=(\d+) initial=(\d+) under=(\w+) restored=(\w+) gc_ms=([\d.]+)")
MARK = re.compile(r"HARNESS hit=(\d+) (begin|end)")

write_lock = Lock()
heavy = Semaphore(1)


def run_case(case, out):
    env = dict(os.environ)
    env.update(case["env"])
    env["MINI_RACER_RS_HEAP_LOG"] = "1"
    timeout = 30 + 60 * case["repeat"]
    cmd = [
        "bash", "-c",
        f". {ENV_SH}; cd {REPO}; ulimit -c 0; exec bundle exec ruby -Ilib "
        + " ".join(case.get("ruby_args", []))
        + f" {case.get('script', CASE)} {case['workload']} {case['cap']} {case['repeat']}",
    ]
    started = time.monotonic()
    guard = heavy if case.get("heavy") else None
    if guard:
        guard.acquire()
    try:
        proc = subprocess.Popen(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                start_new_session=True)
        try:
            stdout, stderr = proc.communicate(timeout=timeout)
            hung = False
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, 9)
            stdout, stderr = proc.communicate()
            hung = True
    finally:
        if guard:
            guard.release()
    wall = time.monotonic() - started
    status = proc.returncode
    hits = {}
    for line in stdout.splitlines():
        try:
            rec = json.loads(line)
        except ValueError:
            continue
        if "hit" in rec:
            hits[rec["hit"]] = rec
        elif "later" in rec:
            later = rec["later"]
        elif rec.get("done"):
            pass
    later = None
    for line in stdout.splitlines():
        if line.startswith('{"later"'):
            later = json.loads(line)["later"]
    done = '{"done":true}' in stdout
    # Pair stderr lines with hits.
    current = None
    callbacks = {}
    afters = {}
    began = set()
    fatal = []
    for line in stderr.splitlines():
        m = MARK.search(line)
        if m:
            current = int(m.group(1))
            if m.group(2) == "begin":
                began.add(current)
            continue
        m = CALLBACK.search(line)
        if m:
            callbacks.setdefault(current, []).append({
                "current": int(m.group(3)), "initial": int(m.group(4)), "next": int(m.group(5)),
                "refused": m.group(6) == "true", "collecting": m.group(7) == "true",
                "same_thread": m.group(8) == m.group(9),
            })
            continue
        m = AFTER.search(line)
        if m:
            afters[current] = {
                "used_before": int(m.group(2)), "used_after_gc": int(m.group(3)),
                "under": m.group(7) == "true", "restored": m.group(8) == "true", "gc_ms": float(m.group(9)),
            }
            continue
        if FATAL.match(line.strip()):
            fatal.append(line.strip())
    records = []
    for hit in sorted(began):
        rec = {k: v for k, v in case.items() if k not in ("env",)}
        rec["policy_env"] = case["env"]
        rec["hit"] = hit
        cbs = callbacks.get(hit, [])
        rec["callbacks"] = len(cbs)
        rec["refusals"] = sum(1 for c in cbs if c["refused"])
        rec["off_thread"] = sum(1 for c in cbs if not c["same_thread"])
        rec["collecting_calls"] = sum(1 for c in cbs if c["collecting"])
        rec["peak_limit"] = max([c["next"] for c in cbs], default=None)
        rec["after"] = afters.get(hit)
        if hit in hits:
            rec.update({k: v for k, v in hits[hit].items() if k != "hit"})
        elif hung:
            rec["outcome"] = "hang"
        else:
            rec["outcome"] = "abort"
            rec["exit_status"] = status
            rec["abort_message"] = " | ".join(fatal) or stderr.strip().splitlines()[-1:] or None
        records.append(rec)
    proc_rec = {
        "process": True, "workload": case["workload"], "cap": case["cap"], "policy": case["policy"],
        "tag": case.get("tag"), "status": status, "hung": hung, "done": done, "later": later, "wall": round(wall, 2),
        "fatal": fatal[:3],
    }
    if not done and not records:
        proc_rec["stderr_tail"] = stderr.strip().splitlines()[-5:]
    with write_lock:
        for rec in records:
            out.write(json.dumps(rec) + "\n")
        out.write(json.dumps(proc_rec) + "\n")
        out.flush()
    print(f"{case.get('tag') or ''} {case['workload']} {case['cap']} {case['policy']}: "
          + ",".join(r["outcome"] for r in records) + f" status={status} wall={wall:.1f}", flush=True)


def matrix():
    cases = []
    workloads = ["objects", "concat", "json", "literal", "literal_single", "map", "recursion"]
    for workload in workloads:
        for cap in (64, 256, 512):
            for policy, penv in POLICIES.items():
                env = {"MINI_RACER_RS_HEAP_LIMIT": f"{cap}m", **penv}
                is_heavy = workload == "literal_single" and cap >= 256
                for proc in range(4 if not is_heavy else 2):
                    cases.append({"workload": workload, "cap": cap, "policy": policy, "env": env,
                                  "repeat": 5 if not is_heavy else 3, "heavy": is_heavy, "tag": "matrix"})
    for cap in (64, 256, 512):
        cases.append({"workload": "arraybuffer", "cap": cap, "policy": "f16",
                      "env": {"MINI_RACER_RS_HEAP_LIMIT": f"{cap}m", **POLICIES["f16"]}, "repeat": 3, "tag": "matrix"})
    return cases


def main():
    which = sys.argv[1]
    out_path = sys.argv[2]
    jobs = int(os.environ.get("JOBS", "4"))
    if which == "matrix":
        cases = matrix()
    else:
        cases = json.loads(open(which).read())
    # Heavy cases first, so they overlap with light ones.
    cases.sort(key=lambda c: (not c.get("heavy"), -c["cap"]))
    with open(out_path, "a") as out, ThreadPoolExecutor(max_workers=jobs) as pool:
        list(pool.map(lambda c: run_case(c, out), cases))


if __name__ == "__main__":
    main()
