#include <sqlite3.h>
#include <stdio.h>

int main(void) {
    printf("sqlite version: %s\n", sqlite3_libversion());
    printf("sqlite threadsafe: %d\n", sqlite3_threadsafe());

    sqlite3 *db = NULL;
    if (sqlite3_open(":memory:", &db) != SQLITE_OK) {
        fprintf(stderr, "open failed: %s\n", sqlite3_errmsg(db));
        sqlite3_close(db);
        return 1;
    }

    char *err = NULL;
    if (sqlite3_exec(db, "CREATE TABLE t(x); INSERT INTO t VALUES (42);", NULL,
                     NULL, &err)
        != SQLITE_OK) {
        fprintf(stderr, "exec failed: %s\n", err);
        sqlite3_free(err);
        sqlite3_close(db);
        return 1;
    }

    sqlite3_stmt *stmt = NULL;
    if (sqlite3_prepare_v2(db, "SELECT x FROM t", -1, &stmt, NULL) == SQLITE_OK
        && sqlite3_step(stmt) == SQLITE_ROW) {
        printf("sqlite query result: %d\n", sqlite3_column_int(stmt, 0));
    }
    sqlite3_finalize(stmt);
    sqlite3_close(db);
    return 0;
}
