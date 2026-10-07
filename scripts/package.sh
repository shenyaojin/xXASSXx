#!/bin/sh
# Package one already-built target. Outputs a checksum alongside its archive.
set -eu
[ "$#" -eq 3 ] || { echo 'Usage: package.sh BINARY TARGET OUTPUT_DIRECTORY' >&2; exit 2; }
binary=$1
target=$2
output=$3
case "$target" in aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-musl) ;; *) echo 'Unsupported release target' >&2; exit 2 ;; esac
mkdir -p "$output"
temp=$(mktemp -d "${TMPDIR:-/tmp}/xxassxx-package.XXXXXXXX")
trap 'rm -rf "$temp"' EXIT HUP INT TERM
install -m 755 "$binary" "$temp/xxassxx"
archive="xxassxx-${target}.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$output/$archive" -C "$temp" xxassxx
if command -v sha256sum >/dev/null 2>&1; then
  hash=$(sha256sum "$output/$archive" | awk '{print $1}')
else
  hash=$(shasum -a 256 "$output/$archive" | awk '{print $1}')
fi
printf '%s  %s\n' "$hash" "$archive" > "$output/$archive.sha256"
