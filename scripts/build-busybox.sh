#!/usr/bin/env bash
# Builds sill's busybox-w32 (the native tools in dist/sill/tools) on Linux with mingw-w64.
#
# Upstream busybox-w32 at the FRP-6075 release commit, plus two changes:
#   - third_party/busybox-w32/sill.patch: sed keeps CR (GNU behaviour); upstream strips
#     it unless -b, so `sed -i` turned CRLF files into LF.
#   - CONFIG_GLOBBING off: the C runtime would expand wildcards in each tool's own
#     command line, even quoted ones, so `find -name '*.txt'` got file names. The shell
#     has already done all the globbing.
#
# Usage: scripts/build-busybox.sh OUT.exe   (needs git, make, gcc, x86_64-w64-mingw32-gcc)
set -euo pipefail

out=$(realpath -m "$1")
here=$(cd "$(dirname "$0")/.." && pwd)
rev=169694ebdba292f6533f78aeea678bfbb907396c
src=$(mktemp -d)/busybox-w32

git init -q "$src"
git -C "$src" fetch -q --depth 1 https://github.com/rmyorston/busybox-w32.git "$rev"
git -C "$src" checkout -q FETCH_HEAD
git -C "$src" apply "$here/third_party/busybox-w32/sill.patch"

cd "$src"
make -s mingw64u_defconfig
sed -i 's/^CONFIG_GLOBBING=y$/# CONFIG_GLOBBING is not set/' .config
make -s oldconfig </dev/null >/dev/null
grep -q '^# CONFIG_GLOBBING is not set' .config
make -s -j"$(nproc)" CROSS_COMPILE=x86_64-w64-mingw32-
cp busybox.exe "$out"
echo "built $out ($(stat -c %s "$out") bytes)"
