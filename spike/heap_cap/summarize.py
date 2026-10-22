import json
import statistics
import sys
from collections import defaultdict, Counter

rows = []
procs = []
for path in sys.argv[1:]:
    for line in open(path):
        rec = json.loads(line)
        (procs if rec.get("process") else rows).append(rec)

groups = defaultdict(list)
for r in rows:
    groups[(r.get("tag"), r["workload"], r["cap"], r["policy"])].append(r)

def fmt_s(xs):
    if not xs:
        return "-"
    return f"{statistics.median(xs):.2f}/{max(xs):.2f}"

print("| tag | workload | cap MB | policy | attempts | cap hits | error | abort | hang | completed | other | later-hit errors | usable after | restored | s to error (median/max) | peak limit MB | abort message |")
print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
total = Counter()
for key in sorted(groups, key=lambda k: (k[0] or "", k[1], k[2], k[3])):
    rs = groups[key]
    c = Counter(r["outcome"] for r in rs)
    hits = sum(1 for r in rs if r["callbacks"] > 0)
    later = sum(1 for r in rs if r["outcome"] == "error" and r["hit"] > 0)
    usable = sum(1 for r in rs if r.get("usable") == "yes")
    restored = sum(1 for r in rs if (r.get("after") or {}).get("restored"))
    secs = [r["seconds"] for r in rs if r["outcome"] == "error"]
    peak = max((r["peak_limit"] or 0) for r in rs) >> 20
    msgs = sorted({(r.get("abort_message") or "").replace("# Fatal JavaScript out of memory: ", "") for r in rs if r["outcome"] == "abort"})
    others = sorted({(r.get("error") or "") + ": " + (r.get("message") or "")[:50] for r in rs if r["outcome"] == "other"})
    print(f"| {key[0]} | {key[1]} | {key[2]} | {key[3]} | {len(rs)} | {hits} | {c['error']} | {c['abort']} | {c['hang']} | {c['completed']} | {c['other']} | {later} | {usable} | {restored} | {fmt_s(secs)} | {peak} | {'; '.join(msgs + others)} |")
    total.update(c)
    total["hits"] += hits
    total["attempts"] += len(rs)

off = sum(r["off_thread"] for r in rows)
cbs = sum(r["callbacks"] for r in rows)
print()
print(f"totals: {dict(total)}; callbacks={cbs}, off-runner-thread callbacks={off}")
not_done = [p for p in procs if not p["done"]]
print(f"processes: {len(procs)}, ended without done: {len(not_done)}, hung: {sum(1 for p in procs if p['hung'])}")
laters = Counter(tuple(p["later"]) for p in procs if p.get("later"))
print("later-script outcomes per process:", dict(laters))
