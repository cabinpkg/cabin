// Temporary probe: `embed <file> <mode>`, reporting how it was started and
// what happens to its standard output handle to `diag-<DIAG_LABEL>.txt` in
// CABIN_OUT_DIR.  Modes: `close` (fclose(stdout)), `flush`, and
// `strict-close` (turn strict handle checks on first, then fclose).
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>
#pragma comment(lib, "advapi32.lib")

#include <io.h>
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
    char name[MAX_PATH] = "?";
    if (InterlockedIncrement(&exceptions) > 6) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if (GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                               GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           (LPCSTR)record->ExceptionAddress, &module) &&
        module) {
        GetModuleFileNameA(module, name, sizeof name);
    }
    say("exception 0x%lx at %s + 0x%llx\r\n", record->ExceptionCode, name,
        (unsigned long long)((char *)record->ExceptionAddress - (char *)module));
    return EXCEPTION_CONTINUE_SEARCH;
}

static void policy(const char *when) {
    PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY strict;
    BOOL ok;
    memset(&strict, 0, sizeof strict);
    ok = GetProcessMitigationPolicy(GetCurrentProcess(), ProcessStrictHandleCheckPolicy, &strict,
                                    sizeof strict);
    say("%s: strict handle checks query %d flags 0x%lx\r\n", when, ok, strict.Flags);
}

static void handle_state(const char *when, HANDLE handle) {
    __try {
        DWORD flags = 0;
        DWORD type;
        DWORD error;
        BOOL known;
        SetLastError(0);
        type = GetFileType(handle);
        error = GetLastError();
        known = GetHandleInformation(handle, &flags);
        say("%s: handle %p type %lu error %lu information %d flags 0x%lx\r\n", when,
            (void *)handle, type, error, known, flags);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        say("%s: handle %p raised 0x%lx\r\n", when, (void *)handle, GetExceptionCode());
    }
}

static void leaving(void) {
    say("atexit\r\n");
}

int main(int argc, char **argv) {
    const char *directory = getenv("CABIN_OUT_DIR");
    const char *label = getenv("DIAG_LABEL");
    const char *mode = argc > 2 ? argv[2] : "close";
    DWORD ids[3] = {STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE};
    HANDLE token = NULL;
    HANDLE output;
    DWORD container = 0;
    DWORD size = 0;
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
    for (index = 0; startup.lpReserved2 && index < startup.cbReserved2 && index < 40; index++) {
        say(" %02x", startup.lpReserved2[index]);
    }
    say("\r\n");
    if (OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) {
        GetTokenInformation(token, TokenIsAppContainer, &container, sizeof container, &size);
    }
    say("app container %lu\r\n", container);
    policy("start");
    if (strcmp(mode, "strict-close") == 0) {
        PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY strict;
        BOOL set;
        memset(&strict, 0, sizeof strict);
        strict.RaiseExceptionOnInvalidHandleReference = 1;
        strict.HandleExceptionsPermanentlyEnabled = 1;
        set = SetProcessMitigationPolicy(ProcessStrictHandleCheckPolicy, &strict, sizeof strict);
        say("set strict: %d error %lu\r\n", set, set ? 0 : GetLastError());
        policy("after set");
    }
    for (index = 0; index < 3; index++) {
        say("std %d descriptor handle %p\r\n", index, (void *)_get_osfhandle(index));
        handle_state("before", GetStdHandle(ids[index]));
    }
    output = GetStdHandle(STD_OUTPUT_HANDLE);

    if (argc < 2 || !(input = fopen(argv[1], "rb"))) {
        fputs("usage: embed <file> <mode>\n", stderr);
        return 1;
    }
    fputs("static const unsigned char embedded[] = {\n", stdout);
    while ((value = fgetc(input)) != EOF) {
        fprintf(stdout, "%d,", value);
    }
    fputs("0};\n", stdout);
    if (strcmp(mode, "flush") == 0) {
        result = ferror(input) || fflush(stdout) != 0;
    } else {
        result = ferror(input) || fclose(stdout) != 0;
    }
    handle_state("after", output);
    say("main returns %d\r\n", result);
    return result;
}
