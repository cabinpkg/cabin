# Compiler settings
CC = clang++
# CFLAGS = -Wall -Wextra -Werror -fdiagnostics-color -pedantic-errors -std=c++20
CFLAGS = -Wextra -fdiagnostics-color -pedantic-errors -std=c++20
LDFLAGS = -L.

DEBUG_FLAGS = -g -O0 -DDEBUG
RELEASE_FLAGS = -O3 -DNDEBUG

# Archiver settings
AR = ar
ARFLAGS = rcs

# Directories
SRC_DIR = src
OUT_DIR = build-out

# Project settings
PROJ_NAME = $(OUT_DIR)/poac


all: $(PROJ_NAME)

clean:
	rm -rf $(OUT_DIR)

$(PROJ_NAME): $(OUT_DIR)/main.o
	$(CC) $(CFLAGS) $^ -o $@

$(OUT_DIR)/main.o: $(SRC_DIR)/main.cc src/Util/Rustify.hpp src/Util/Algos.hpp | $(OUT_DIR)
	$(CC) $(CFLAGS) -c $< -o $@

$(OUT_DIR):
	mkdir -p $(OUT_DIR)

.PHONY: all clean
