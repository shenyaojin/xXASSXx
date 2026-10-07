#!/usr/bin/env python3
"""Independently verify the synthetic protocol check; never invokes a model or edits a database."""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path


def check(root):
    def read(name):
        return json.loads((root / name).read_text())
    report = read("report.json")
    assert report["status"] == "passed" and report["mode"] == "two_host_technical"
    case = read("case.json")
    a, b = read("a-workflow.json"), read("b-workflow.json")
    rules = json.loads(case["a"]["text"])
    rows = list(csv.DictReader(io.StringIO(case["b"]["text"])))
    expected = sum(int(r["raw"]) * rules["calibration_by_batch"][r["batch"]] for r in rows) + rules["local_offset"]
    actual = json.loads(a["workflow"]["result"]["body"])["final_total"]
    assert actual == expected, (actual, expected)
    for member, state in [("a", a), ("b", b)]:
        raw = case[member]["text"].encode()
        digest = hashlib.sha256(raw).hexdigest()
        assert state["grants"][0]["sha256"] == digest
        assert any(s["sha256"] == digest and s["member"] == member for s in a["workflow"]["result"]["sources"])
        assert any(r["offset"] == 0 and r["bytes"] == len(raw) for r in state["file_reads"])
        assert state["workflow"]["state"] == "completed"
    result = dict(passed=True, expected=expected, actual=actual,
                  method="Independently parse original synthetic CSV and JSON; recompute and verify both SHA-256 references against final result",
                  both_granted_source_hashes_match=True)
    (root / "independent-check.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    print(json.dumps(check(args.output)))
