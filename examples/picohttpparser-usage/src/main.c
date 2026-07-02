#include <picohttpparser.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    const char *req = "GET /hello HTTP/1.1\r\nHost: cabinpkg.com\r\n\r\n";
    const char *method = NULL;
    const char *path = NULL;
    size_t method_len = 0;
    size_t path_len = 0;
    int minor_version = 0;
    struct phr_header headers[4];
    size_t num_headers = sizeof(headers) / sizeof(headers[0]);

    int ret = phr_parse_request(req, strlen(req), &method, &method_len, &path,
                                &path_len, &minor_version, headers,
                                &num_headers, 0);
    if (ret <= 0) {
        fprintf(stderr, "picohttpparser parse failed: %d\n", ret);
        return 1;
    }

    printf("picohttpparser method: %.*s\n", (int)method_len, method);
    printf("picohttpparser path: %.*s\n", (int)path_len, path);
    printf("picohttpparser headers: %d\n", (int)num_headers);
    return 0;
}
