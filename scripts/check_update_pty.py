#!/usr/bin/env python3
"""Opt-in: real PTY upgrade in an isolated client, optionally followed by real Codex.

The supplied live client's configuration is read-only and only model/executor settings
are reused. A separate relay, identity, database, install prefix and file root are used.
"""
import argparse
import fcntl
import hashlib
import json
import os
import pathlib
import pty
import re
import select
import shutil
import sqlite3
import struct
import subprocess
import termios
import time
import uuid


def read(db, sql, args=()):
    # A read-only URI can fail to open a WAL database without its sidecars on
    # macOS. Allow SQLite to manage them, while keeping all queries read-only.
    with sqlite3.connect(db.resolve().as_uri() + "?mode=rw", uri=True) as conn:
        conn.execute("PRAGMA query_only=ON")
        conn.row_factory = sqlite3.Row
        return [dict(r) for r in conn.execute(sql, args)]


def toml(config):
    lines = []
    for name, value in config.items():
        if value is not None and not isinstance(value, (dict, list)):
            lines.append(f"{name} = {json.dumps(value, ensure_ascii=False)}")
    for name in ("executor", "model"):
        lines.append(f"[{name}]")
        for key, value in config[name].items():
            if value is not None:
                lines.append(f"{key} = {json.dumps(value, ensure_ascii=False)}")
    for contact in config["contacts"]:
        lines.append("[[contacts]]")
        lines.extend(f"{key} = {json.dumps(value)}" for key, value in contact.items())
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--root", type=pathlib.Path, required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--live-client", type=pathlib.Path)
    parser.add_argument("--automatic", action="store_true")
    args = parser.parse_args()
    root, binary = args.root.resolve(), args.binary.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    client = root / "client"
    client.mkdir()
    work = root / "work"
    work.mkdir()
    source = work / "update-check.txt"
    source.write_text("Synthetic updater acceptance data.\nmarker=UPDATER-8247\nsample_count=42\n")
    source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    db = client / "member.sqlite3"
    token = uuid.uuid4().hex
    secret = client / "secrets.env"
    secret.write_text(f"UPDATE_A_TOKEN={token}\n")
    secret.chmod(0o600)
    relay_bin = root / "relay-xxassxx"
    shutil.copy2(binary, relay_bin)
    relay_config = root / "relay.toml"
    relay_config.write_text("team_id='updater-test'\n[[members]]\nmember_id='a'\ndisplay_name='Updater test'\ncredential_env='UPDATE_A_TOKEN'\n[[members]]\nmember_id='b'\ndisplay_name='Test peer'\ncredential_env='UPDATE_B_TOKEN'\n")
    env = {**os.environ, "UPDATE_A_TOKEN": token, "UPDATE_B_TOKEN": uuid.uuid4().hex, "TERM": "xterm-256color"}
    relay = subprocess.Popen([str(relay_bin), "--db", str(root / "relay.sqlite3"), "mailbox", "serve", "--listen", "127.0.0.1:0", "--config", str(relay_config)], stdout=subprocess.PIPE, stderr=(root / "relay.log").open("w"), text=True, env=env)
    url = "http://" + json.loads(relay.stdout.readline())["listening"]
    master = slave = terminal = None
    data = bytearray()
    report = {"status": "running", "automatic": args.automatic, "source": args.source}

    def run(*parts):
        p = subprocess.run([str(binary), *map(str, parts)], capture_output=True, text=True, env=env, timeout=30)
        if p.returncode:
            raise RuntimeError(p.stderr[-1500:])
        return p.stdout

    def drain(seconds=0.3):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([master], [], [], 0.03)[0]:
                try:
                    data.extend(os.read(master, 65536))
                except OSError:
                    break

    def visible():
        return re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", data.decode("utf-8", "replace"))

    def keys(value):
        os.write(master, value)
        drain()

    def text(value):
        keys(b"\x1b[200~" + value.encode() + b"\x1b[201~")
        keys(b"\r")

    def wait(predicate, label, timeout=90):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            drain(0.3)
            if predicate():
                print(label, flush=True)
                return
            if terminal.poll() is not None:
                raise RuntimeError(f"terminal exited: {terminal.returncode}")
        raise RuntimeError("timeout: " + label)

    try:
        run("--db", db, "member", "init", "--id", "a", "--name", "Updater test", "--team", "updater-test")
        if args.live_client:
            live = args.live_client / "member.sqlite3"
            config = json.loads(read(live, "SELECT config FROM identity")[0]["config"])
        else:
            config = {"executor": {"mode": "queue", "codex": "codex", "workdir": str(work), "timeout_secs": 180}, "model": {"provider": "off"}}
        config.update(mailbox_url=url, credential_env="UPDATE_A_TOKEN", secrets_file=str(secret), allow_insecure_http=False, contacts=[{"member_id": "b", "display_name": "Test peer"}])
        config["executor"]["workdir"] = str(work)
        config["executor"]["timeout_secs"] = 600
        settings = client / "member.toml"
        settings.write_text(toml(config))
        settings.chmod(0o600)
        run("--db", db, "member", "configure", settings)
        run("client", "--directory", client, "roots", "add", work)
        session = json.loads(run("--db", db, "app", "new-session", "--project", work))["session_id"]
        run("update", "--configure", "--source", args.source, "--policy", "idle" if args.automatic else "notify")
        report["old_version"] = run("--version").strip()
        report["session"] = session
        before_identity = read(db, "SELECT * FROM identity")
        before_roots = read(db, "SELECT * FROM file_roots ORDER BY path")
        before_directories = json.loads(run("client", "--directory", client, "roots", "list"))["directories"]
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 42, 155, 0, 0))
        original_modes = termios.tcgetattr(slave)
        terminal = subprocess.Popen([str(binary), "open", str(work), "--directory", str(client), "--resume-session", session], stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True)
        wait(lambda: "发现新版" in visible(), "update notice visible")
        text("whoami")
        wait(lambda: bool(read(db, "SELECT id FROM app_commands WHERE state='done'")), "pre-update conversation exists")
        if not args.automatic:
            text("/update")
        wait(lambda: "更新检查完成；已回到原会话" in visible(), "upgrade and conversation reopen visible", 360)
        report["new_version"] = run("--version").strip()
        assert report["new_version"] != report["old_version"]
        assert read(db, "SELECT * FROM identity") == before_identity
        assert read(db, "SELECT * FROM file_roots ORDER BY path") == before_roots
        assert json.loads(run("client", "--directory", client, "roots", "list"))["directories"] == before_directories
        text("whoami")
        wait(lambda: len(read(db, "SELECT id FROM app_commands WHERE state='done'")) >= 2, "new UI responds in preserved conversation")
        assert {r["session_id"] for r in read(db, "SELECT session_id FROM app_commands")} == {session}
        report["identity_config_roots_history_session_preserved"] = True
        wait(lambda: read(db, "SELECT state FROM app_network WHERE singleton=1")[0]["state"] == "connected", "relay connection restored")
        drain(1.5)  # Let the terminal refresh its status before the final capture.
        report["relay_connection_restored"] = True
        if args.live_client:
            text("请创建并完成一个只读任务：读取当前工作目录的 update-check.txt，告诉我 marker 和 sample_count 的准确值，并说明来源。只在对话里报告这两个值及文件来源，不生成或发送附件。只读这个测试文件，不修改任何文件，不运行项目程序。")
            wait(lambda: bool(read(db, "SELECT id FROM app_tasks WHERE state='draft'")), "real task confirmation shown", 120)
            task = read(db, "SELECT id FROM app_tasks ORDER BY rowid DESC LIMIT 1")[0]["id"]
            keys(b"\x14")
            drain(0.6)
            keys(b"\r")
            keys(b"\x13")
            def completed():
                row = read(db, "SELECT state,error FROM app_tasks WHERE id=?", (task,))[0]
                if row["state"] == "needs_attention":
                    raise RuntimeError("real task needs attention: " + str(row["error"]))
                return row["state"] == "completed"
            wait(completed, "real task completed", 600)
            keys(b"\x14")
            drain(0.6)
            keys(b"\r")
            wait(lambda: "UPDATER-8247" in visible(), "meaningful final answer visible", 20)
            answer = read(db, "SELECT body FROM app_messages WHERE task_id=? AND kind='answer' ORDER BY rowid DESC", (task,))
            result = read(db, "SELECT result FROM app_tasks WHERE id=?", (task,))[0]["result"]
            report.update(task=task, answer=answer, result=result, runs=read(db, "SELECT state,turn_completed,exit_code,session_id FROM runs"))
            assert "UPDATER-8247" in str(answer) + str(result) and "42" in str(answer) + str(result)
            assert "UPDATER-8247" in visible()
            assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
            print(json.dumps({"task": task, "answer": answer, "result": result}, ensure_ascii=False), flush=True)
        (root / "before-exit.ansi").write_bytes(data)
        keys(b"\x11")
        terminal.wait(timeout=10)
        assert terminal.returncode == 0
        assert termios.tcgetattr(slave) == original_modes
        report["termios_restored"] = True
        report["status"] = "passed"
    except Exception as error:
        report["status"] = "failed"
        report["error"] = str(error)
        raise
    finally:
        if terminal and terminal.poll() is None:
            terminal.terminate()
            terminal.wait(timeout=15)
        if master is not None:
            os.close(master)
            os.close(slave)
        if db.exists():
            try:
                run("client", "--directory", client, "stop")
            except Exception:
                pass
        relay.terminate()
        relay.wait(timeout=10)
        (root / "terminal.ansi").write_bytes(data)
        (root / "evidence.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
        print(json.dumps(report, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
