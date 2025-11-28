# Tools
CXX        ?= clang++
GIT        ?= git
PKG_CONFIG ?= pkg-config
INSTALL    ?= install

# Configuration
PREFIX  ?= /usr/local
DESTDIR ?=
BUILD   ?= dev
O       := build
PROJECT := $(O)/cabin

# Git info
COMMIT_HASH       ?= $(shell $(GIT) rev-parse HEAD)
COMMIT_SHORT_HASH ?= $(shell $(GIT) rev-parse --short=8 HEAD)
COMMIT_DATE       ?= $(shell $(GIT) show -s --date=format-local:'%Y-%m-%d' --format=%cd)

# cabin.toml
EDITION         := $(shell grep -m1 edition cabin.toml | cut -f 2 -d'"')
VERSION         := $(shell grep -m1 version cabin.toml | cut -f 2 -d'"')
CUSTOM_CXXFLAGS := $(shell grep -m1 cxxflags cabin.toml | sed 's/cxxflags = \[//; s/\]//; s/"//g' | tr ',' ' ')

# Git dependency versions
TOML11_VER := $(shell grep -m1 toml11 cabin.toml | sed 's/.*tag = \(.*\)}/\1/' | tr -d '"')
RESULT_VER := v11.0.0

GIT_DEPS   := $(O)/DEPS/toml11 $(O)/DEPS/mitama-cpp-result $(O)/DEPS/rs-cpp

# System dependency versions
PKGS :=							\
  'libgit2 >= 1.7.0'		'libgit2 < 1.10.0'	\
  'libcurl >= 7.79.1'		'libcurl < 9.0.0'	\
  'nlohmann_json >= 3.10.5'	'nlohmann_json < 4.0.0'	\
  'tbb >= 2021.5.0'		'tbb < 2023.0.0'	\
  'fmt >= 9.0.0'		'fmt < 13.0.0'		\
  'spdlog >= 1.8.0'		'spdlog < 2.0.0'

PKG_CFLAGS := $(shell $(PKG_CONFIG) --cflags $(PKGS))
PKG_LIBS   := $(shell $(PKG_CONFIG) --libs $(PKGS))

# Flags
DEFINES := -DCABIN_CABIN_PKG_VERSION='"$(VERSION)"' \
  -DCABIN_CABIN_COMMIT_HASH='"$(COMMIT_HASH)"' \
  -DCABIN_CABIN_COMMIT_SHORT_HASH='"$(COMMIT_SHORT_HASH)"' \
  -DCABIN_CABIN_COMMIT_DATE='"$(COMMIT_DATE)"'
INCLUDES := -Iinclude -Isrc -isystem $(O)/DEPS/toml11/include \
  -isystem $(O)/DEPS/mitama-cpp-result/include \
  -isystem $(O)/DEPS/rs-cpp/include

CXXFLAGS := -std=c++$(EDITION) -fdiagnostics-color $(CUSTOM_CXXFLAGS) \
  $(DEFINES) $(INCLUDES) $(PKG_CFLAGS) -MMD -MP

ifeq ($(BUILD),dev)
  CXXFLAGS += -g -O0 -DDEBUG
else ifeq ($(BUILD),release)
  CXXFLAGS += -O3 -DNDEBUG -flto
  LDFLAGS += -flto
else
  $(error "Unknown BUILD: `$(BUILD)'. Use `dev' or `release'.")
endif

LDLIBS := $(PKG_LIBS)

# Source files
SRCS := $(shell find src -name '*.cc') $(shell find lib -name '*.cc')
OBJS := $(SRCS:%.cc=$(O)/%.o)
DEPS := $(OBJS:.o=.d)

# Targets
.PHONY: all clean install versions check_deps
.DEFAULT_GOAL := all

all: check_deps $(PROJECT)

check_deps:
	@$(PKG_CONFIG) --print-errors --exists $(PKGS)

$(PROJECT): $(OBJS)
	@mkdir -p $(@D)
	$(CXX) $(LDFLAGS) $^ -o $@ $(LDLIBS)

$(O)/%.o: %.cc $(GIT_DEPS)
	@mkdir -p $(@D)
	$(CXX) $(CXXFLAGS) -c $< -o $@

-include $(DEPS)

install: all
	@mkdir -p $(DESTDIR)$(PREFIX)/bin
	$(INSTALL) -m 0755 $(PROJECT) $(DESTDIR)$(PREFIX)/bin

clean:
	@rm -rf $(O)

versions:
	@$(MAKE) -v
	@$(CXX) --version

$(O)/DEPS/toml11:
	@mkdir -p $(@D)
	@$(GIT) clone https://github.com/ToruNiina/toml11.git $@
	@$(GIT) -C $@ reset --hard $(TOML11_VER)

$(O)/DEPS/mitama-cpp-result:
	@mkdir -p $(@D)
	@$(GIT) clone https://github.com/loliGothicK/mitama-cpp-result.git $@
	@$(GIT) -C $@ reset --hard $(RESULT_VER)

$(O)/DEPS/rs-cpp:
	@mkdir -p $(@D)
	@$(GIT) clone https://github.com/ken-matsui/rs-cpp.git $@
