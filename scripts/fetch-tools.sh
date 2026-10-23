#!/usr/bin/env bash
# Fetches the prebuilt Windows tools sill bundles in tools\, pinned and checksummed:
#   rg.exe  ripgrep: agents reach for `rg` by habit (their own harnesses search with it)
#   jq.exe  jq: agents parse JSON with it; Windows and Git for Windows have neither
#
# Usage: scripts/fetch-tools.sh OUT_DIR   (writes OUT_DIR/rg.exe and OUT_DIR/jq.exe; needs curl, unzip)
set -euo pipefail

out=$1
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

fetch() { # url sha256 file
    curl -sfL --retry 3 -o "$3" "$1"
    local got
    got=$( (sha256sum "$3" 2>/dev/null || shasum -a 256 "$3") | cut -d' ' -f1)
    if [ "$got" != "$2" ]; then
        echo "fetch-tools: checksum mismatch for $1: $got" >&2
        exit 1
    fi
}

rg_version=15.2.0
rg_name="ripgrep-$rg_version-x86_64-pc-windows-msvc"
fetch "https://github.com/BurntSushi/ripgrep/releases/download/$rg_version/$rg_name.zip" \
    71b2fef860abe467217a538ff31de02f5258807c0129f771846f87bd029aafc5 "$work/rg.zip"
unzip -q -o "$work/rg.zip" "$rg_name/rg.exe" -d "$work"
cp "$work/$rg_name/rg.exe" "$out/rg.exe"

jq_version=1.8.2
fetch "https://github.com/jqlang/jq/releases/download/jq-$jq_version/jq-windows-amd64.exe" \
    a6fc67fedaf9128a3309a1e2ebb8b986aeccf70122ee46d2cb4849e423f0c627 "$out/jq.exe"

echo "fetched $out/rg.exe (ripgrep $rg_version), $out/jq.exe (jq $jq_version)"
