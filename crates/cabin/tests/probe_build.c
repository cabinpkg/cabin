// Temporary probe: how a confined build script can start its tool on Windows.
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>

#include <direct.h>
#include <errno.h>
#include <process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void start(const char *label, const char *program, const char *arguments, BOOL inherit,
                  DWORD flags) {
    STARTUPINFOA startup = {sizeof startup};
    PROCESS_INFORMATION process;
    char command[256];
    DWORD code = 0;
    strcpy(command, arguments);
    if (!CreateProcessA(program, command, NULL, NULL, inherit, flags, NULL, NULL, &startup,
                        &process)) {
        fprintf(stderr, "%s: CreateProcess error %lu\n", label, GetLastError());
        return;
    }
    WaitForSingleObject(process.hProcess, INFINITE);
    GetExitCodeProcess(process.hProcess, &code);
    fprintf(stderr, "%s: exit code 0x%lx\n", label, code);
    CloseHandle(process.hProcess);
    CloseHandle(process.hThread);
}

static void try_open(const char *label, const char *path, DWORD access, DWORD flags) {
    HANDLE handle =
        CreateFileA(path, access, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, NULL,
                    OPEN_EXISTING, flags, NULL);
    if (handle == INVALID_HANDLE_VALUE) {
        fprintf(stderr, "%s: error %lu\n", label, GetLastError());
    } else {
        fprintf(stderr, "%s: ok\n", label);
        CloseHandle(handle);
    }
}

static void spawn(const char *label, const char *tool) {
    intptr_t spawned;
    int error;
    unsigned long os_error;
    errno = 0;
    _doserrno = 0;
    spawned = _spawnl(_P_WAIT, tool, "embed", "assets/banner.txt", NULL);
    error = errno;
    os_error = _doserrno;
    fprintf(stderr, "%s: %lld errno %d doserrno %lu\n", label, (long long)spawned, error,
            os_error);
}

int main(void) {
    const char *tool = getenv("CABIN_BIN_EMBED");
    const char *out_dir = getenv("CABIN_OUT_DIR");
    const char *system_root = getenv("SystemRoot");
    char path[4096];
    char cwd[4096];
    char *separator;
    DWORD attributes;
    DWORD error;
    int closed;

    fprintf(stderr, "tool: %s\n", tool);
    fprintf(stderr, "cwd: %s\n", _getcwd(cwd, sizeof cwd));
    fprintf(stderr, "console window: %p\n", (void *)GetConsoleWindow());
    fprintf(stderr, "console code page: %u\n", GetConsoleCP());
    SetLastError(0);
    attributes = GetFileAttributesA(tool);
    error = GetLastError();
    fprintf(stderr, "attributes: 0x%lx error %lu\n", attributes, error);

    try_open("open read", tool, GENERIC_READ, 0);
    try_open("open execute", tool,
             FILE_EXECUTE | FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE, 0);
    strcpy(path, tool);
    separator = strrchr(path, '\\');
    if (separator) {
        *separator = 0;
    }
    try_open("dir traverse", path, FILE_TRAVERSE | SYNCHRONIZE, FILE_FLAG_BACKUP_SEMANTICS);
    try_open("dir list", path, FILE_LIST_DIRECTORY | SYNCHRONIZE, FILE_FLAG_BACKUP_SEMANTICS);
    try_open("dir attributes", path, FILE_READ_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS);

    snprintf(path, sizeof path, "%s\\System32\\cmd.exe", system_root ? system_root : "C:\\Windows");
    start("cmd", path, "cmd /c exit 7", FALSE, 0);

    start("inherit", tool, "embed assets/banner.txt", TRUE, 0);
    start("no inherit", tool, "embed assets/banner.txt", FALSE, 0);
    start("no window", tool, "embed assets/banner.txt", FALSE, CREATE_NO_WINDOW);
    start("detached", tool, "embed assets/banner.txt", FALSE, DETACHED_PROCESS);
    spawn("_spawnl", tool);

    snprintf(path, sizeof path, "%s\\embed.exe", out_dir);
    if (!CopyFileA(tool, path, FALSE)) {
        fprintf(stderr, "copy: error %lu\n", GetLastError());
    } else {
        start("copied inherit", path, "embed assets/banner.txt", TRUE, 0);
        start("copied no inherit", path, "embed assets/banner.txt", FALSE, 0);
        spawn("copied _spawnl", path);
    }

    // The example's own steps.
    snprintf(path, sizeof path, "%s/banner.h", out_dir);
    if (!freopen(path, "w", stdout)) {
        perror("freopen");
        return 1;
    }
    spawn("redirected _spawnl", tool);
    errno = 0;
    closed = fclose(stdout);
    fprintf(stderr, "fclose: %d errno %d\n", closed, errno);
    return 1;
}
