/* Linked into sill's GNU tools for Windows (scripts/build-gnu-tools.sh).
 *
 * - No C-runtime wildcard expansion: the shell has already globbed, and mingw's
 *   expansion would turn a quoted '*.txt' into file names.
 * - Binary mode for files and standard streams, as on Linux: in mingw's default text
 *   mode, sed sees no CR in CRLF files and writes LF as CRLF, so `sed -i` would turn a
 *   Unix file into a Windows one (and grep would hide CRs). Bytes in, bytes out. */
#include <fcntl.h>
#include <io.h>
#include <stdio.h>
#include <stdlib.h>

int _dowildcard = 0;

/* mingw-w64's startup code applies this before main, for msvcrt and UCRT alike (as
 * busybox-w32 does in win32/mingw.c). */
#undef _fmode
int _fmode = _O_BINARY;

__attribute__((constructor)) static void sill_binary_mode(void)
{
    _setmode(_fileno(stdin), _O_BINARY);
    _setmode(_fileno(stdout), _O_BINARY);
    _setmode(_fileno(stderr), _O_BINARY);
}

/* Arguments in a file. A Windows command line holds at most 32,767 characters, so
 * `grep -l x data/*` over a few thousand files couldn't start (Linux allows ~2 MB). When
 * the command line would be too long, sill (brush-core push_args) runs the tool as
 * `TOOL --sill-args-file FILE`, with the arguments NUL-separated in FILE (UTF-8, the
 * tools' code page through utf8.manifest). Linked with -Wl,--wrap=main, so the C
 * runtime's call to main lands here first. */
#include <string.h>

extern int __real_main(int argc, char **argv, char **envp);

int __wrap_main(int argc, char **argv, char **envp)
{
    if (argc == 3 && strcmp(argv[1], "--sill-args-file") == 0) {
        FILE *f = fopen(argv[2], "rb");
        if (f) {
            fseek(f, 0, SEEK_END);
            long n = ftell(f);
            fseek(f, 0, SEEK_SET);
            char *buf = malloc((size_t)n + 1);
            size_t got = buf ? fread(buf, 1, (size_t)n, f) : 0;
            fclose(f);
            remove(argv[2]);
            if (buf) {
                buf[got] = '\0';
                int count = 1;
                for (size_t i = 0; i < got; i++)
                    if (buf[i] == '\0')
                        count++;
                char **nv = malloc(sizeof(char *) * ((size_t)count + 2));
                if (nv) {
                    int k = 0;
                    nv[k++] = argv[0];
                    if (got > 0) {
                        char *p = buf, *end = buf + got;
                        while (p <= end) {
                            nv[k++] = p;
                            p += strlen(p) + 1;
                        }
                    }
                    nv[k] = NULL;
                    return __real_main(k, nv, envp);
                }
            }
        }
    }
    return __real_main(argc, argv, envp);
}
