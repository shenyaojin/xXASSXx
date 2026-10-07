#!/usr/bin/env python3
"""Independent offline verification for the public HTTPS QC installation test."""
import argparse
import hashlib
import json
from pathlib import Path
from urllib.parse import urlsplit
import check_phase3_qc as qc
from ssh_workflow import validate_evidence


def check(root):
    case, answer = qc.materials()
    assert qc.read(root / "case.json") == case
    report = qc.read(root / "report.json")
    assert report["status"] == "passed"
    assert report["real_codex"] is True and report["real_deepseek"] is True
    assert urlsplit(report["mailbox_url"]).scheme == "https"
    assert report["checks"]["no_ssh_tunnels"] is True and report["checks"]["idle_no_extra_calls"] is True
    states = {}
    sources = []
    for m in ("a", "b"):
        assert report["checks"][m + "_doctor"]["mailbox"]["reachable_and_identity_matches"] is True
        state = qc.read(root / (m + "-workflow.json"))
        execution = qc.read(root / (m + "-execution.json"))
        validate_evidence(state, execution)
        assert state["runs"] == execution["runs"]
        assert len(state["runs"]) >= (3 if m == "a" else 2)
        for run in state["runs"]:
            assert "mock" not in run["codex_version"] and "simulat" not in run["codex_version"]
        body = json.loads(state["workflow"]["result"]["body"])
        qc.compare(body, answer, m)
        assert qc.read(root / (m + "-result.json"))["result"] == state["workflow"]["result"]
        assert len(state["grants"]) == 1
        source = {k: state["grants"][0][k] for k in ("member", "object_id", "version_id", "path", "sha256")}
        raw = case[m]["text"].encode()
        assert source["member"] == m and source["sha256"] == hashlib.sha256(raw).hexdigest()
        assert state["grants"][0]["bytes"] == len(raw)
        assert all(r["state"] == "succeeded" for r in state["model_runs"])
        covered = 0
        for receipt in sorted(state["file_reads"], key=lambda r: r["offset"]):
            assert all(receipt[k] == source[k] for k in ("object_id", "version_id", "path", "sha256"))
            assert 0 <= receipt["offset"] <= covered
            covered = max(covered, receipt["offset"] + receipt["bytes"])
        assert covered == len(raw)
        for run in state["runs"]:
            events = execution["events"][run["id"]]
            assert any(e["type"] == "turn.completed" for e in events)
            chunks = []
            for event in events:
                item = event.get("item", {})
                if event["type"] != "item.completed" or item.get("tool") != "read_task_file":
                    continue
                assert item["status"] == "completed" and item["error"] is None
                value = item["result"]["structured_content"]
                assert value["source"] == source
                chunk = value["text"].encode()
                assert len(chunk) == value["bytes"]
                assert chunk == raw[value["offset"]:value["next_offset"]]
                chunks.append((value["offset"], value["next_offset"]))
            covered = 0
            for start, end in sorted(chunks):
                assert 0 <= start <= covered and start < end <= len(raw)
                covered = max(covered, end)
            assert covered == len(raw), "raw MCP file content not verified for every turn"
        assert report["cleanup"][m]["alive"] is False
        sources.append(source)
        states[m] = state
    for state in states.values():
        assert all(s in state["workflow"]["result"]["sources"] for s in sources)
    events = [m["workflow"]["event"] for m in states["a"]["messages"]]
    cursor = iter(events)
    assert all(any(e == required for e in cursor) for required in ("request", "clarification", "answer", "result"))
    assert report["cleanup"]["team_mailbox"] == "kept_running_as_requested"
    verdict = dict(passed=True, summary=answer["summary"], transport=report["transport"],
                   both_results_recomputed=True, sources_and_reads_verified=True,
                   every_turn_raw_file_content_verified=True,
                   real_execution_and_session_resume_verified=True, test_butlers_stopped=True)
    (root / "independent-check.json").write_text(json.dumps(verdict, indent=2) + "\n")
    return verdict


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    print(json.dumps(check(parser.parse_args().output)))
