// Temporary probe: a build script starting `probe_tool.c` the C runtime's
// way (`_spawnl`) and Windows' own (`CreateProcessA`), printing how each
// ended.  With PROBE_STRICT set it also runs the tool's `strict-` modes.
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>

#include <errno.h>
#include <process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char *tool;

static void spawn(const char *mode) {
    char name[64];
    intptr_t code;
    int error;
    snprintf(name, sizeof name, "spawn-%s", mode);
    _putenv_s("DIAG_LABEL", name);
    errno = 0;
    code = _spawnl(_P_WAIT, tool, "embed", "assets/banner.txt", mode, NULL);
    error = errno;
    fprintf(stderr, "%s: 0x%llx errno %d\n", name, (unsigned long long)code, error);
}

static void create(const char *mode) {
    STARTUPINFOA startup = {sizeof startup};
    PROCESS_INFORMATION process;
    char name[64];
    char command[256];
    DWORD code = 0;
    snprintf(name, sizeof name, "create-%s", mode);
    _putenv_s("DIAG_LABEL", name);
    snprintf(command, sizeof command, "embed assets/banner.txt %s", mode);
    if (!CreateProcessA(tool, command, NULL, NULL, TRUE, 0, NULL, NULL, &startup, &process)) {
        fprintf(stderr, "%s: CreateProcess error %lu\n", name, GetLastError());
        return;
    }
    WaitForSingleObject(process.hProcess, INFINITE);
    GetExitCodeProcess(process.hProcess, &code);
    fprintf(stderr, "%s: 0x%lx\n", name, code);
    CloseHandle(process.hProcess);
    CloseHandle(process.hThread);
}

int main(void) {
    PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY strict;
    BOOL ok;
    tool = getenv("CABIN_BIN_EMBED");
    memset(&strict, 0, sizeof strict);
    ok = GetProcessMitigationPolicy(GetCurrentProcess(), ProcessStrictHandleCheckPolicy, &strict,
                                    sizeof strict);
    fprintf(stderr, "script: strict handle checks query %d flags 0x%lx\n", ok, strict.Flags);
    spawn("close");
    spawn("flush");
    spawn("close-in");
    spawn("close-err");
    spawn("preload-close");
    create("close");
    if (getenv("PROBE_STRICT")) {
        spawn("strict-close");
        spawn("strict-flush");
        spawn("strict-preload-close");
        create("strict-close");
    }
    return 1;
}
