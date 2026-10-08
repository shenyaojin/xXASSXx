#!/bin/sh
# Install a released binary into a user-owned directory. Requires no Rust or sudo.
set -eu

prefix="${HOME}/.local"
repository="shenyaojin/xXASSXx"
version="latest"
source_dir=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix|--version|--from)
      [ "$#" -ge 2 ] || { echo "Missing value for $1" >&2; exit 2; }
      case "$1" in
        --prefix) prefix=$2 ;;
        --version) version=$2 ;;
        --from) source_dir=$2 ;;
      esac
      shift 2 ;;
    --help)
      echo 'Usage: install.sh [--prefix ~/.local] [--version vX.Y.Z[-PRERELEASE]] [--from release-directory]'
      exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) target=aarch64-apple-darwin ;;
  Darwin/x86_64) target=x86_64-apple-darwin ;;
  Linux/x86_64) target=x86_64-unknown-linux-musl ;;
  *) echo 'Supported: macOS Apple Silicon/Intel and Linux x86_64.' >&2; exit 1 ;;
esac
archive="xxassxx-${target}.tar.gz"
temp=$(mktemp -d "${TMPDIR:-/tmp}/xxassxx-install.XXXXXXXX")
trap 'rm -rf "$temp"' EXIT HUP INT TERM

if [ -n "$source_dir" ]; then
  cp "$source_dir/$archive" "$temp/$archive"
  cp "$source_dir/SHA256SUMS" "$temp/SHA256SUMS"
else
  if [ "$version" = latest ]; then
    release_url=$(curl --proto '=https' --proto-redir '=https' -fsSL --connect-timeout 10 --max-time 30 \
      -o /dev/null -w '%{url_effective}' "https://github.com/$repository/releases/latest")
    version=${release_url##*/}
  fi
  case "$version" in
    v[0-9]*) ;;
    *) echo 'No published release found; use an explicit tag such as --version v0.3.0-alpha for a prerelease.' >&2; exit 1 ;;
  esac
  case "$version" in *[!A-Za-z0-9._-]*) echo 'Invalid release tag.' >&2; exit 1 ;; esac
  base="https://github.com/$repository/releases/download/$version"
  for file in "$archive" SHA256SUMS; do
    curl --proto '=https' --proto-redir '=https' -fsSL --connect-timeout 10 --max-time 180 \
      "$base/$file" -o "$temp/$file"
  done
fi

expected=$(awk -v name="$archive" '$2 == name { print $1; n++ } END { if (n != 1) exit 1 }' "$temp/SHA256SUMS") || {
  echo 'Missing or ambiguous archive checksum.' >&2; exit 1;
}
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$temp/$archive" | awk '{print $1}')
else
  actual=$(shasum -a 256 "$temp/$archive" | awk '{print $1}')
fi
[ "$actual" = "$expected" ] || { echo 'Checksum mismatch; nothing installed.' >&2; exit 1; }
tar -xzf "$temp/$archive" -C "$temp" xxassxx
[ -f "$temp/xxassxx" ] && [ ! -L "$temp/xxassxx" ] || { echo 'Invalid binary archive.' >&2; exit 1; }
mkdir -p "$prefix/bin"
[ ! -d "$prefix/bin/xxassxx" ] && [ ! -L "$prefix/bin/xxassxx" ] || {
  echo 'Existing destination is a directory or symlink; choose another prefix.' >&2; exit 1;
}
staged="$prefix/bin/.xxassxx-install-$$"
trap 'rm -f "$staged"; rm -rf "$temp"' EXIT HUP INT TERM
install -m 755 "$temp/xxassxx" "$staged"
"$staged" --version
mv -f "$staged" "$prefix/bin/xxassxx"
echo "Installed: $prefix/bin/xxassxx"
echo "Ensure $prefix/bin is in PATH. Next: xxassxx client init --help"
