#!/usr/bin/env python3
"""Offline acceptance of the synthetic QC case. No model, SSH, or DB writes.

--materials checks the prepared pack. An output directory checks a REAL run.
The expected answer is recomputed from input bytes, never from agent output.
"""
import argparse
import csv
from decimal import Decimal, ROUND_HALF_UP
import hashlib
import io
import json
from pathlib import Path

PACK = Path(__file__).resolve().parents[1] / "examples" / "phase3-qc"


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


def require(ok, message):
    if not ok:
        raise ValueError(message)


def expected(case):
    rules = json.loads(case["a"]["text"])
    limits = rules["acceptance"]
    quantum = Decimal(1).scaleb(-rules["reporting"]["decimal_places"])

    def reported(value):
        return float(value.quantize(quantum, rounding=ROUND_HALF_UP))

    samples = []
    for row in csv.DictReader(io.StringIO(case["b"]["text"])):
        calibration = rules["calibration_by_batch"][row["batch"]]
        corrected = [
            (Decimal(row[f"raw_{i}"]) - Decimal(str(calibration["blank"])))
            * Decimal(str(calibration["factor"]))
            for i in (1, 2, 3)
        ]
        mean = sum(corrected) / 3
        spread = max(corrected) - min(corrected)
        reasons = []
        if mean < Decimal(str(limits["mean_min"])):
            reasons.append("mean_below_min")
        if mean > Decimal(str(limits["mean_max"])):
            reasons.append("mean_above_max")
        if spread > Decimal(str(limits["max_spread"])):
            reasons.append("spread_exceeded")
        samples.append(dict(
            sample_id=row["sample_id"], batch=row["batch"],
            corrected=[reported(v) for v in corrected], mean=reported(mean),
            spread=reported(spread), status="fail" if reasons else "pass", reasons=reasons,
        ))
    samples.sort(key=lambda s: s["sample_id"])
    retest = [s["sample_id"] for s in samples if s["status"] == "fail"]
    return dict(
        case_id=rules["case_id"], protocol_id=rules["protocol_id"], samples=samples,
        summary=dict(sample_count=len(samples), passed=len(samples)-len(retest),
                     failed=len(retest), retest_sample_ids=retest),
    )


def materials():
    case = read(PACK / "case.json")
    for member, relative in [("a", "a/protocol.json"), ("b", "b/measurements.csv")]:
        entry = case[member]
        require(set(entry) == {"file", "text", "goal"}, "invalid manifest fields")
        require(entry["file"] == Path(relative).name, "incorrect material label")
        require(entry["text"].encode() == (PACK / relative).read_bytes(), "manifest/material drift")
        require(len(entry["text"].encode()) <= 8192, "material exceeds runner limit")
    answer = expected(case)
    require(answer == read(PACK / "acceptance/expected.json"), "expected answer drift")
    return case, answer


def compare(actual, answer, label):
    for key in ("case_id", "protocol_id", "summary"):
        require(actual.get(key) == answer[key], f"{label}: incorrect {key}")
    rows = actual.get("samples")
    require(isinstance(rows, list), f"{label}: missing samples")
    rows = sorted(rows, key=lambda row: row["sample_id"])
    require(len(rows) == len(answer["samples"]), f"{label}: wrong sample count")
    for row, wanted in zip(rows, answer["samples"]):
        for key, value in wanted.items():
            require(row.get(key) == value, f"{label}: {wanted['sample_id']}.{key} differs")


def verify(root):
    case, answer = materials()
    require(read(root / "case.json") == case, "run used different materials/goals")
    report = read(root / "report.json")
    require(report["status"] == "passed" and report["mode"] == "reviewed_case", "real run not passed")
    require(report["real_codex"] is True and report["real_deepseek"] is True, "real models required")
    for key in ("a_real_tools_and_persisted_result", "b_real_tools_and_persisted_result",
                "automatic_clarification_and_explicit_session_resume", "idle_no_model_calls"):
        require(report["checks"].get(key) is True, f"missing runner check: {key}")
    states = {m: read(root / f"{m}-workflow.json") for m in ("a", "b")}
    sources = []
    for member, state in states.items():
        require(state["workflow"]["state"] == "completed", f"{member}: incomplete")
        require(len(state["grants"]) == 1, f"{member}: unexpected grants")
        grant = state["grants"][0]
        raw = case[member]["text"].encode()
        digest = hashlib.sha256(raw).hexdigest()
        require(grant["sha256"] == digest and grant["bytes"] == len(raw), f"{member}: wrong input hash")
        source = {key: grant[key] for key in ("member", "object_id", "version_id", "path", "sha256")}
        require(source["member"] == member, "wrong source member")
        sources.append(source)
        covered = 0
        for receipt in sorted(state["file_reads"], key=lambda r: r["offset"]):
            require(all(receipt[k] == source[k] for k in ("object_id", "version_id", "path", "sha256")), "wrong read source")
            require(0 <= receipt["offset"] <= covered, "gap in actual file reads")
            covered = max(covered, receipt["offset"] + receipt["bytes"])
        require(covered == len(raw), f"{member}: whole material was not read")
        runs = state["runs"]
        require(len(runs) >= (3 if member == "a" else 2), "missing collaboration turns")
        require(bool(runs[0]["session_id"]), "missing session ID")
        for index, run in enumerate(runs):
            require(run["state"] == "succeeded" and run["exit_code"] == 0, "unsuccessful run")
            require(run["mcp_initialized"] and run["reads"] > 0 and run["submissions"] > 0
                    and run["turn_completed"], "unverified MCP turn")
            require("simulat" not in run["codex_version"] and "mock" not in run["codex_version"], "simulated Codex")
            require(run["session_id"] == runs[0]["session_id"], "session changed")
            if index:
                require(run["resumed_session_id"] == runs[0]["session_id"], "session not resumed")
        execution = read(root / f"{member}-execution.json")
        require(execution["runs"] == runs, "execution snapshot mismatch")
        event_text = json.dumps(execution["events"])
        for token in ("mcp_tool_call", "read_task_file", "submit_collaboration_turn", "turn.completed"):
            require(token in event_text, f"missing raw event: {token}")
        result = state["workflow"]["result"]
        compare(json.loads(result["body"]), answer, member)
        require(read(root / f"{member}-result.json")["result"] == result, "saved artifact mismatch")
        model_runs = state["model_runs"]
        require(model_runs and all(r["state"] == "succeeded" for r in model_runs), "coordinator failed")
        tools = {t["name"] for r in model_runs for t in r["trace"] if t["result"]["ok"]}
        require({"inspect_workflow", "dispatch_codex"} <= tools, "missing butler tools")
    for state in states.values():
        require(all(s in state["workflow"]["result"]["sources"] for s in sources), "missing exact source citation")
    events = [m["workflow"]["event"] for m in states["a"]["messages"]]
    cursor = iter(events)
    require(all(any(e == need for e in cursor) for need in ("request", "clarification", "answer", "result")), "missing round trip")
    for host in report["hosts"].values():
        require(report["cleanup"][host["alias"]]["alive"] is False, "test service left running")
    require(report["cleanup"]["mailbox"] == "stopped", "test mailbox left running")
    verdict = dict(passed=True, case_id=answer["case_id"], summary=answer["summary"],
                   checked="Both results recomputed from input bytes; file reads, exact citations, original session resume, raw events and cleanup verified")
    (root / "independent-check.json").write_text(json.dumps(verdict, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return verdict


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, nargs="?")
    parser.add_argument("--materials", action="store_true", help="Check materials only; does not prove a real run")
    args = parser.parse_args()
    if args.materials:
        if args.output:
            parser.error("choose output OR --materials")
        _, answer = materials()
        print(json.dumps(dict(materials_valid=True, real_run_checked=False, expected_summary=answer["summary"])))
    elif args.output:
        print(json.dumps(verify(args.output), ensure_ascii=False))
    else:
        parser.error("provide a real-run output directory, or --materials")
