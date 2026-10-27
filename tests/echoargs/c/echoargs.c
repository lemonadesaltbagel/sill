/* Print argv[1..] as a JSON array of strings, UTF-8, one line.
 * Built with MSVC (cl /utf-8), so the parser under test is the real UCRT
 * startup code (wmain gets the UCRT's split of GetCommandLineW()).
 * Wildcard expansion is off because setargv.obj is not linked. */
#include <windows.h>
#include <stdio.h>
#include <io.h>
#include <fcntl.h>

static void put_json_string(const wchar_t *s)
{
    int n = WideCharToMultiByte(CP_UTF8, 0, s, -1, NULL, 0, NULL, NULL);
    char *buf = (char *)HeapAlloc(GetProcessHeap(), 0, n > 0 ? n : 1);
    if (n <= 0 || !buf) {
        fputs("\"<conversion error>\"", stdout);
        return;
    }
    WideCharToMultiByte(CP_UTF8, 0, s, -1, buf, n, NULL, NULL);
    putchar('"');
    for (const unsigned char *p = (const unsigned char *)buf; *p; p++) {
        if (*p == '"' || *p == '\\')
            printf("\\%c", *p);
        else if (*p < 0x20)
            printf("\\u%04x", *p);
        else
            putchar(*p);
    }
    putchar('"');
    HeapFree(GetProcessHeap(), 0, buf);
}

int wmain(int argc, wchar_t **argv)
{
    _setmode(_fileno(stdout), _O_BINARY);
    putchar('[');
    for (int i = 1; i < argc; i++) {
        if (i > 1)
            putchar(',');
        put_json_string(argv[i]);
    }
    puts("]");
    return 0;
}
