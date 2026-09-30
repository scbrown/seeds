#!/usr/bin/env python3
"""Run one command sequence against a fresh br store and a fresh sd store and
compare the --json output SHAPE (keys and JSON types, not values).

Values differ by design (ids, timestamps); a key br emits that sd does not, or
a different JSON type at the same path, is a parity gap. Scratch stores only:
nothing touches a configured project. Usage: scripts/json-conformance.py [--json]
"""
import argparse
import json
import os
import subprocess
import tempfile

# (label, argv with {A}/{B} placeholders). Only verbs both tools claim.
STEPS = [
    ("create", ["create", "parity A", "-t", "task", "-p", "2", "-d", "desc"]),
    ("create2", ["create", "parity B", "-t", "bug", "-p", "1"]),
    ("show", ["show", "{A}"]),
    ("list", ["list"]),
    ("ready", ["ready"]),
    ("count", ["count"]),
    ("version", ["version"]),
    ("where", ["where"]),
    ("info", ["info"]),
    ("stale", ["stale", "--days", "0"]),
    ("stats", ["stats", "--by-type", "--by-priority", "--by-assignee", "--by-label"]),
    ("epic status", ["epic", "status"]),
    ("epic close-eligible --dry-run", ["epic", "close-eligible", "--dry-run"]),
    ("search", ["search", "parity"]),
    ("update", ["update", "{A}", "--status", "in_progress", "--assignee", "probe"]),
    ("dep add", ["dep", "add", "{B}", "{A}"]),
    ("dep list", ["dep", "list", "{B}"]),
    ("blocked", ["blocked"]),
    ("comments add", ["comments", "add", "{A}", "a comment"]),
    ("comments list", ["comments", "list", "{A}"]),
    ("label add", ["label", "add", "{A}", "{B}", "infra"]),
    ("label add -l", ["label", "add", "{A}", "-l", "infra"]),
    ("label list", ["label", "list", "{A}"]),
    ("label list-all", ["label", "list-all"]),
    ("label rename", ["label", "rename", "infra", "ops"]),
    ("label remove", ["label", "remove", "{B}", "-l", "ops"]),
    ("close", ["close", "{A}", "--reason", "done"]),
    ("reopen", ["reopen", "{A}", "-r", "not done"]),
    ("defer", ["defer", "{A}", "--until", "+1d"]),
    ("undefer", ["undefer", "{A}"]),
    ("search closed", ["search", "parity A"]),
    ("list all", ["list", "--all"]),
]


# br JSON fields that describe its own storage engine; no referent in a quipu ledger.
NOT_APPLICABLE = {"compaction_level": "br compaction", "original_size": "br compaction",
                  "source_repo": "JSONL multi-repo", "source_repo_path": "JSONL multi-repo",
                  "daemon_connected": "br daemon", "daemon_detail": "br daemon",
                  "daemon_fallback_reason": "br daemon", "jsonl_size": "JSONL file"}


def shape(value, path="$", out=None):
    out = {} if out is None else out
    kind = type(value).__name__ if value is not None else "null"
    out.setdefault(path, set()).add(kind)
    if isinstance(value, dict):
        for key, child in value.items():
            shape(child, f"{path}.{key}", out)
    elif isinstance(value, list):
        for child in value:
            shape(child, f"{path}[]", out)
    return out


def run(tool, cwd, argv, ids, env):
    argv = [a.format(**ids) for a in argv]
    p = subprocess.run([tool, "--json", *argv], cwd=cwd, capture_output=True, text=True,
                       timeout=60, env=env)
    try:
        return p.returncode, json.loads(p.stdout), p.stderr.strip()[:200]
    except ValueError:
        return p.returncode, None, (p.stderr or p.stdout).strip()[:200]


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--br", default="br")
    ap.add_argument("--sd", default="sd")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    root = tempfile.mkdtemp(prefix="sd-conformance-")
    env = {k: v for k, v in os.environ.items() if not k.startswith(("SEEDS_", "BEADS_"))}
    env["SEEDS_ACTOR"] = env["BR_ACTOR"] = "probe"
    dirs = {}
    for tool in ("br", "sd"):
        d = os.path.join(root, tool)
        os.makedirs(d)
        subprocess.run(["git", "init", "-q"], cwd=d, check=True)
        if tool == "br":
            subprocess.run([a.br, "init"], cwd=d, capture_output=True, check=True, env=env)
        dirs[tool] = d
    ids = {"br": {}, "sd": {}}
    rows = []
    for label, argv in STEPS:
        res = {}
        for tool, binary in (("br", a.br), ("sd", a.sd)):
            rc, data, err = run(binary, dirs[tool], argv, ids[tool], env)
            if label.startswith("create") and isinstance(data, dict) and "id" in data:
                ids[tool]["A" if label == "create" else "B"] = data["id"]
            res[tool] = (rc, data, err)
        (brc, bd, be), (src, sdd, se) = res["br"], res["sd"]
        row = {"step": label, "br_rc": brc, "sd_rc": src}
        if bd is None or sdd is None:
            row["error"] = {"br": None if bd is not None else be, "sd": None if sdd is not None else se}
        else:
            bs, ss = shape(bd), shape(sdd)
            na = {p for p in set(bs) - set(ss) if p.rsplit(".", 1)[-1] in NOT_APPLICABLE}
            row["not_applicable"] = sorted(na)
            row["br_only_paths"] = sorted(set(bs) - set(ss) - na)
            row["sd_only_paths"] = sorted(set(ss) - set(bs))
            row["type_mismatch"] = sorted(
                f"{p}: br={'/'.join(sorted(bs[p]))} sd={'/'.join(sorted(ss[p]))}"
                for p in set(bs) & set(ss)
                if bs[p] - {"null"} and ss[p] - {"null"} and not (bs[p] - {"null"}) & (ss[p] - {"null"}))
            row["conformant"] = not (row["br_only_paths"] or row["type_mismatch"]) and brc == src
        rows.append(row)
    summary = {"steps": len(rows), "conformant": sum(bool(r.get("conformant")) for r in rows),
               "rows": rows, "scratch": root}
    if a.json:
        print(json.dumps(summary, indent=2))
        return
    print(f"json conformance: {summary['conformant']}/{summary['steps']} steps (scratch {root})")
    for r in rows:
        mark = "ok " if r.get("conformant") else "GAP"
        print(f"\n[{mark}] {r['step']}  rc br={r['br_rc']} sd={r['sd_rc']}")
        if "error" in r:
            print("  unparsable:", r["error"])
            continue
        for key in ("br_only_paths", "type_mismatch", "sd_only_paths"):
            if r[key]:
                print(f"  {key}: {' '.join(r[key])}")


if __name__ == "__main__":
    main()
