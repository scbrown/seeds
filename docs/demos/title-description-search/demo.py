#!/usr/bin/env python3
"""Compare two sd binaries against one disposable local fixture (no network)."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

base = os.environ["BASE_SD"]
candidate = os.environ["CANDIDATE_SD"]
with tempfile.TemporaryDirectory(prefix="search-scope-demo-") as temporary:
    store = str(Path(temporary) / "fixture.db")
    environment = {key: value for key, value in os.environ.items() if not key.startswith(("SEEDS_", "QUIPU_"))}
    environment["HOME"] = temporary

    def run(binary, *arguments):
        location = [] if arguments[0] == "init" else ["--store", store]
        result = subprocess.run(
            [binary, *location, "--actor", "demo", "--json", *arguments],
            check=True, capture_output=True, text=True, cwd=temporary, env=environment,
        )
        return json.loads(result.stdout)

    run(base, "init", "--prefix", "demo")
    seed = run(base, "create", "Capture work", "--description", "read process pool")
    run(base, "comments", "add", seed["id"], "comment-only-zebra")
    print("Disposable local fixture: title='Capture work'; description='read process pool'; comment='comment-only-zebra'", flush=True)
    for label, binary, query, full, expected in [
        ("BASE", base, "Capture work", False, 1),
        ("BASE", base, "read process pool", False, 0),
        ("CANDIDATE", candidate, "read process pool", False, 1),
        ("CANDIDATE", candidate, "comment-only-zebra", False, 0),
        ("CANDIDATE", candidate, "comment-only-zebra", True, 1),
    ]:
        arguments = ["search", query, *(["--full"] if full else [])]
        print(f"$ {label} sd search {query!r}" + (" --full" if full else ""), flush=True)
        answer = run(binary, *arguments)
        print(json.dumps({"total": answer["total"], "scope": answer.get("search_scope", "full-field envelope"), "titles": [row["title"] for row in answer["issues"]]}, ensure_ascii=False), flush=True)
        assert answer["total"] == expected
    print("PASS: description-only miss -> hit; comment excluded by default and included with --full", flush=True)
