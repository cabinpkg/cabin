#ifndef SQLITE3_H
#define SQLITE3_H
#ifdef __cplusplus
extern "C" {
#endif
const char *sqlite3_libversion(void);
int sqlite3_threadsafe(void);
#ifdef __cplusplus
}
#endif
#endif
