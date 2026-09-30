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
            na = fb & set(NOT_APPLICABLE)
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
