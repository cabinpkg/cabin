// Temporary probe, round 2: which ways of starting its tool work for a
// confined build script on Windows.  `probe_tool.c` is the tool.
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>

#include <errno.h>
#include <io.h>
#include <process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char *tool;

static void label(const char *name) {
    _putenv_s("DIAG_LABEL", name);
}

static void create(const char *name, const char *arguments, STARTUPINFOA *startup) {
    PROCESS_INFORMATION process;
    char command[1024];
    DWORD code = 0;
    label(name);
    strcpy(command, arguments);
    if (!CreateProcessA(tool, command, NULL, NULL, TRUE, 0, NULL, NULL, startup, &process)) {
        fprintf(stderr, "%s: CreateProcess error %lu\n", name, GetLastError());
        return;
    }
    WaitForSingleObject(process.hProcess, INFINITE);
    GetExitCodeProcess(process.hProcess, &code);
    fprintf(stderr, "%s: exit code 0x%lx\n", name, code);
    CloseHandle(process.hProcess);
    CloseHandle(process.hThread);
}

static void spawned(const char *name, intptr_t code) {
    int error = errno;
    fprintf(stderr, "%s: 0x%llx errno %d\n", name, (unsigned long long)code, error);
}

int main(void) {
    const char *out_dir = getenv("CABIN_OUT_DIR");
    const char *system_root = getenv("SystemRoot");
    const char *shell = getenv("COMSPEC");
    STARTUPINFOA plain = {sizeof plain};
    STARTUPINFOA reserved = {sizeof reserved};
    STARTUPINFOA empty = {sizeof empty};
    STARTUPINFOA redirected = {sizeof redirected};
    SECURITY_ATTRIBUTES inheritable = {sizeof inheritable, NULL, TRUE};
    unsigned char block[4 + 3 + 3 * sizeof(intptr_t)];
    unsigned char none[4] = {0};
    char path[1024];
    char command[2048];
    LARGE_INTEGER size;
    int count = 3;
    int descriptor;
    int status;
    int error;

    tool = getenv("CABIN_BIN_EMBED");
    fprintf(stderr, "COMSPEC: %s\n", shell ? shell : "(unset)");
    for (descriptor = 0; descriptor < 3; descriptor++) {
        fprintf(stderr, "fd %d: handle %p tty %d\n", descriptor,
                (void *)_get_osfhandle(descriptor), _isatty(descriptor));
    }

    create("create-plain", "embed assets/banner.txt", &plain);

    label("spawn-close");
    errno = 0;
    spawned("spawn-close", _spawnl(_P_WAIT, tool, "embed", "assets/banner.txt", NULL));
    label("spawn-noclose");
    errno = 0;
    spawned("spawn-noclose",
            _spawnl(_P_WAIT, tool, "embed", "assets/banner.txt", "noclose", NULL));
    snprintf(path, sizeof path, "%s\\spawned.h", out_dir);
    label("spawn-out");
    errno = 0;
    spawned("spawn-out", _spawnl(_P_WAIT, tool, "embed", "assets/banner.txt", "out", path, NULL));

    snprintf(path, sizeof path, "%s\\System32\\cmd.exe", system_root ? system_root : "C:\\Windows");
    errno = 0;
    spawned("spawn-cmd", _spawnl(_P_WAIT, path, "cmd", "/c", "exit", "7", NULL));

    label("system");
    snprintf(command, sizeof command, "%s assets/banner.txt out %s\\system.h", tool, out_dir);
    errno = 0;
    status = system(command);
    error = errno;
    fprintf(stderr, "system: %d errno %d\n", status, error);

    // What the C runtime passes for its three open descriptors.
    memcpy(block, &count, sizeof count);
    for (descriptor = 0; descriptor < 3; descriptor++) {
        intptr_t handle = _get_osfhandle(descriptor);
        block[4 + descriptor] = (unsigned char)(_isatty(descriptor) ? 0xC1 : 0x81);
        memcpy(block + 7 + descriptor * sizeof handle, &handle, sizeof handle);
    }
    reserved.cbReserved2 = (WORD)sizeof block;
    reserved.lpReserved2 = block;
    create("create-reserved", "embed assets/banner.txt", &reserved);
    empty.cbReserved2 = (WORD)sizeof none;
    empty.lpReserved2 = none;
    create("create-reserved-empty", "embed assets/banner.txt", &empty);

    snprintf(path, sizeof path, "%s\\banner.h", out_dir);
    redirected.dwFlags = STARTF_USESTDHANDLES;
    redirected.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    redirected.hStdError = GetStdHandle(STD_ERROR_HANDLE);
    redirected.hStdOutput = CreateFileA(path, GENERIC_WRITE, 0, &inheritable, CREATE_ALWAYS,
                                        FILE_ATTRIBUTE_NORMAL, NULL);
    if (redirected.hStdOutput == INVALID_HANDLE_VALUE) {
        fprintf(stderr, "create-redirected: open error %lu\n", GetLastError());
    } else {
        create("create-redirected", "embed assets/banner.txt", &redirected);
        size.QuadPart = 0;
        GetFileSizeEx(redirected.hStdOutput, &size);
        fprintf(stderr, "create-redirected: %lld bytes\n", (long long)size.QuadPart);
        CloseHandle(redirected.hStdOutput);
    }
    return 1;
}
