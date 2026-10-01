#!/usr/bin/env python3
"""Measure sd's CLI parity with br from each tool's --help (no store access).

Prints verb coverage and, per shared verb (including `dep`/`comments`
subcommands), the long flags br has that sd lacks and vice versa.
Usage: scripts/parity-diff.py [--br br] [--sd sd] [--json]
Re-run unchanged after each change: the before/after pair is the metric.
"""
import argparse
import json
import re
import subprocess

COMMANDS = re.compile(r"^  ([a-z][a-z0-9-]*)\s{2,}", re.M)
FLAG = re.compile(r"^\s+(?:-[A-Za-z], )?(--[a-z][a-z0-9-]*)", re.M)
ALIAS = re.compile(r"\[alias(?:es)?: ([^\]]+)\]")


def help_text(tool, *path):
    out = subprocess.run([tool, *path, "--help"], capture_output=True, text=True, timeout=30)
    return out.stdout + out.stderr


# sd verbs whose capability lives elsewhere in the stack: they print a pointer
# and exit 21. They are reported separately and never counted as implemented.
MAPPED = {"query", "upgrade", "gate", "scheduler", "audit", "robot-docs"}


def verbs(tool, *path):
    text = help_text(tool, *path)
    if "Commands:" not in text:
        return set()
    block = text.split("Commands:", 1)[1].split("\n\n", 1)[0]
    found = set(COMMANDS.findall("\n" + block))
    for group in ALIAS.findall(block):  # visible aliases are real verbs too
        found |= {a.strip() for a in group.split(",") if a.strip()}
    return found - {"help"}


def flags(tool, *path):
    text = help_text(tool, *path)
    found = set(FLAG.findall(text))
    for group in ALIAS.findall(text):
        found |= {a.strip() for a in group.split(",") if a.strip().startswith("--")}
    return found - {"--help", "--version"}


# br flags with no referent in sd: its SQLite+JSONL storage engine (seeds stores facts in
# quipu), and output features sd does not have (TOON, colour, truncation, logging); each is listed with its reason and counted separately, never silently dropped.
NOT_APPLICABLE = {
    "--db": "SQLite path; seeds uses --store/--quipu",
    "--no-db": "JSONL-only mode", "--no-daemon": "br daemon",
    "--no-auto-flush": "JSONL flush", "--no-auto-import": "JSONL import",
    "--allow-stale": "JSONL staleness check", "--lock-timeout": "SQLite busy timeout",
    "--allow-external-jsonl": "JSONL path", "--flush-only": "JSONL export",
    "--import-only": "JSONL import", "--force-db": "SQLite vs JSONL",
    "--force-jsonl": "SQLite vs JSONL", "--rebuild": "SQLite rebuild",
    "--export-parallelism": "JSONL export", "--witness": "JSONL witness",
    "--witness-chunk-lines": "JSONL witness", "--witness-parallelism": "JSONL witness",
    "--migrate-source-repo-path": "br store migration", "--rename-prefix": "br id prefix migration",
    "--stats": "token-savings stats for br's TOON output, which sd does not produce",
    "--no-color": "sd emits no ANSI colour, so there is nothing to disable",
    "--wrap": "sd never truncates text output, so lines are already whole",
    "--no-wrap": "sd never truncates or wraps text output",
    "--verbose": "sd writes no log output for a level to raise",
    "--hard": "prunes tombstones from br's JSONL; sd keeps tombstones in quipu history by design",
    # br-engine machinery (aegis-w3k75d.13): each one operates on br's SQLite DB, its JSONL
    # export or its base snapshot, which a quipu ledger does not have. Each name is used by
    # exactly one br verb (measured with br's --help on every verb), so listing it here hides
    # nothing elsewhere.
    "--merge": "br sync: three-way DB/JSONL/base merge; sd sync is always a three-way merge with its remote, so there is no mode to select",
    "--reconcile": "br sync: JSONL -> SQLite reconcile; sd has no derived DB",
    "--reconcile-additive": "br sync: JSONL -> SQLite reconcile plan; sd has no derived DB",
    "--expect-plan-sha256": "br sync: token for an --apply of a reconcile/migration plan, which sd does not have",
    "--resolve-source-id": "br sync: resolves a JSONL-vs-DB scalar-row conflict",
    "--manifest": "br sync: manifest of a JSONL export",
    "--error-policy": "br sync: JSONL export serialization policy",
    "--skip-invalid-records": "br sync --import-only: drops invalid JSONL lines",
    "--repair": "br doctor: rebuilds the SQLite DB from JSONL; the quipu store is the ledger, nothing is derived",
    "--repair-indexes": "br doctor: SQLite REINDEX",
    "--allow-repeated-repair": "br doctor: another JSONL rebuild after a failed one",
    "--unsafe-auto-fix": "br doctor: opt-in to br's own repair fixers (only with --repair)",
    "--only": "br doctor: selects br repair fixers (only with --repair)",
    "--skip": "br doctor: skips br repair fixers (only with --repair)",
    "--projections": "br info: graph projection cache health; sd keeps no projection cache",
    "--source-repo": "br update: per-record repo name for JSONL cross-machine sync; a seed lives in a project graph",
    "--source-repo-path": "br update: absolute local path stamped on records (aegis-19lsrv: it leaks home paths); seeds have none",
    "--bypass-policy": "br close: bypasses .beads/policy.yaml gates; sd has no closure policy",
    "--bypass-reason": "br close: reason for --bypass-policy",
    "--backend": "br init: documented by br itself as ignored (always sqlite)",
    "--apply": "br sync: commits a reviewed reconcile/migration plan; sd sync has no such plan (--dry-run previews a sync, and a plain sync applies)",
    "--orphans": "br sync --import-only: how JSONL import treats deps on deleted issues; sd sync is a merge of two ledgers, never a JSONL import",
}

# The same, for a flag name br also uses on OTHER verbs with a meaning sd may
# share: n/a only on the verb named here, still counted everywhere else.
NOT_APPLICABLE_ON = {
    "sync": {
        "--force": "br sync: bypasses the Empty/Stale DB export guards of a JSONL export; sd has no export guard, and its one sync guard (removals) is lifted only by the explicitly named --allow-remote-deletes, deliberately not by --force",
    },
    "doctor": {
        "--fix": "br doctor: alias of --repair (rebuild the SQLite DB from JSONL); sd doctor never repairs, and nothing in a quipu ledger is derived",
        "--dry-run": "br doctor: previews --repair; without it br documents it as a no-op, and sd doctor is always read-only",
    },
    "count": {
        "--include-templates": "br count: includes template issues, but no br command can create one (every br --help swept) and the fleet has none (br count = br count --include-templates = 537, 2026-10-01); lead ruling on aegis-w3k75d.13",
    },
    "dep add": {
        "--metadata": "br dep add: stores JSON surfaced only in br's JSONL export (not show, not dep list); 0 of 1,419 live dependencies carry it, and sd dependencies are bare edges. Reopen if a producer starts writing it; lead ruling on aegis-w3k75d.13",
    },
}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--br", default="br")
    ap.add_argument("--sd", default="sd")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    b, s = verbs(a.br), verbs(a.sd)
    shared = sorted((b & s) - MAPPED)
    gb, gs = flags(a.br), flags(a.sd)
    per_verb = {}
    for verb in shared:
        paths = [(verb,)]
        subs = verbs(a.br, verb) & verbs(a.sd, verb)
        paths += [(verb, sub) for sub in sorted(subs)]
        for path in paths:
            fb, fs = flags(a.br, *path) - gb, flags(a.sd, *path) - gs
            na = fb & (set(NOT_APPLICABLE) | set(NOT_APPLICABLE_ON.get(" ".join(path), {})))
            fb -= na
            # A br per-verb flag is present in sd if sd has it on the verb OR as a
            # global flag (br declares --robot per verb; sd declares it once).
            have = fs | gs
            per_verb[" ".join(path)] = {"br_only": sorted(fb - have), "sd_only": sorted(fs - fb),
                                        "not_applicable": sorted(na),
                                        "shared": len(fb & have), "br_total": len(fb)}
    gna = gb & set(NOT_APPLICABLE)
    mapped = sorted((b & s) & MAPPED)
    report = {"br_verbs": len(b), "sd_verbs": len(s), "covered": len(b & s) - len(mapped),
              "mapped": mapped,
              "br_only_verbs": sorted(b - s), "sd_only_verbs": sorted(s - b),
              "global_flags": {"br_only": sorted(gb - gs - gna), "sd_only": sorted(gs - gb),
                               "not_applicable": sorted(gna), "shared": sorted(gb & gs)},
              "per_verb": per_verb,
              "flag_parity": {"shared": sum(v["shared"] for v in per_verb.values()),
                              "br_total": sum(v["br_total"] for v in per_verb.values())}}
    if a.json:
        print(json.dumps(report, indent=2, sort_keys=True))
        return
    print(f"verbs covered: {report['covered']}/{report['br_verbs']} implemented "
          f"(+{len(report['mapped'])} mapped to another tool; sd has {report['sd_verbs']})")
    print("mapped (pointer, exit 21):", " ".join(report["mapped"]) or "-")
    fp = report["flag_parity"]
    print(f"flag parity on shared verbs: {fp['shared']}/{fp['br_total']} br flags present in sd")
    print("br-only verbs:", " ".join(report["br_only_verbs"]))
    print("sd-only verbs:", " ".join(report["sd_only_verbs"]))
    print("global flags br-only:", " ".join(report["global_flags"]["br_only"]))
    print("global flags sd-only:", " ".join(report["global_flags"]["sd_only"]))
    print("global flags n/a:", " ".join(report["global_flags"]["not_applicable"]))
    for verb, row in per_verb.items():
        print(f"\n{verb}: {row['shared']}/{row['br_total']}")
        print("  br-only:", " ".join(row["br_only"]) or "-")
        print("  sd-only:", " ".join(row["sd_only"]) or "-")
        if row["not_applicable"]:
            print("  n/a:", " ".join(row["not_applicable"]))


if __name__ == "__main__":
    main()
