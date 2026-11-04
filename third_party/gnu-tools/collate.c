/* Linked into sill's GNU tools for Windows (scripts/build-gnu-tools.sh), with shim.c.
 *
 * String comparison in byte order. The tools' strings are UTF-8 (utf8.manifest), but the
 * C runtime's strcoll and _stricoll compare them in the locale's code page, which
 * setlocale(LC_ALL, "") makes the system's (GBK on a Chinese system); UTF-8 that isn't
 * valid GBK (`文件 €.txt`) failed the comparison, and `diff -r` printed "cannot compare
 * file names ... Invalid argument" for such a folder. UTF-8 in byte order is in code
 * point order, as in the C locale.
 *
 * Defined under the runtime's names and as the import entries dllimport calls go through
 * (see findfile.c), without its headers, which declare them dllimport. */

int __cdecl strcoll(const char *a, const char *b)
{
    const unsigned char *x = (const unsigned char *)a, *y = (const unsigned char *)b;
    while (*x && *x == *y) {
        x++;
        y++;
    }
    return (*x > *y) - (*x < *y);
}

/* Case-insensitive in ASCII (make compares file names with it). */
int __cdecl _stricoll(const char *a, const char *b)
{
    const unsigned char *x = (const unsigned char *)a, *y = (const unsigned char *)b;
    for (;;) {
        unsigned char c = *x >= 'A' && *x <= 'Z' ? *x + 32 : *x;
        unsigned char d = *y >= 'A' && *y <= 'Z' ? *y + 32 : *y;
        if (c != d || !c)
            return (c > d) - (c < d);
        x++;
        y++;
    }
}

int (__cdecl *__imp_strcoll)(const char *, const char *) = strcoll;
int (__cdecl *__imp__stricoll)(const char *, const char *) = _stricoll;
