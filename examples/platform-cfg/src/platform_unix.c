/* Compiled only where `cfg(family = "unix")` holds; see cabin.toml. */
const char *cabin_platform_source(void) { return "src/platform_unix.c"; }
