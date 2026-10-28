# Checks the packaged sill (scripts/package-windows.ps1) the way agents use it, on a
# machine with no Git: run with dist\sill\bash.exe and PATH=C:\Windows\System32.
# Prints PASS/FAIL per check; exits 1 if any fail.

fails=0
check() { # name, expected, actual
    if [[ "$2" == "$3" ]]; then
        echo "PASS $1"
    else
        echo "FAIL $1"
        printf '  expected: %q\n  actual:   %q\n' "$2" "$3"
        fails=$((fails + 1))
    fi
}

work="${TMPDIR:-/tmp}/sill-dist-check-$$"
mkdir -p "$work/src/sub" && cd "$work" || exit 1
printf 'alpha 1\nbeta 2\nTODO gamma 3\n' > src/a.txt
printf 'TODO delta\n' > src/sub/b.py
printf 'x\r\ny\r\n' > crlf.txt

check "grep from tools" "tools" "$(basename "$(dirname "$(command -v grep)")")"
check "which" "tools" "$(basename "$(dirname "$(which sed)")")"
check "grep -rn" "src/a.txt:3:TODO gamma 3|src/sub/b.py:1:TODO delta" \
    "$(grep -rn TODO src | sort | tr '\n' '|' | sed 's/|$//')"
check "grep -c / -E" "2" "$(grep -cE '^(alpha|beta)' src/a.txt)"
check "grep no match exit 1" "1" "$(grep -q nomatch src/a.txt; echo $?)"
if command -v make >/dev/null; then
    printf 'all:\n\t@echo "make via $$(basename "$$0")"\n' > Makefile.sill-check
    check "GNU make, recipes in sill" "make via sh.exe" "$(make -s -f Makefile.sill-check | tr -d '\r')"
    rm -f Makefile.sill-check
fi
check "ps aux lists this shell" "yes" "$(ps aux | grep -q "$$" && echo yes)"
check "cygpath -w" 'C:\Windows' "$(cygpath -w /c/Windows)"
echo needle > "한국어 😀.txt"
check "GNU grep opens a name outside the code page" "한국어 😀.txt" "$(grep -l needle "한국어 😀.txt")"
mkdir -p fe/sub && echo x > fe/sub/n.txt
printf '#!/bin/sh\necho ran\n' > run.sh
check "chmod +x, then run" "ran" "$(chmod +x run.sh && ./run.sh)"
check "chmod a-w makes a file read-only" "yes" "$(echo x > ro.txt && chmod a-w ro.txt && { [ -w ro.txt ] && echo no || echo yes; })"
check "chmod u+w makes it writable again" "yes" "$(chmod u+w ro.txt && [ -w ro.txt ] && echo yes)"
check "x=\$(native program) has no CR" "42" "$(x=$(cmd.exe //d //c echo 42); printf '%s' "$x")"
rm -f run.sh ro.txt
check "find -exec gets / paths" "fe/sub/n.txt" "$(find fe -name n.txt -exec grep -l x {} + | tr -d '\r')"
rm -rf fe
check "ls of 3000 args (args file)" "3000" "$(mkdir -p many && (cd many && for i in $(seq 1 3000); do : > "a_longish_file_name_$i.txt"; done) && ls many/* | wc -l)"
echo hit >> many/a_longish_file_name_7.txt
check "GNU grep over 3000 args (args file)" "many/a_longish_file_name_7.txt" "$(grep -l hit many/* | tr -d '\r')"
rm -rf many "한국어 😀.txt"
if command -v jq >/dev/null; then
    check "jq -r (LF, no CR)" "x" "$(echo '{"a":"x"}' | jq -r .a)"
fi
if command -v rg >/dev/null; then
    check "rg -n (ripgrep)" "src/a.txt:3:TODO gamma 3" "$(rg -n --no-heading 'TODO gamma' src | tr -d '\r')"
fi
sed -i 's/beta/BETA/' src/a.txt
check "sed -i" "BETA 2" "$(sed -n 2p src/a.txt)"
check "grep -r --include (GNU)" "src/sub/b.py:1:TODO delta" "$(grep -rn --include='*.py' TODO src)"
printf 'u1\nu2\n' > lf.txt; sed -i 's/u1/U1/' lf.txt
check "sed -i keeps LF" "U 1 \n u 2 \n" "$(od -An -c lf.txt | tr -s ' ' | sed 's/^ //;s/ $//')"
sed -i 's/x/X/' crlf.txt
# Bytes, not lines: `od -c` shows whether the \r survived the in-place edit.
check "sed -i keeps CRLF" "X \r \n y \r \n" "$(od -An -c crlf.txt | tr -s ' ' | sed 's/^ //;s/ $//')"
check "read keeps CR (bash does)" "y|13" "$(sed -n 2p crlf.txt | { IFS= read -r l; printf '%s|%d' "${l%$'\r'}" "'${l: -1}"; })"
check "awk sum" "6" "$(awk '{s += $NF} END {print s}' src/a.txt)"
check "find -name" "./src/a.txt" "$(find . -name '*.txt' -path '*src*' -type f)"
check "find | xargs" "2" "$(find src -type f | xargs grep -l TODO | wc -l | tr -d ' ')"
printf 'one\n' > d1; printf 'two\n' > d2
check "diff exit 1" "1" "$(diff -u d1 d2 > /dev/null; echo $?)"
check "diff -u body" "-one|+two" "$(diff -u d1 d2 | grep -E '^[-+][a-z]' | tr '\n' '|' | sed 's/|$//')"
check "timeout" "124" "$(timeout 1 sleep 5; echo $?)"
check "gzip round trip" "hello" "$(echo hello | gzip | gunzip)"
check "xxd -p" "4142" "$(printf AB | xxd -p)"
check "coreutils builtin" "ls is a shell builtin" "$(type ls)"
check "ls hides dotfiles" "crlf.txt d1 d2 lf.txt src" "$(touch .hidden; ls | tr '\n' ' ' | sed 's/ $//')"
check "PATH is POSIX form" "posix" "$(case "$PATH" in *";"*) echo windows;; /*) echo posix;; *) echo other;; esac)"
check "find /c/... (busybox gets the drive path)" "cmd.exe" "$(find /c/Windows/System32 -maxdepth 1 -iname cmd.exe | sed 's|.*/||' | tr 'A-Z' 'a-z')"
check "cmd //c (Git Bash idiom)" "via-double-slash" "$(cmd //d //c echo via-double-slash | tr -d '\r')"
win="$TEMP\\sill dist check"; mkdir -p "$win" && printf '@echo from-bat\r\n' > "$win\\m.bat"
check "cmd /c '<string>' with quoted path" "from-bat|ok" "$(cmd.exe /c "call \"$win\\m.bat\" && echo ok" | tr -d '\r' | tr '\n' '|' | sed 's/|$//')"
echo shared > /tmp/sill-dist-share.txt
check "/tmp shared with a native program" "shared" "$(cmd.exe /d /c type '\tmp\sill-dist-share.txt' | tr -d '\r')"
mkdir -p ext && touch ext/a.tmp ext/b.tmp ext/keep.txt
find ext -name '*.tmp' -exec rm {} \;
check "find -exec rm (external rm)" "keep.txt" "$(ls ext)"
touch ext/c.tmp ext/d.tmp && printf 'ext/c.tmp\next/d.tmp\n' | xargs rm
check "xargs rm (external rm)" "keep.txt" "$(ls ext)"
check "sort is POSIX sort, not Windows sort.exe" "a|b|c" "$(printf 'c\na\nb\n' | cmd.exe /d /c sort | tr -d '\r' | tr '\n' '|' | sed 's/|$//')"
check "find output uses /" "./src/a.txt" "$(find . -name a.txt)"
check "diff <(...) <(...) (named pipes)" "2" "$(diff <(printf 'a\n') <(printf 'b\n') | grep -c '^[<>]')"
check "cmp <(...) <(...)" "same" "$(cmp <(echo x) <(echo x) && echo same)"
check "diff -r --exclude (GNU diffutils)" "0" "$(mkdir -p dA dB && echo s > dA/k && echo s > dB/k && echo 1 > dA/STAMP && diff -r --exclude=STAMP dA dB >/dev/null; echo $?)"
check "OSTYPE" "msys" "$OSTYPE"
check "uname -s" "Windows_NT" "$(uname -s)"

cd / && rm -rf "$work"
echo "dist-check: $fails failure(s)"
[[ $fails -eq 0 ]]
