#!/usr/bin/env python3
"""Opt-in HTTPS update/failure acceptance against an isolated managed mailbox."""
import argparse
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import urllib.request
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=pathlib.Path)
    parser.add_argument("--root", required=True, type=pathlib.Path)
    parser.add_argument("--public", required=True, type=pathlib.Path)
    parser.add_argument("--source", required=True)
    args = parser.parse_args()
    binary, root = args.binary.resolve(), args.root.resolve()
    root.mkdir(parents=True, mode=0o700, exist_ok=False)
    report = {"status": "running", "checks": []}
    relay = None

    def digest():
        return hashlib.sha256(binary.read_bytes()).hexdigest()

    def update(source):
        p = subprocess.run([str(binary), "update", "--source", source, "--directory", str(root / "no-client")], text=True, capture_output=True, timeout=240)
        report["checks"].append({"source": source, "exit": p.returncode, "output": p.stdout + p.stderr})
        return p

    token = uuid.uuid4().hex
    config = root / "mailbox.toml"
    config.write_text("team_id='updater-server-test'\n[[members]]\nmember_id='a'\ndisplay_name='Acceptance member'\ncredential_env='UPDATE_SERVER_TOKEN'\n[[members]]\nmember_id='b'\ndisplay_name='Test peer'\ncredential_env='UPDATE_OTHER_TOKEN'\n")

    def start():
        proc = subprocess.Popen([str(binary), "--db", str(root / "mailbox.sqlite3"), "mailbox", "serve", "--listen", "127.0.0.1:0", "--config", str(config)], stdout=subprocess.PIPE, stderr=(root / "mailbox.log").open("a"), env={**os.environ, "UPDATE_SERVER_TOKEN": token, "UPDATE_OTHER_TOKEN": uuid.uuid4().hex}, text=True)
        address = json.loads(proc.stdout.readline())["listening"]
        request = urllib.request.Request(f"http://{address}/v1/whoami", headers={"Authorization": "Bearer " + token})
        with urllib.request.urlopen(request, timeout=10) as response:
            identity = json.load(response)
        return proc, identity

    original = digest()
    try:
        report["old_version"] = subprocess.check_output([str(binary), "--version"], text=True).strip()
        # Copies contain public release artifacts only, never user data or keys.
        for kind in ("bad-signature", "truncated"):
            fixture = args.public / kind
            shutil.copytree(args.public / "verified", fixture, dirs_exist_ok=True)
            if kind == "bad-signature":
                (fixture / "update.json.sig").write_text("00" * 64 + "\n")
            else:
                (fixture / "xxassxx-x86_64-unknown-linux-musl.tar.gz").write_bytes(b"incomplete download")
            assert update(args.source + "/" + kind).returncode != 0
            assert digest() == original
        relay, identity = start()
        result = update(args.source + "/verified")
        assert result.returncode != 0 and "其他终端" in result.stderr
        assert relay.poll() is None and digest() == original
        report["running_mailbox_prevents_replacement"] = True
        relay.terminate()
        relay.wait(timeout=15)
        relay = None
        assert update(args.source + "/verified").returncode == 0
        assert digest() != original
        report["new_version"] = subprocess.check_output([str(binary), "--version"], text=True).strip()
        relay, new_identity = start()
        assert new_identity == identity
        assert not (root / "no-client").exists()
        report.update(status="passed", server_identity_preserved=True, binary_sha256=digest())
    except Exception as error:
        report.update(status="failed", error=str(error))
        raise
    finally:
        if relay and relay.poll() is None:
            relay.terminate()
            relay.wait(timeout=15)
        (root / "evidence.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
        print(json.dumps(report, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
