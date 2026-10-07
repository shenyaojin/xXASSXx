#!/usr/bin/env python3
"""Fake official CLI; all task access uses the real Rust MCP subprocess."""
import json
import os
import re
import subprocess
import sys
import time
import tomllib

mode = os.environ.get("XXASSXX_MOCK_MODE", "success")
args = sys.argv[1:]
SESSION = "11111111-2222-4333-8444-555555555555"

def event(value):
    print(json.dumps(value), flush=True)

def record_codex_pid():
    with open(os.environ["XXASSXX_MOCK_CODEX_PID"], "w") as f:
        f.write(str(os.getpid()))

if args == ["--version"]:
    if mode == "preflight_timeout":
        record_codex_pid()
        time.sleep(30)
    print("codex-cli mock-0.144.5")
    sys.exit(0)
if args == ["login", "status"]:
    if mode == "not_logged_in":
        print("Not logged in", file=sys.stderr)
        sys.exit(1)
    print("Logged in using ChatGPT", file=sys.stderr)
    sys.exit(0)

assert args[0] == "exec"
assert "--json" in args and "--ignore-user-config" in args
assert "--last" not in args and "--ephemeral" not in args
assert args[-1] == "-"
if "resume" in args:
    assert args[args.index("resume") + 1] == SESSION
    session = SESSION
else:
    session = SESSION
config = {}
for i, arg in enumerate(args):
    if arg == "-c":
        key, value = args[i + 1].split("=", 1)
        config[key] = tomllib.loads("value=" + value)["value"]
assert config["approval_policy"] == "never"
native = config.get("default_permissions") == "xxassxx_read"
if native:
    assert config["features.shell_tool"]
    assert config["permissions.xxassxx_read.filesystem"][":minimal"] == "read"
    assert not config["permissions.xxassxx_read.network.enabled"]
    assert "--strict-config" in args and "sandbox_mode" not in config
    assert "mcp_servers.xxassxx_butler.command" not in config
else:
    assert config["sandbox_mode"] == "read-only"
    assert not config["features.shell_tool"]
assert config["mcp_servers.xxassxx.required"]
assert os.getcwd() == config["mcp_servers.xxassxx.cwd"]
prompt = sys.stdin.read()
assert "UNTRUSTED_TASK_TEXT" not in prompt
assert "XXASSXX_RUN_TOKEN" not in prompt
if record := os.environ.get("XXASSXX_MOCK_RECORD"):
    with open(record, "w") as f:
        json.dump({"argv": args, "cwd": os.getcwd(), "prompt": prompt}, f)
if mode == "wrong_session":
    session = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
event({"type": "thread.started", "thread_id": session})
event({"type": "turn.started"})
if mode == "quota":
    event({"type": "turn.failed", "error": {"message": "usage limit reached", "code": "insufficient_quota"}})
    sys.exit(1)
if mode == "failure":
    print("simulated process failure", file=sys.stderr)
    sys.exit(7)
if mode == "mcp_init_failure":
    print("MCP xxassxx failed to initialize", file=sys.stderr)
    sys.exit(1)
if mode == "malformed":
    print("not JSON", flush=True)
    sys.exit(0)
if mode == "no_mcp":
    event({"type": "item.completed", "item": {"type": "agent_message", "text": "完成了"}})
    event({"type": "turn.completed"})
    sys.exit(0)

env = os.environ.copy()
mcp = subprocess.Popen([config["mcp_servers.xxassxx.command"], *config["mcp_servers.xxassxx.args"]],
                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                       text=True, env=env, cwd=config["mcp_servers.xxassxx.cwd"])
counter = 0
def rpc(method, params=None, notify=False):
    global counter
    counter += 1
    request = {"jsonrpc": "2.0", "method": method, "params": params or {}}
    if not notify:
        request["id"] = counter
    mcp.stdin.write(json.dumps(request) + "\n")
    mcp.stdin.flush()
    if notify:
        return
    response = json.loads(mcp.stdout.readline())
    assert response["id"] == counter and "error" not in response, response
    return response["result"]

rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fake-codex", "version": "1"}})
rpc("notifications/initialized", notify=True)
assert len(rpc("tools/list")["tools"]) == 2
task_id = re.search(r"Assigned task_id: ([a-f0-9-]+)", prompt)[1]
run_id = re.search(r"Current run_id: ([a-f0-9-]+)", prompt)[1]
def tool(name, arguments, expect_error=False):
    result = rpc("tools/call", {"name": name, "arguments": arguments})
    assert bool(result.get("isError")) == expect_error, result
    event({"type": "item.completed", "item": {"type": "mcp_tool_call", "server": "xxassxx", "tool": name, "status": "completed", "result": result}})
    return result

if mode == "wrong_identity":
    tool("get_task", {"task_id": "someone-else"}, True)
data = tool("get_task", {"task_id": task_id})["structuredContent"]
assert data["run_id"] == run_id
if mode in ("timeout", "hold"):
    record_codex_pid()
    with open(os.environ["XXASSXX_MOCK_PID"], "w") as f:
        f.write(str(mcp.pid))
    time.sleep(30)
if mode != "missing_result":
    submission = {"task_id": task_id, "run_id": run_id, "idempotency_key": run_id, "result": "MOCK_RESULT"}
    if native:
        task = json.loads(data["input"])
        root = task["authorized_roots"][0]["path"]
        assert config["permissions.xxassxx_read.filesystem"][root] == "read"
        with open(os.path.join(root, "found.txt")) as f:
            assert f.read() == "search fixture"
        event({"type":"item.completed","item":{"type":"command_execution","command":"cat found.txt","exit_code":0,"status":"completed"}})
        submission["result"] = json.dumps({"body":"找到相关文件。","files":[{"root":0,"path":"found.txt","reason":"与请求相关"}]})
    if mode == "wrong_identity":
        tool("submit_task_result", {**submission, "run_id": "old-run"}, True)
    tool("submit_task_result", submission)
    if mode == "duplicate":
        duplicate = tool("submit_task_result", submission)["structuredContent"]
        assert duplicate["duplicate"]
        tool("submit_task_result", {**submission, "result": "different"}, True)
        tool("submit_task_result", {**submission, "idempotency_key": "different"}, True)
if mode == "stderr_flood":
    print("diagnostic noise\n" * 20000, file=sys.stderr, flush=True)
event({"type": "future.compatible.event", "value": 1})
mcp.stdin.close()
assert mcp.wait(timeout=5) == 0
if mode == "fail_after_result":
    event({"type": "turn.failed", "error": {"message": "failed after result"}})
    sys.exit(9)
if mode == "terminal_then_completed":
    event({"type": "turn.failed", "error": {"message": "terminal failure"}})
if mode == "transient_recovered":
    event({"type": "error", "message": "reconnecting"})
if mode != "no_completion":
    event({"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}})
