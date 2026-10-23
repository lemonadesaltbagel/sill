#!/usr/bin/env bash
# Cross-builds GNU grep, sed, diffutils and make for Windows with mingw-w64 (static, no
# NLS), for dist/sill/tools. busybox's grep lacks --include/--exclude and its diff --exclude,
# which agents use constantly, and GNU is the reference behaviour. third_party/gnu-tools/shim.c turns off the C
# runtime's wildcard expansion and text mode.
#
# Usage: scripts/build-gnu-tools.sh OUT_DIR   (needs curl, make, x86_64-w64-mingw32-gcc)
set -euo pipefail

out=$(mkdir -p "$1" && cd "$1" && pwd)
here=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
host=x86_64-w64-mingw32
cc=$host-gcc

"$cc" -O2 -c "$here/third_party/gnu-tools/shim.c" -o "$work/shim.o"
# A UTF-8 active code page (third_party/gnu-tools/utf8.manifest): with the system code
# page, `grep x 한국어.txt` failed to open the file on a Chinese system, and any CJK or
# Cyrillic name on an English one.
(cd "$here/third_party/gnu-tools" && "$host-windres" utf8.rc -O coff -o "$work/utf8.o")

# GCC 14 made incompatible pointer types an error; diffutils' sdiff.c trips it.
cflags="-O2 -Wno-error=incompatible-pointer-types"

build() { # name version sha256 "configure args" outputs...   (TARBALL_EXT, EXTRA_CFLAGS)
    local name=$1 version=$2 sha=$3 extra=$4
    shift 4
    local tarball="$name-$version.${TARBALL_EXT:-tar.xz}"
    curl -sfL --retry 3 -o "$work/$tarball" "https://ftp.gnu.org/gnu/$name/$tarball"
    echo "$sha  $work/$tarball" | (command -v sha256sum >/dev/null && sha256sum -c --quiet || shasum -a 256 -c --quiet)
    tar -xf "$work/$tarball" -C "$work"
    (
        cd "$work/$name-$version"
        # shellcheck disable=SC2086 # $extra is a list of configure arguments
        ./configure -q --host="$host" --disable-nls --enable-threads=windows $extra \
            CFLAGS="$cflags ${EXTRA_CFLAGS:-}" LDFLAGS="-static -Wl,--wrap=main" LIBS="$work/shim.o $work/utf8.o -lbcrypt" >/dev/null
        make -s -j"$(getconf _NPROCESSORS_ONLN)" >/dev/null
    )
    for exe in "$@"; do
        cp "$work/$name-$version/$exe" "$out/"
        echo "built $out/$(basename "$exe") ($(wc -c < "$out/$(basename "$exe")") bytes)"
    done
}

build grep 3.12 2649b27c0e90e632eadcd757be06c6e9a4f48d941de51e7c0f83ff76408a07b9 \
    "--disable-perl-regexp" src/grep.exe
build sed 4.10 b8e72182b2ec96a3574e2998c47b7aaa64cc20ce000d8e9ac313cc07cecf28c7 \
    "" sed/sed.exe
# The strcasecmp check runs a test program, which a cross build can't.
build diffutils 3.12 7c8b7f9fc8609141fdea9cece85249d308624391ff61dedaf528fcb337727dfd \
    "gl_cv_func_strcasecmp_works=yes" src/diff.exe src/cmp.exe src/diff3.exe src/sdiff.exe
# GNU make: Unix Makefiles are common in repos and Windows has no make. On Windows it runs
# recipes with the sh.exe on PATH, which in the package is sill. make 4.4.1's K&R-style
# declarations need pre-C23 (GCC 15 defaults to C23); it ships no .tar.xz.
TARBALL_EXT=tar.gz EXTRA_CFLAGS=-std=gnu17 \
build make 4.4.1 dd16fb1d67bfab79a72f5e8390735c49e3e8e70b4945a15ab1f81ddb78658fb3 \
    "" make.exe
