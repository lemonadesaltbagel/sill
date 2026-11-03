/* Linked into sill's GNU tools for Windows (scripts/build-gnu-tools.sh), with shim.c.
 *
 * Directory listings that don't fail on some file names. The tools run in the UTF-8 code
 * page (utf8.manifest), and read directories through the C runtime's _findfirst64 and
 * _findnext64 (mingw's readdir, make) or FindFirstFileA (gnulib). Those call Windows'
 * ANSI find functions, which also convert each file's 8.3 short name into a 14-byte
 * field; a short name longer than that in UTF-8 (`文件Ü~1.TXT` for `文件 ü.txt` is 15
 * bytes) fails the call with "Invalid argument", and the listing stops there: `make`,
 * `grep -r` and `diff -r` failed in any folder holding such a file.
 *
 * The replacements below list with the wide functions, without short names, and give
 * names in the active code page (UTF-8). The C runtime's are defined under their own
 * names and as the import entries (`__imp_...`) that dllimport calls go through, so the
 * import library's are never linked; FindFirstFileA and FindNextFileA (declared dllimport
 * by windows.h) only as import entries. msvcrt's _findclose and kernel32's FindClose close
 * their handles as before. */
#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <time.h>
#include <windows.h>

struct sill_finddata64 { /* struct __finddata64_t */
    unsigned attrib;
    __time64_t time_create;
    __time64_t time_access;
    __time64_t time_write;
    __int64 size;
    char name[260];
};

/* The first or next entry whose name fits a 260-byte ANSI field (as the ANSI functions
 * would return them; longer ones they couldn't return at all). */
static BOOL next_fitting(HANDLE h, WIN32_FIND_DATAW *w, char name[260], BOOL first)
{
    for (;;) {
        if (!first && !FindNextFileW(h, w))
            return FALSE;
        first = FALSE;
        if (WideCharToMultiByte(CP_ACP, 0, w->cFileName, -1, name, 260, NULL, NULL) > 0)
            return TRUE;
    }
}

static HANDLE first_entry(const char *spec, WIN32_FIND_DATAW *w, char name[260])
{
    int n = MultiByteToWideChar(CP_ACP, 0, spec, -1, NULL, 0);
    if (n <= 0) {
        SetLastError(ERROR_INVALID_NAME);
        return INVALID_HANDLE_VALUE;
    }
    WCHAR *wspec = HeapAlloc(GetProcessHeap(), 0, (SIZE_T)n * sizeof(WCHAR));
    if (!wspec) {
        SetLastError(ERROR_NOT_ENOUGH_MEMORY);
        return INVALID_HANDLE_VALUE;
    }
    MultiByteToWideChar(CP_ACP, 0, spec, -1, wspec, n);
    HANDLE h = FindFirstFileExW(wspec, FindExInfoBasic, w, FindExSearchNameMatch, NULL, 0);
    HeapFree(GetProcessHeap(), 0, wspec);
    if (h != INVALID_HANDLE_VALUE && !next_fitting(h, w, name, TRUE)) {
        FindClose(h);
        SetLastError(ERROR_FILE_NOT_FOUND);
        return INVALID_HANDLE_VALUE;
    }
    return h;
}

static void to_ansi(const WIN32_FIND_DATAW *w, const char name[260], WIN32_FIND_DATAA *a)
{
    a->dwFileAttributes = w->dwFileAttributes;
    a->ftCreationTime = w->ftCreationTime;
    a->ftLastAccessTime = w->ftLastAccessTime;
    a->ftLastWriteTime = w->ftLastWriteTime;
    a->nFileSizeHigh = w->nFileSizeHigh;
    a->nFileSizeLow = w->nFileSizeLow;
    a->dwReserved0 = w->dwReserved0;
    a->dwReserved1 = w->dwReserved1;
    memcpy(a->cFileName, name, 260);
    a->cAlternateFileName[0] = '\0';
}

static HANDLE WINAPI sill_FindFirstFileA(LPCSTR spec, LPWIN32_FIND_DATAA a)
{
    WIN32_FIND_DATAW w;
    char name[260];
    HANDLE h = first_entry(spec, &w, name);
    if (h != INVALID_HANDLE_VALUE)
        to_ansi(&w, name, a);
    return h;
}

static BOOL WINAPI sill_FindNextFileA(HANDLE h, LPWIN32_FIND_DATAA a)
{
    WIN32_FIND_DATAW w;
    char name[260];
    if (!next_fitting(h, &w, name, FALSE))
        return FALSE;
    to_ansi(&w, name, a);
    return TRUE;
}

static __time64_t unix_time(FILETIME ft)
{
    ULONGLONG t = ((ULONGLONG)ft.dwHighDateTime << 32) | ft.dwLowDateTime;
    if (t == 0)
        return -1;
    return (__time64_t)((t - 116444736000000000ULL) / 10000000ULL);
}

static void to_finddata(const WIN32_FIND_DATAW *w, const char name[260], struct sill_finddata64 *f)
{
    f->attrib = w->dwFileAttributes == FILE_ATTRIBUTE_NORMAL ? 0 : w->dwFileAttributes;
    f->time_create = unix_time(w->ftCreationTime);
    f->time_access = unix_time(w->ftLastAccessTime);
    f->time_write = unix_time(w->ftLastWriteTime);
    f->size = ((__int64)w->nFileSizeHigh << 32) | w->nFileSizeLow;
    memcpy(f->name, name, 260);
}

static void set_errno_from_last_error(void)
{
    switch (GetLastError()) {
    case ERROR_FILE_NOT_FOUND:
    case ERROR_PATH_NOT_FOUND:
    case ERROR_NO_MORE_FILES:
        errno = ENOENT;
        break;
    case ERROR_NOT_ENOUGH_MEMORY:
        errno = ENOMEM;
        break;
    case ERROR_ACCESS_DENIED:
        errno = EACCES;
        break;
    default:
        errno = EINVAL;
    }
}

intptr_t __cdecl _findfirst64(const char *spec, struct sill_finddata64 *f)
{
    WIN32_FIND_DATAW w;
    char name[260];
    HANDLE h = first_entry(spec, &w, name);
    if (h == INVALID_HANDLE_VALUE) {
        set_errno_from_last_error();
        return -1;
    }
    to_finddata(&w, name, f);
    return (intptr_t)h;
}

int __cdecl _findnext64(intptr_t h, struct sill_finddata64 *f)
{
    WIN32_FIND_DATAW w;
    char name[260];
    if (!next_fitting((HANDLE)h, &w, name, FALSE)) {
        set_errno_from_last_error();
        return -1;
    }
    to_finddata(&w, name, f);
    return 0;
}

/* The same for the other layouts of the C runtime's find data (32-bit size, or 32-bit
 * times as well), which a build picks by its file-offset and time sizes. */
struct sill_finddata64i32 {
    unsigned attrib;
    __time64_t time_create;
    __time64_t time_access;
    __time64_t time_write;
    unsigned long size;
    char name[260];
};
struct sill_finddata32 {
    unsigned attrib;
    __time32_t time_create;
    __time32_t time_access;
    __time32_t time_write;
    unsigned long size;
    char name[260];
};

static void to_finddata64i32(const struct sill_finddata64 *f, struct sill_finddata64i32 *g)
{
    g->attrib = f->attrib;
    g->time_create = f->time_create;
    g->time_access = f->time_access;
    g->time_write = f->time_write;
    g->size = (unsigned long)f->size;
    memcpy(g->name, f->name, 260);
}

static void to_finddata32(const struct sill_finddata64 *f, struct sill_finddata32 *g)
{
    g->attrib = f->attrib;
    g->time_create = (__time32_t)f->time_create;
    g->time_access = (__time32_t)f->time_access;
    g->time_write = (__time32_t)f->time_write;
    g->size = (unsigned long)f->size;
    memcpy(g->name, f->name, 260);
}

intptr_t __cdecl _findfirst64i32(const char *spec, struct sill_finddata64i32 *g)
{
    struct sill_finddata64 f;
    intptr_t h = _findfirst64(spec, &f);
    if (h != -1)
        to_finddata64i32(&f, g);
    return h;
}

int __cdecl _findnext64i32(intptr_t h, struct sill_finddata64i32 *g)
{
    struct sill_finddata64 f;
    int r = _findnext64(h, &f);
    if (r == 0)
        to_finddata64i32(&f, g);
    return r;
}

intptr_t __cdecl _findfirst32(const char *spec, struct sill_finddata32 *g)
{
    struct sill_finddata64 f;
    intptr_t h = _findfirst64(spec, &f);
    if (h != -1)
        to_finddata32(&f, g);
    return h;
}

int __cdecl _findnext32(intptr_t h, struct sill_finddata32 *g)
{
    struct sill_finddata64 f;
    int r = _findnext64(h, &f);
    if (r == 0)
        to_finddata32(&f, g);
    return r;
}

/* The import entries the tools' calls go through. */
HANDLE (WINAPI *__imp_FindFirstFileA)(LPCSTR, LPWIN32_FIND_DATAA) = sill_FindFirstFileA;
BOOL (WINAPI *__imp_FindNextFileA)(HANDLE, LPWIN32_FIND_DATAA) = sill_FindNextFileA;
intptr_t (__cdecl *__imp__findfirst64)(const char *, struct sill_finddata64 *) = _findfirst64;
int (__cdecl *__imp__findnext64)(intptr_t, struct sill_finddata64 *) = _findnext64;
intptr_t (__cdecl *__imp__findfirst64i32)(const char *, struct sill_finddata64i32 *) = _findfirst64i32;
int (__cdecl *__imp__findnext64i32)(intptr_t, struct sill_finddata64i32 *) = _findnext64i32;
intptr_t (__cdecl *__imp__findfirst32)(const char *, struct sill_finddata32 *) = _findfirst32;
int (__cdecl *__imp__findnext32)(intptr_t, struct sill_finddata32 *) = _findnext32;
