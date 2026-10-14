#!/usr/bin/env python3
"""Sign bounded update metadata with an offline Ed25519 PEM key (OpenSSL 3+)."""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
TARGETS = ("aarch64-apple-darwin", "x86_64-unknown-linux-musl")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=pathlib.Path)
    parser.add_argument("--key", required=True, type=pathlib.Path)
    parser.add_argument("--version")
    parser.add_argument("--notes", type=pathlib.Path)
    parser.add_argument("--allow-partial", action="store_true", help="isolated platform validation only")
    args = parser.parse_args()
    version = args.version or re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M)[1]
    public = subprocess.check_output(["openssl", "pkey", "-in", str(args.key), "-pubout", "-outform", "DER"])
    if public[-32:].hex() != (ROOT / "src/update/public-key.hex").read_text().strip():
        raise SystemExit("Signing key does not match the embedded release public key")
    assets = {}
    for target in TARGETS:
        name = f"xxassxx-{target}.tar.gz"
        path = args.directory / name
        if not path.exists() and args.allow_partial:
            continue
        data = path.read_bytes()
        assets[target] = {"name": name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    if not assets:
        raise SystemExit("No release archives")
    notes = args.notes or ROOT / f"docs/releases/v{version}.md"
    schema = int(re.search(r"pub const SCHEMA_VERSION: u32 = (\d+)", (ROOT / "src/store.rs").read_text())[1])
    manifest = {"format": 1, "version": version, "schema": schema, "notes": notes.read_text()[:4000], "assets": assets}
    path = args.directory / "update.json"
    path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    with tempfile.TemporaryDirectory() as temp:
        signature = pathlib.Path(temp) / "signature"
        subprocess.run(["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(args.key), "-in", str(path), "-out", str(signature)], check=True)
        (args.directory / "update.json.sig").write_text(signature.read_bytes().hex() + "\n")
    print(f"Signed {version}: {', '.join(assets)}")


if __name__ == "__main__":
    main()
