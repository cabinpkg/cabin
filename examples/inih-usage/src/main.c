#include <ini.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct config {
    char name[32];
    int port;
};

static int handler(void *user, const char *section, const char *name,
                   const char *value) {
    struct config *cfg = user;
    if (strcmp(section, "server") == 0 && strcmp(name, "name") == 0) {
        snprintf(cfg->name, sizeof(cfg->name), "%s", value);
    } else if (strcmp(section, "server") == 0 && strcmp(name, "port") == 0) {
        cfg->port = atoi(value);
    }
    return 1;
}

int main(void) {
    const char *ini = "[server]\nname = Cabin\nport = 8080\n";
    struct config cfg;
    memset(&cfg, 0, sizeof(cfg));

    if (ini_parse_string(ini, handler, &cfg) != 0) {
        fprintf(stderr, "inih parse failed\n");
        return 1;
    }

    printf("inih parsed name: %s\n", cfg.name);
    printf("inih parsed port: %d\n", cfg.port);
    return 0;
}
