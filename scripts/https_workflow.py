#!/usr/bin/env python3
"""REAL two-host QC over the persistent HTTPS mailbox, with no SSH tunnels.

SSH is only used for bounded deployment/status commands. The Rust services own
all peer delivery and model turns and survive these SSH commands disconnecting.
Invitations are private local files and are never written to report/log output.
"""
import argparse
import json
from pathlib import Path
import time
import uuid
from urllib.parse import urlsplit

from ssh_workflow import Host, save, validate_evidence
import check_phase3_qc as qc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary-a", required=True)
    parser.add_argument("--binary-b", required=True)
    parser.add_argument("--invite-a", type=Path, required=True)
    parser.add_argument("--invite-b", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    case, _ = qc.materials()
    invitations = {m: json.loads(p.read_text()) for m, p in [("a", args.invite_a), ("b", args.invite_b)]}
    url = invitations["a"]["mailbox_url"]
    assert urlsplit(url).scheme == "https"
    assert invitations["b"]["mailbox_url"] == url
    for m in ("a", "b"):
        assert invitations[m]["member_id"] == m
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    name = "phase4-https-" + str(uuid.uuid4())
    report = dict(status="running", run_id=name, real_codex=True, real_deepseek=True,
                  transport="direct HTTPS; no SSH forwarding or controller relay", mailbox_url=url,
                  checks={}, hosts={}, cleanup={})
    hosts = {}
    workflows = {}
    started = []
    error = None
    save(root / "case.json", case)
    try:
        for member, alias, binary, codex in [
            ("a", "trader", args.binary_a, "/home/ubuntu/.local/bin/codex"),
            ("b", "lakota", args.binary_b, "/rcp/rcp42/home/shenyaojin/.nvm/versions/node/v22.18.0/bin/codex"),
        ]:
            # -S none disables multiplexing; no master, -L, -R, or tunnel is created.
            host = Host(alias, binary, name, {"TOKIO_WORKER_THREADS": "2"}, "none")
            hosts[member] = host
            host.info = host.rpc("setup")
            report["hosts"][member] = dict(alias=alias, **host.info)
            host.write("invitation.json", json.dumps(invitations[member]))
            directory = host.info["root"] + "/client"
            home = host.info["root"].split("/xxassxx-tests/")[0]
            host.call("client", "--directory", directory, "init", "--invite", host.info["root"] + "/invitation.json",
                      "--provider", "deepseek", "--model", "deepseek-flash", "--model-secrets",
                      home + "/.config/xxassxx/secrets.env", "--codex", codex)
            # doctor is read-only here; the actual task proves model execution.
            report["checks"][member + "_doctor"] = host.call("client", "--directory", directory, "doctor")
            assert report["checks"][member + "_doctor"]["ready"]
            material = case[member]
            obj = host.call("object", "create", "--title", material["file"], "--kind", "dataset", db="client/member.sqlite3")
            version = host.call("object", "publish", obj["id"], "--expected", "none", "--request-id", "initial",
                                "--text", material["text"], db="client/member.sqlite3")
            workflows[member] = dict(grant=version["id"] + ":record.txt", directory=directory)
        assert hosts["a"].info["binary_sha256"] == hosts["b"].info["binary_sha256"]
        b = hosts["b"].call("collaboration", "prepare", "--peer", "a", "--goal", case["b"]["goal"],
                            "--grant", workflows["b"]["grant"], db="client/member.sqlite3")
        a = hosts["a"].call("collaboration", "start", "--peer", "b", "--remote-workflow", b["id"],
                            "--goal", case["a"]["goal"], "--grant", workflows["a"]["grant"], db="client/member.sqlite3")
        workflows["a"]["id"], workflows["b"]["id"] = a["id"], b["id"]
        report["workflows"] = {m: w["id"] for m, w in workflows.items()}
        save(root / "report.json", report)
        for member, host in hosts.items():
            host.call("client", "--directory", workflows[member]["directory"], "start")
            started.append(member)
        # No deployment or observation connection exists during this interval.
        time.sleep(12)
        report["checks"]["no_ssh_tunnels"] = True
        report["checks"]["no_control_calls_seconds"] = 12
        deadline = time.monotonic() + 1200
        last = None
        while time.monotonic() < deadline:
            states = {m: h.call("collaboration", "show", workflows[m]["id"], db="client/member.sqlite3")
                      for m, h in hosts.items()}
            current = {m: s["workflow"]["state"] for m, s in states.items()}
            for m, state in states.items():
                save(root / (m + "-workflow.json"), state)
            if current != last:
                print(json.dumps(current), flush=True)
                last = current
            # A completed MCP submission can precede Codex's turn.completed and
            # process exit. Wait for the execution receipts before taking evidence.
            completed = all(s == "completed" for s in current.values())
            if completed and all(s["runs"] and all(r["state"] == "succeeded" for r in s["runs"]) for s in states.values()):
                break
            if any(s in ("failed", "needs_attention", "limit_reached", "timed_out", "stopped") for s in current.values()):
                raise RuntimeError("Workflow stopped: " + json.dumps(current))
            time.sleep(3)
        else:
            raise TimeoutError("Public workflow deadline")
        for m, host in hosts.items():
            state = states[m]
            execution = host.call("show", state["workflow"]["task_id"], "--events", db="client/member.sqlite3")
            save(root / (m + "-execution.json"), execution)
            validate_evidence(state, execution)
            artifact = host.rpc("read", path="client/member.sqlite3.results/" + workflows[m]["id"] + ".json")
            (root / (m + "-result.json")).write_text(artifact)
        time.sleep(3)
        for m, host in hosts.items():
            after = host.call("collaboration", "show", workflows[m]["id"], db="client/member.sqlite3")
            assert after["runs"] == states[m]["runs"] and after["model_runs"] == states[m]["model_runs"]
        report["checks"]["idle_no_extra_calls"] = True
        report["status"] = "passed"
    except Exception as exc:
        error = exc
        report["status"], report["error"] = "failed", str(exc)
    finally:
        for m in started:
            host = hosts[m]
            try:
                if error:
                    host.call("collaboration", "stop", workflows[m]["id"], db="client/member.sqlite3")
                stopped = host.call("client", "--directory", workflows[m]["directory"], "stop")
                report["cleanup"][m] = stopped
                assert not stopped["alive"], "test butler still active"
            except Exception as exc:
                report["status"] = "failed"
                report["cleanup"][m] = {"error": str(exc)}
                error = error or exc
        report["cleanup"]["team_mailbox"] = "kept_running_as_requested"
        save(root / "report.json", report)
    print(str(root / "report.json"), flush=True)
    if error:
        raise SystemExit(str(error))


if __name__ == "__main__":
    main()
