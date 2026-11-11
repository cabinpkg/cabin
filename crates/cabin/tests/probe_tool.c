// Temporary probe: `embed`, reporting how it was started and any exception
// to `diag-<DIAG_LABEL>.txt` in CABIN_OUT_DIR.
//
// `embed <file>` writes to standard output and closes it, `embed <file>
// noclose` only flushes it, `embed <file> out <path>` writes to `<path>`.
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

static void where(const char *label, void *address) {
    HMODULE module = NULL;
    char name[MAX_PATH] = "?";
    if (GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                               GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           (LPCSTR)address, &module) &&
        module) {
        GetModuleFileNameA(module, name, sizeof name);
    }
    say("%s %s + 0x%llx\r\n", label, name,
        (unsigned long long)((char *)address - (char *)module));
}

static LONG CALLBACK crashed(EXCEPTION_POINTERS *info) {
    EXCEPTION_RECORD *record = info->ExceptionRecord;
    void *frames[32];
    USHORT count;
    USHORT frame;
    DWORD parameter;
    if (InterlockedIncrement(&exceptions) > 4) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    say("exception 0x%lx flags 0x%lx parameters", record->ExceptionCode, record->ExceptionFlags);
    for (parameter = 0; parameter < record->NumberParameters && parameter < 4; parameter++) {
        say(" 0x%llx", (unsigned long long)record->ExceptionInformation[parameter]);
    }
    say("\r\n");
    where("at", record->ExceptionAddress);
    count = CaptureStackBackTrace(0, 32, frames, NULL);
    for (frame = 0; frame < count; frame++) {
        where("frame", frames[frame]);
    }
    return EXCEPTION_CONTINUE_SEARCH;
}

static void leaving(void) {
    say("atexit\r\n");
}

int main(int argc, char **argv) {
    const char *directory = getenv("CABIN_OUT_DIR");
    const char *label = getenv("DIAG_LABEL");
    DWORD ids[3] = {STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE};
    char path[MAX_PATH];
    STARTUPINFOA startup;
    FILE *input;
    FILE *output = stdout;
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
    say("pid %lu arguments %d flags 0x%lx reserved2 %u:", GetCurrentProcessId(), argc,
        startup.dwFlags, (unsigned)startup.cbReserved2);
    for (index = 0; startup.lpReserved2 && index < startup.cbReserved2 && index < 64; index++) {
        say(" %02x", startup.lpReserved2[index]);
    }
    say("\r\n");
    for (index = 0; index < 3; index++) {
        HANDLE handle = GetStdHandle(ids[index]);
        say("std %d handle %p type %lu\r\n", index, (void *)handle, GetFileType(handle));
    }

    if (argc < 2 || !(input = fopen(argv[1], "rb"))) {
        fputs("usage: embed <file>\n", stderr);
        return 1;
    }
    if (argc == 4 && !(output = fopen(argv[3], "w"))) {
        perror(argv[3]);
        return 1;
    }
    fputs("static const unsigned char embedded[] = {\n", output);
    while ((value = fgetc(input)) != EOF) {
        fprintf(output, "%d,", value);
    }
    fputs("0};\n", output);
    if (argc == 3) {
        result = ferror(input) || fflush(output) != 0;
    } else {
        result = ferror(input) || fclose(output) != 0;
    }
    say("main returns %d\r\n", result);
    return result;
}
