// Temporary probe: `embed <file> <mode>`: the example's tool plus a report
// of how it was started and of every exception, written to
// `diag-<DIAG_LABEL>.txt` in CABIN_OUT_DIR.  It links only kernel32 and the
// C runtime, and never touches a handle it closed.  Modes: `close`
// (fclose(stdout)), `flush`, `close-in` (fclose(stdin)), `close-err`
// (fclose(stderr)), `preload-close` (load msvcrt.dll, then fclose(stdout));
// a `strict-` prefix turns strict handle checks on first.
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static HANDLE report = INVALID_HANDLE_VALUE;
static LONG exceptions;

static void say(const char *format, ...) {
    char line[1024];
    DWORD written;
    int length;
    va_list arguments;
    va_start(arguments, format);
    length = vsnprintf(line, sizeof line, format, arguments);
    va_end(arguments);
    if (length >= (int)sizeof line) {
        length = (int)sizeof line - 1;
    }
    if (length > 0 && report != INVALID_HANDLE_VALUE) {
        WriteFile(report, line, (DWORD)length, &written, NULL);
    }
}

static LONG CALLBACK crashed(EXCEPTION_POINTERS *info) {
    EXCEPTION_RECORD *record = info->ExceptionRecord;
    HMODULE module = NULL;
    char name[MAX_PATH] = "(no module)";
    if (InterlockedIncrement(&exceptions) > 6) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if (GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                               GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           (LPCSTR)record->ExceptionAddress, &module) &&
        module) {
        GetModuleFileNameA(module, name, sizeof name);
    }
    say("exception 0x%lx at %p = %s + 0x%llx, parameters %lu: 0x%llx 0x%llx, msvcrt %p\r\n",
        record->ExceptionCode, record->ExceptionAddress, name,
        (unsigned long long)((char *)record->ExceptionAddress - (char *)module),
        record->NumberParameters,
        (unsigned long long)(record->NumberParameters > 0 ? record->ExceptionInformation[0] : 0),
        (unsigned long long)(record->NumberParameters > 1 ? record->ExceptionInformation[1] : 0),
        (void *)GetModuleHandleA("msvcrt.dll"));
    return EXCEPTION_CONTINUE_SEARCH;
}

static void policy(const char *when) {
    PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY strict;
    BOOL ok;
    memset(&strict, 0, sizeof strict);
    ok = GetProcessMitigationPolicy(GetCurrentProcess(), ProcessStrictHandleCheckPolicy, &strict,
                                    sizeof strict);
    say("%s: strict handle checks query %d flags 0x%lx, msvcrt %p, kernel.appcore %p\r\n", when, ok,
        strict.Flags, (void *)GetModuleHandleA("msvcrt.dll"),
        (void *)GetModuleHandleA("kernel.appcore.dll"));
}

static void leaving(void) {
    policy("atexit");
}

int main(int argc, char **argv) {
    const char *directory = getenv("CABIN_OUT_DIR");
    const char *label = getenv("DIAG_LABEL");
    const char *mode = argc > 2 ? argv[2] : "close";
    char path[MAX_PATH];
    STARTUPINFOA startup;
    FILE *input;
    int value;
    int result;
    int index;

    snprintf(path, sizeof path, "%s\\diag-%s.txt", directory ? directory : ".",
             label ? label : "none");
    report = CreateFileA(path, GENERIC_WRITE, FILE_SHARE_READ, NULL, CREATE_ALWAYS,
                         FILE_ATTRIBUTE_NORMAL, NULL);
    AddVectoredExceptionHandler(1, crashed);
    atexit(leaving);
    GetStartupInfoA(&startup);
    say("mode %s flags 0x%lx reserved2 %u:", mode, startup.dwFlags, (unsigned)startup.cbReserved2);
    for (index = 0; startup.lpReserved2 && index < (int)startup.cbReserved2 && index < 12; index++) {
        say(" %02x", startup.lpReserved2[index]);
    }
    say("\r\n");
    policy("start");
    if (strncmp(mode, "strict-", 7) == 0) {
        PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY strict;
        BOOL set;
        memset(&strict, 0, sizeof strict);
        strict.RaiseExceptionOnInvalidHandleReference = 1;
        strict.HandleExceptionsPermanentlyEnabled = 1;
        set = SetProcessMitigationPolicy(ProcessStrictHandleCheckPolicy, &strict, sizeof strict);
        say("set strict: %d error %lu\r\n", set, set ? 0 : GetLastError());
        policy("after set");
        mode += 7;
    }
    if (strcmp(mode, "preload-close") == 0) {
        say("preload msvcrt: %p\r\n", (void *)LoadLibraryA("msvcrt.dll"));
    }

    if (argc < 2 || !(input = fopen(argv[1], "rb"))) {
        fputs("usage: embed <file> <mode>\n", stderr);
        return 1;
    }
    fputs("static const unsigned char embedded[] = {\n", stdout);
    while ((value = fgetc(input)) != EOF) {
        fprintf(stdout, "%d,", value);
    }
    fputs("0};\n", stdout);
    result = ferror(input) || fflush(stdout) != 0;
    if (strcmp(mode, "close") == 0 || strcmp(mode, "preload-close") == 0) {
        result = result || fclose(stdout) != 0;
    } else if (strcmp(mode, "close-in") == 0) {
        result = result || fclose(stdin) != 0;
    } else if (strcmp(mode, "close-err") == 0) {
        result = result || fclose(stderr) != 0;
    }
    say("main returns %d\r\n", result);
    return result;
}
