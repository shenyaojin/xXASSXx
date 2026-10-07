#!/usr/bin/env python3
"""Two Linux hosts, real mailbox/butlers/MCP, model off and simulated Codex.

Requires a prebuilt xxassxx on each host and Python 3.11+ on host B for the
existing Codex fixture. Uses existing SSH aliases; creates fresh test data only.
All services are supervised through SSH stdin and stopped before returning.
The Mac/controller carries SSH tunnels; this is not a production deployment.
"""
import argparse
import json
from pathlib import Path
import queue
import secrets
import shlex
import socket
import subprocess
import tempfile
import threading
import time
import uuid

REPO = Path(__file__).resolve().parents[1]

# Runs on either Linux host using its existing Python (3.8 is sufficient).
# Payloads travel inside SSH stdin, including disposable test credentials.
REMOTE = r'''
import hashlib, json, os, pathlib, signal, socket, subprocess, sys
p = json.loads(sys.stdin.readline())
root = pathlib.Path.home() / "xxassxx-tests" / p["name"]
binary = pathlib.Path(p["binary"])
env = os.environ.copy()
env.update(p["env"])
env["PATH"] = str(binary.parent) + os.pathsep + env.get("PATH", "")
op = p["op"]
if op == "setup":
    assert binary.is_file(), str(binary)
    root.mkdir(parents=True, exist_ok=False)
    root.chmod(0o700)
    (root / "work").mkdir()
    print(json.dumps({"root":str(root), "host":socket.gethostname(),
        "binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest()}))
elif op == "write":
    path = (root / p["path"]).resolve()
    assert root.resolve() in path.parents
    path.write_text(p["text"])
    path.chmod(p.get("mode", 0o600))
    print("{}")
elif op == "port":
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        print(json.dumps(sock.getsockname()[1]))
elif op in ("cli", "daemon"):
    command = [str(binary), "--db", str(root / p["db"])] + p["args"]
    if op == "cli":
        result = subprocess.run(command, input=p.get("input", ""), text=True,
            capture_output=True, env=env, cwd=str(root), timeout=90)
        print(json.dumps({"code":result.returncode,"out":result.stdout,"err":result.stderr}))
    else:
        child = subprocess.Popen(command, stdin=subprocess.DEVNULL,
            env=env, cwd=str(root), start_new_session=True)
        print(json.dumps({"started_pid":child.pid}), flush=True)
        try:
            sys.stdin.readline()  # Explicit stop or SSH disconnect (EOF).
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGINT)
            try:
                child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
            print(json.dumps({"stopped_pid":child.pid,"exit_code":child.returncode}), flush=True)
else:
    raise RuntimeError("unknown operation")
'''


class Host:
    def __init__(self, alias, binary, name, env, control):
        self.alias, self.binary, self.name, self.env = alias, binary, name, env
        self.control = str(control)

    def ssh(self, *args):
        return ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10",
                "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3",
                "-S", self.control, *args, self.alias]

    def payload(self, op, **kw):
        return dict(op=op, name=self.name, binary=self.binary, env=self.env, **kw)

    def command(self):
        path = shlex.quote(str(Path(self.binary).parent))
        return self.ssh() + ['PATH='+path+':"$PATH" python3 -u -c '+shlex.quote(REMOTE)]

    def rpc(self, op, **kw):
        result = subprocess.run(self.command(), input=json.dumps(self.payload(op, **kw))+"\n",
                                text=True, capture_output=True, timeout=110)
        if result.returncode:
            raise RuntimeError(self.alias + ": " + result.stderr)
        return json.loads(result.stdout)

    def raw(self, *args, db="member.sqlite3", input=""):
        result = self.rpc("cli", db=db, args=list(args), input=input)
        if result["code"]:
            raise RuntimeError(self.alias + ": " + result["err"])
        return result["out"]

    def call(self, *args, **kw):
        return json.loads(self.raw(*args, **kw))

    def write(self, path, text, mode=0o600):
        self.rpc("write", path=path, text=text, mode=mode)

    def forward(self, option, spec):
        subprocess.run(self.ssh("-O", "forward", "-o", "ExitOnForwardFailure=yes", option, spec),
                       check=True, capture_output=True, text=True, timeout=15)


class Service:
    def __init__(self, host, output, label, args, db="member.sqlite3"):
        self.lines = queue.Queue()
        self.observed = {}
        self.err = open(output / (label+".stderr.log"), "w")
        self.out = open(output / (label+".stdout.log"), "w")
        self.child = subprocess.Popen(host.command(), text=True, stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=self.err)
        self.child.stdin.write(json.dumps(host.payload("daemon", db=db, args=args))+"\n")
        self.child.stdin.flush()
        self.thread = threading.Thread(target=self.read, daemon=True)
        self.thread.start()

    def read(self):
        for line in self.child.stdout:
            self.out.write(line)
            self.out.flush()
            self.lines.put(json.loads(line))

    def wait_for(self, field):
        deadline = time.monotonic()+30
        while time.monotonic() < deadline:
            if field in self.observed:
                return self.observed[field]
            value = self.lines.get(timeout=max(0.1, deadline-time.monotonic()))
            self.observed.update(value)
            if field in value:
                return value[field]
        raise TimeoutError(field)

    def stop(self):
        if not self.child.stdin.closed:
            self.child.stdin.close()
        self.child.wait(timeout=25)
        self.thread.join(timeout=5)
        stops = [line for line in [self.observed, *list(self.lines.queue)] if "stopped_pid" in line]
        self.out.close()
        self.err.close()
        if self.child.returncode or not stops or any(line["exit_code"] != 0 for line in stops):
            raise RuntimeError("Remote service did not confirm graceful shutdown: "+str(stops))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host-a", required=True, help="Mailbox + requesting butler")
    parser.add_argument("--host-b", required=True, help="Knowledge owner + simulated Codex")
    parser.add_argument("--binary-a", required=True, help="Absolute remote binary path")
    parser.add_argument("--binary-b", required=True, help="Absolute remote binary path; its directory must supply Python 3.11+ as python3")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    name = "ssh-demo-"+str(uuid.uuid4())
    env = dict(TEAM_A_TOKEN=secrets.token_hex(24), TEAM_B_TOKEN=secrets.token_hex(24),
               XXASSXX_MOCK_MODE="success", TOKIO_WORKER_THREADS="2")
    report = dict(status="running", run_id=name, model="off", codex="mock process",
                  transport="loopback HTTP through SSH tunnels via controller",
                  real_codex="not_run", real_deepseek="not_run", checks={})
    services, masters, master_logs = [], [], []
    control_dir = tempfile.TemporaryDirectory(prefix="xxssh-", dir="/tmp")
    a = Host(args.host_a, args.binary_a, name, env, Path(control_dir.name)/"a")
    b = Host(args.host_b, args.binary_b, name, env, Path(control_dir.name)/"b")
    error = None
    try:
        for label, host in [("a", a), ("b", b)]:
            log = open(output / (label+"-ssh.log"), "w")
            master_logs.append(log)
            master = subprocess.Popen(host.ssh("-M", "-N", "-o", "ControlPersist=no"),
                                      stdout=log, stderr=log)
            masters.append(master)
            deadline = time.monotonic()+25
            while not Path(host.control).exists():
                if master.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("SSH connection failed: "+host.alias)
                time.sleep(0.1)
            host.info = host.rpc("setup")
        report["hosts"] = {"a":dict(alias=a.alias, **a.info), "b":dict(alias=b.alias, **b.info)}
        assert a.info["binary_sha256"] == b.info["binary_sha256"]
        report["checks"]["identical_linux_binary"] = True
        print("Both Linux hosts connected; binary checksums match.", flush=True)
        a.write("mailbox.toml", (REPO/"examples/mailbox.toml").read_text())
        b.write("mock-codex", (REPO/"tests/fixtures/codex.py").read_text(), 0o700)
        relay_port = a.rpc("port")
        def start(host, label, *parts, db="member.sqlite3"):
            service = Service(host, output, label, list(parts), db)
            services.append(service)
            service.wait_for("started_pid")
            return service
        def relay(label):
            service = start(a, label, "mailbox", "serve", "--listen", f"127.0.0.1:{relay_port}",
                            "--config", a.info["root"]+"/mailbox.toml", db="relay.sqlite3")
            service.wait_for("listening")
            return service
        relay_service = relay("relay-1")
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            local_port = sock.getsockname()[1]
        a.forward("-L", f"127.0.0.1:{local_port}:127.0.0.1:{relay_port}")
        b_port = b.rpc("port")
        b.forward("-R", f"127.0.0.1:{b_port}:127.0.0.1:{local_port}")
        for host, member, peer, port in [(a,"a","b",relay_port), (b,"b","a",b_port)]:
            host.call("member", "init", "--id", member, "--name", member.upper(), "--team", "lab")
            config = f'''mailbox_url="http://127.0.0.1:{port}"
credential_env="TEAM_{member.upper()}_TOKEN"
[[contacts]]
member_id="{peer}"
display_name="{peer.upper()}"
[executor]
mode="auto"
codex={json.dumps(b.info["root"]+"/mock-codex" if member == "b" else "codex")}
workdir="work"
timeout_secs=30
[model]
provider="off"
'''
            host.write("member.toml", config)
            host.call("member", "configure", host.info["root"]+"/member.toml")
            host.call("butler", "sync")
        obj = b.call("object", "create", "--title", "Cross-machine decision", "--kind", "decision", "--shared")["id"]
        v1 = b.call("object", "publish", obj, "--expected", "none", "--request-id", "v1", "--text", "PRIVATE_CONTENT_V1")["id"]
        v2 = b.call("object", "publish", obj, "--expected", v1, "--request-id", "v2", "--text", "PRIVATE_CONTENT_V2")["id"]
        b.call("version", "verify", v1)
        request_args = ["message", "send", "--to", "b", "--body", "Which version is published?",
                        "--operation", "metadata", "--object-id", obj, "--message-id", str(uuid.uuid4())]
        request = a.call(*request_args)["message"]
        a.call("butler", "sync")
        assert b.call("message", "inbox") == []
        relay_service.stop()
        services.remove(relay_service)
        relay_service = relay("relay-2")
        a_service = start(a, "a-butler", "butler", "run", "--poll-secs", "1")
        b_service = start(b, "b-butler-1", "butler", "run", "--poll-secs", "1")
        def reply_to(message_id):
            deadline = time.monotonic()+60
            while time.monotonic() < deadline:
                replies = [r["message"] for r in a.call("message", "inbox") if r["message"]["reply_to"] == message_id]
                if replies:
                    assert len(replies) == 1
                    return replies[0]
                time.sleep(0.5)
            raise TimeoutError("Reply did not arrive")
        facts = reply_to(request["message_id"])
        assert facts["version_id"] == v2 and "PRIVATE_CONTENT" not in facts["body"]
        assert a.call("object", "list") == []
        report["checks"].update(offline_delivery_after_mailbox_restart=True, cross_machine_metadata_reply=True,
                                 separate_knowledge_stores=True, metadata_reply_excludes_content=True)
        print("Cross-machine reply received after mailbox restart.", flush=True)
        b_service.stop()
        services.remove(b_service)
        followup = a.call("message", "send", "--to", "b", "--body", "Analyze the referenced version.",
                          "--operation", "analysis", "--reply-to", facts["message_id"])["message"]
        assert followup["version_id"] == v2 and followup["conversation_id"] == request["conversation_id"]
        a.call("butler", "sync")
        v3 = b.call("object", "publish", obj, "--expected", v2, "--request-id", "v3", "--text", "PRIVATE_CONTENT_V3")["id"]
        b_service = start(b, "b-butler-2", "butler", "run", "--poll-secs", "1")
        reply = reply_to(followup["message_id"])
        assert "MOCK_RESULT" in reply["body"] and reply["version_id"] == v2
        delegation = b.call("delegation", "status", followup["message_id"])
        assert delegation["state"] == "completed" and len(delegation["executions"]) == 1
        again = b.call("delegation", "run", followup["message_id"])
        assert again["task"]["id"] == delegation["task"]["id"] and len(again["executions"]) == 1
        duplicate = a.call(*request_args)["message"]
        assert duplicate["message_id"] == request["message_id"]
        for host in (a, b, a):
            host.call("butler", "sync")
        for host in (a, b):
            messages = host.call("conversation", "show", request["conversation_id"])["messages"]
            assert len(messages) == 4 and len({m["message"]["message_id"] for m in messages}) == 4
        rpc = [dict(jsonrpc="2.0", id=1, method="initialize", params=dict(protocolVersion="2025-06-18", capabilities={}, clientInfo=dict(name="cross-machine-smoke",version="1"))),
               dict(jsonrpc="2.0", method="notifications/initialized"),
               dict(jsonrpc="2.0", id=2, method="tools/call", params=dict(name="collaboration_read_replies", arguments=dict(message_id=followup["message_id"])))]
        mcp = [json.loads(line) for line in a.raw("butler", "mcp-serve", input="".join(json.dumps(r)+"\n" for r in rpc)).splitlines()]
        result = next(r for r in mcp if r.get("id") == 2)["result"]
        assert not result["isError"] and result["structuredContent"]["replies"][0]["message"]["message_id"] == reply["message_id"]
        report["checks"].update(butler_restart_recovery=True, followup_pinned_to_v2_after_v3=True,
                                 one_delegation_one_execution=True, repeated_message_no_duplicate=True,
                                 owner_mcp_reads_remote_reply=True)
        print("Restart, version pinning, simulated Codex delegation and MCP reply checks passed.", flush=True)
        report.update(status="passed", object_id=obj, v1=v1, v2=v2, v3=v3, conversation_id=request["conversation_id"],
                      request_id=request["message_id"], followup_id=followup["message_id"], reply_id=reply["message_id"],
                      delegation=delegation)
    except Exception as exc:
        error = exc
        report.update(status="failed", error=str(exc))
    finally:
        cleanup_errors = []
        for service in reversed(services):
            try:
                service.stop()
            except Exception as exc:
                cleanup_errors.append(str(exc))
        for host in (a, b):
            if Path(host.control).exists():
                try:
                    subprocess.run(host.ssh("-O", "exit"), capture_output=True, timeout=15, check=True)
                except Exception as exc:
                    cleanup_errors.append(str(exc))
        for master in masters:
            try:
                master.wait(timeout=15)
            except subprocess.TimeoutExpired:
                master.terminate()
                master.wait(timeout=5)
                cleanup_errors.append("SSH master needed termination")
        for log in master_logs:
            log.close()
        control_dir.cleanup()
        report["cleanup"] = dict(services_stopped=not cleanup_errors, errors=cleanup_errors)
        if cleanup_errors:
            report["status"] = "failed"
        (output/"report.json").write_text(json.dumps(report, indent=2)+"\n")
        print(report["status"].upper()+": "+str(output/"report.json"))
    if error:
        raise error
    if report["status"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
