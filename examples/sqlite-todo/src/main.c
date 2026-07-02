#include <sqlite3.h>
#include <stdio.h>

/* A miniature todo list: schema, a few inserts, one UPDATE, then a
 * listing query. Everything lives in ':memory:' so the example is
 * deterministic and leaves no files behind. */

static int fail(sqlite3 *db, const char *what) {
    fprintf(stderr, "%s: %s\n", what, sqlite3_errmsg(db));
    sqlite3_close(db);
    return 1;
}

int main(void) {
    sqlite3 *db = NULL;
    if (sqlite3_open(":memory:", &db) != SQLITE_OK) {
        return fail(db, "open failed");
    }

    const char *setup =
        "CREATE TABLE todos("
        "  id INTEGER PRIMARY KEY,"
        "  title TEXT NOT NULL,"
        "  done INTEGER NOT NULL DEFAULT 0"
        ");"
        "INSERT INTO todos(title) VALUES"
        "  ('write the manifest'),"
        "  ('add a lockfile'),"
        "  ('ship v0.1.0');"
        "UPDATE todos SET done = 1 WHERE title = 'write the manifest';";
    if (sqlite3_exec(db, setup, NULL, NULL, NULL) != SQLITE_OK) {
        return fail(db, "setup failed");
    }

    sqlite3_stmt *stmt = NULL;
    if (sqlite3_prepare_v2(db, "SELECT id, title, done FROM todos ORDER BY id",
                           -1, &stmt, NULL)
        != SQLITE_OK) {
        return fail(db, "prepare failed");
    }
    printf("todo list:\n");
    while (sqlite3_step(stmt) == SQLITE_ROW) {
        printf("  [%c] #%d %s\n", sqlite3_column_int(stmt, 2) ? 'x' : ' ',
               sqlite3_column_int(stmt, 0),
               (const char *)sqlite3_column_text(stmt, 1));
    }
    sqlite3_finalize(stmt);

    if (sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM todos WHERE done = 0", -1,
                           &stmt, NULL)
            == SQLITE_OK
        && sqlite3_step(stmt) == SQLITE_ROW) {
        printf("open todos: %d\n", sqlite3_column_int(stmt, 0));
    }
    sqlite3_finalize(stmt);
    sqlite3_close(db);
    return 0;
}
