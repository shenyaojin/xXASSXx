#!/usr/bin/env python3
"""Offline-model demo, real HTTP + independent foreground butler processes.

Only the model and Codex are simulated. All business state goes through the CLI,
HTTP mailbox and real MCP; the script never edits SQLite directly.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import subprocess
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = Path(__file__).resolve().parents[1]

class MockModel(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if any(m["role"] == "tool" for m in body["messages"]):
            assistant = {"role": "assistant", "content": "The original request is delegated; execution is pending."}
            finish = "stop"
        else:
            assistant = {"role": "assistant", "content": None, "reasoning_content": "Mock coordination only.",
                         "tool_calls": [{"id": "delegate", "type": "function", "function": {"name": "delegate_codex", "arguments": "{}"}}]}
            finish = "tool_calls"
        output = json.dumps({"choices": [{"finish_reason": finish, "message": assistant}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(output)))
        self.end_headers()
        self.wfile.write(output)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=REPO / "target/debug/xxassxx")
    parser.add_argument("--output", type=Path, default=REPO / "smoke-output" / f"phase2-demo-{uuid.uuid4()}")
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env.update(TEAM_A_TOKEN=secrets.token_hex(24), TEAM_B_TOKEN=secrets.token_hex(24), XXASSXX_MOCK_MODE="success")
    children, logs = [], []
    model = ThreadingHTTPServer(("127.0.0.1", 0), MockModel)
    threading.Thread(target=model.serve_forever, daemon=True).start()
    report = {"status": "running", "model": "mock HTTP model", "codex": "mock process", "real_deepseek": "not_run"}
    (root / "report.json").write_text(json.dumps(report, indent=2))

    def call(member, *parts):
        result = subprocess.run([str(args.binary.resolve()), "--db", str(root / member / "tasks.sqlite3"), *parts],
                                env=env, text=True, capture_output=True, timeout=60)
        if result.returncode:
            raise RuntimeError(result.stderr)
        return json.loads(result.stdout)

    def start(member):
        log = open(root / member / "butler.log", "a")
        logs.append(log)
        child = subprocess.Popen([str(args.binary.resolve()), "--db", str(root / member / "tasks.sqlite3"),
                                  "butler", "run", "--poll-secs", "1"], env=env, stdout=log, stderr=log)
        children.append(child)
        return child

    def stop(child):
        if child.poll() is None:
            child.send_signal(signal.SIGINT)
            child.wait(timeout=15)

    def await_reply(request):
        until = time.monotonic() + 40
        while time.monotonic() < until:
            replies = [r for r in call("a", "message", "inbox") if r["message"]["reply_to"] == request]
            if replies:
                return replies[0]["message"]
            time.sleep(0.25)
        raise RuntimeError("reply did not arrive; inspect butler logs")

    try:
        relay_config = root / "mailbox.toml"
        shutil.copy(REPO / "examples/mailbox.toml", relay_config)
        relay = subprocess.Popen([str(args.binary.resolve()), "--db", str(root / "relay.sqlite3"), "mailbox", "serve",
                                  "--listen", "127.0.0.1:0", "--config", str(relay_config)], env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        children.append(relay)
        announced = relay.stdout.readline()
        if not announced:
            raise RuntimeError(relay.stderr.read())
        url = "http://" + json.loads(announced)["listening"]
        fake_codex = root / "mock-codex"
        shutil.copy(REPO / "tests/fixtures/codex.py", fake_codex)
        fake_codex.chmod(0o700)
        for member, peer in [("a", "b"), ("b", "a")]:
            (root / member / "work").mkdir(parents=True)
            call(member, "member", "init", "--id", member, "--name", member.upper(), "--team", "lab")
            config = root / member / "member.toml"
            config.write_text(f'''mailbox_url={json.dumps(url)}
credential_env="TEAM_{member.upper()}_TOKEN"
[[contacts]]
member_id="{peer}"
display_name="{peer.upper()}"
[executor]
mode="auto"
codex={json.dumps(str(fake_codex))}
workdir="work"
timeout_secs=20
[model]
provider="compatible"
model="local-mock-only"
base_url="http://127.0.0.1:{model.server_port}"
api_key_env=""
timeout_secs=5
''')
            call(member, "member", "configure", str(config))
        source = root / "dataset"
        source.mkdir()
        call("b", "roots", "add", str(source))
        (source / "data.csv").write_text("value\n1\n")
        obj = call("b", "object", "create", "--title", "Example dataset", "--kind", "dataset", "--shared")["id"]
        v1 = call("b", "object", "publish", obj, "--expected", "none", "--request-id", "release-v1", "--source", str(source))["id"]
        (source / "data.csv").write_text("value\n1\n2\n")
        v2 = call("b", "object", "publish", obj, "--expected", v1, "--request-id", "release-v2", "--source", str(source))["id"]
        diff = call("b", "version", "diff", v1, v2)
        call("b", "version", "verify", v1)
        call("b", "version", "export", v1, str(root / "export-v1"))
        assert (root / "export-v1/data.csv").read_text() == "value\n1\n"
        request = call("a", "message", "send", "--to", "b", "--body", "Which version is published?",
                       "--operation", "metadata", "--object-id", obj)["message"]
        call("a", "butler", "sync")
        assert call("b", "message", "inbox") == []  # B is offline.
        start("a")
        b_process = start("b")
        facts = await_reply(request["message_id"])
        assert facts["version_id"] == v2
        stop(b_process)
        followup = call("a", "message", "send", "--to", "b", "--body", "Please provide professional analysis of that version.",
                        "--reply-to", facts["message_id"])["message"]
        assert followup["object_id"] == obj and followup["version_id"] == v2
        call("a", "butler", "sync")
        start("b")  # Same identity/database after restart, real HTTP redelivery.
        reply = await_reply(followup["message_id"])
        assert "MOCK_RESULT" in reply["body"]
        delegation = call("b", "delegation", "status", followup["message_id"])
        assert delegation["state"] == "completed"
        again = call("b", "delegation", "run", followup["message_id"])
        assert again["task"]["id"] == delegation["task"]["id"] and len(again["executions"]) == 1
        requests = [{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "initiating-codex-demo", "version": "1"}}},
                    {"jsonrpc": "2.0", "method": "notifications/initialized"},
                    {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "collaboration_read_replies", "arguments": {"message_id": followup["message_id"]}}}]
        read = subprocess.run([str(args.binary.resolve()), "--db", str(root / "a/tasks.sqlite3"), "butler", "mcp-serve"],
                              input="".join(json.dumps(r) + "\n" for r in requests), env=env, text=True, capture_output=True, check=True)
        mcp_reply = json.loads(read.stdout.splitlines()[-1])["result"]
        assert not mcp_reply["isError"] and mcp_reply["structuredContent"]["replies"][0]["message"]["message_id"] == reply["message_id"]
        report.update(status="passed", object_id=obj, v1=v1, v2=v2, diff=diff, request_id=request["message_id"],
                      followup_id=followup["message_id"], reply_id=reply["message_id"], conversation_id=request["conversation_id"],
                      delegation=delegation, initiating_mcp_read_verified=True, offline_recovery_verified=True,
                      old_export_sha256=hashlib.sha256((root / "export-v1/data.csv").read_bytes()).hexdigest())
    except Exception as exc:
        report.update(status="failed", error=str(exc))
        raise
    finally:
        for child in reversed(children):
            try:
                stop(child)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        model.shutdown()
        model.server_close()
        for log in logs:
            log.close()
        (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(f"{report['status'].upper()}: {root / 'report.json'}")

if __name__ == "__main__":
    main()
