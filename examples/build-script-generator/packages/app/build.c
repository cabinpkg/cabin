// The package's build script.  It runs `embed`, the executable of the
// package's build dependency, to turn `assets/banner.txt` into a header.
// Cabin gives the script the executable's path in CABIN_BIN_EMBED: the
// script runs that file and never looks a program up on PATH.

// Declares `fork`, `execl`, and `waitpid` under `-std=c11`.
#define _POSIX_C_SOURCE 200809L
// Quiets MSVC's advice to use its own `_s` functions.
#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>

#ifdef _WIN32
#include <process.h>
#else
#include <sys/wait.h>
#include <unistd.h>
#endif

// Runs `tool` with one argument and waits for it, without a shell.
static int run(const char *tool, const char *argument) {
#ifdef _WIN32
    return (int)_spawnl(_P_WAIT, tool, "embed", argument, NULL);
#else
    int status = 0;
    pid_t child = fork();
    if (child == 0) {
        execl(tool, tool, argument, (char *)NULL);
        _exit(127);
    }
    if (child < 0 || waitpid(child, &status, 0) != child || !WIFEXITED(status)) {
        return -1;
    }
    return WEXITSTATUS(status);
#endif
}

int main(void) {
    const char *out_dir = getenv("CABIN_OUT_DIR");
    const char *manifest_dir = getenv("CABIN_MANIFEST_DIR");
    char header[4096];
    snprintf(header, sizeof header, "%s/banner.h", out_dir);

    // `embed` writes to standard output, which it takes over from this
    // script: send it to the header.  The script starts in the package's
    // root, so the path of the file to embed is relative to it.
    if (!freopen(header, "w", stdout)) {
        perror(header);
        return 1;
    }
    if (run(getenv("CABIN_BIN_EMBED"), "assets/banner.txt") != 0 || fclose(stdout) != 0) {
        fputs("embed failed\n", stderr);
        return 1;
    }

    FILE *result = fopen(getenv("CABIN_RESULT_FILE"), "w");
    if (!result) {
        return 1;
    }
    fprintf(result, "include-dir app %s\n", out_dir);
    fprintf(result, "rerun-if-changed %s/build.c\n", manifest_dir);
    fprintf(result, "rerun-if-changed %s/assets/banner.txt\n", manifest_dir);
    return fclose(result) != 0;
}
