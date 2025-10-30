CXX ?= clang++
GIT ?= git
PREFIX ?= /usr/local
INSTALL ?= install
BUILD ?= dev
COMMIT_HASH ?= $(shell $(GIT) rev-parse HEAD)
COMMIT_SHORT_HASH ?= $(shell $(GIT) rev-parse --short=8 HEAD)
COMMIT_DATE ?= $(shell $(GIT) show -s --date=format-local:'%Y-%m-%d' --format=%cd)

CXXFLAGS := -std=c++$(shell grep -m1 edition cabin.toml | cut -f 2 -d'"')
CXXFLAGS += -fdiagnostics-color
CXXFLAGS += $(shell grep cxxflags cabin.toml | head -n 1 | sed 's/cxxflags = \[//; s/\]//; s/"//g' | tr ',' ' ')
ifeq ($(BUILD),dev)
  CXXFLAGS += -g -O0 -DDEBUG
else ifeq ($(BUILD),release)
  CXXFLAGS += -O3 -DNDEBUG -flto
  LDFLAGS += -flto
else
  $(error "Unknown BUILD: `$(BUILD)'. Use `dev' or `release'.")
endif

O := build
PROJECT := $(O)/cabin
VERSION := $(shell grep -m1 version cabin.toml | cut -f 2 -d'"')
MKDIR_P := @mkdir -p

LIBGIT2_VERREQ := libgit2 >= 1.7.0, libgit2 < 1.10.0
LIBCURL_VERREQ := libcurl >= 7.79.1, libcurl < 9.0.0
NLOHMANN_JSON_VERREQ := nlohmann_json >= 3.10.5, nlohmann_json < 4.0.0
TBB_VERREQ := tbb >= 2021.5.0, tbb < 2023.0.0
FMT_VERREQ := fmt >= 9.0.0, fmt < 13.0.0
SPDLOG_VERREQ := spdlog >= 1.8.0, spdlog < 2.0.0
TOML11_VER := $(shell grep -m1 toml11 cabin.toml | sed 's/.*tag = \(.*\)}/\1/' | tr -d '"')
RESULT_VER := $(shell grep -m1 cpp-result cabin.toml | sed 's/.*tag = \(.*\)}/\1/' | tr -d '"')

DEFINES := -DCABIN_CABIN_PKG_VERSION='"$(VERSION)"' \
  -DCABIN_CABIN_COMMIT_HASH='"$(COMMIT_HASH)"' \
  -DCABIN_CABIN_COMMIT_SHORT_HASH='"$(COMMIT_SHORT_HASH)"' \
  -DCABIN_CABIN_COMMIT_DATE='"$(COMMIT_DATE)"'
INCLUDES := -Isrc -isystem $(O)/DEPS/toml11/include \
  -isystem $(O)/DEPS/mitama-cpp-result/include \
  $(shell pkg-config --cflags '$(LIBGIT2_VERREQ)') \
  $(shell pkg-config --cflags '$(LIBCURL_VERREQ)') \
  $(shell pkg-config --cflags '$(NLOHMANN_JSON_VERREQ)') \
  $(shell pkg-config --cflags '$(TBB_VERREQ)') \
  $(shell pkg-config --cflags '$(FMT_VERREQ)') \
  $(shell pkg-config --cflags '$(SPDLOG_VERREQ)')
LIBS := $(shell pkg-config --libs '$(LIBGIT2_VERREQ)') \
  $(shell pkg-config --libs '$(LIBCURL_VERREQ)') \
  $(shell pkg-config --libs '$(TBB_VERREQ)') \
  $(shell pkg-config --libs '$(FMT_VERREQ)') \
  $(shell pkg-config --libs '$(SPDLOG_VERREQ)')

SRCS := $(shell find src -name '*.cc')
OBJS := $(patsubst src/%,$(O)/%,$(SRCS:.cc=.o))
DEPS := $(OBJS:.o=.d)

GIT_DEPS := $(O)/DEPS/toml11 $(O)/DEPS/mitama-cpp-result


.PHONY: all clean install versions


all: check_deps $(PROJECT)

check_deps:
	@pkg-config '$(LIBGIT2_VERREQ)' || (echo "Error: $(LIBGIT2_VERREQ) not found" && exit 1)
	@pkg-config '$(LIBCURL_VERREQ)' || (echo "Error: $(LIBCURL_VERREQ) not found" && exit 1)
	@pkg-config '$(NLOHMANN_JSON_VERREQ)' || (echo "Error: $(NLOHMANN_JSON_VERREQ) not found" && exit 1)
	@pkg-config '$(TBB_VERREQ)' || (echo "Error: $(TBB_VERREQ) not found" && exit 1)
	@pkg-config '$(FMT_VERREQ)' || (echo "Error: $(FMT_VERREQ) not found" && exit 1)
	@pkg-config '$(SPDLOG_VERREQ)' || (echo "Error: $(SPDLOG_VERREQ) not found" && exit 1)

$(PROJECT): $(OBJS)
	$(CXX) $(LDFLAGS) $^ -o $@ $(LIBS)

$(O)/%.o: src/%.cc $(GIT_DEPS)
	$(MKDIR_P) $(@D)
	$(CXX) $(CXXFLAGS) -MMD $(DEFINES) $(INCLUDES) -c $< -o $@

-include $(DEPS)


install: all
	$(INSTALL) -m 0755 -d $(DESTDIR)$(PREFIX)/bin
	$(INSTALL) -m 0755 $(PROJECT) $(DESTDIR)$(PREFIX)/bin

clean:
	-rm -rf $(O)

versions:
	$(MAKE) -v
	$(CXX) --version

#
# Git dependencies
#

$(O)/DEPS/toml11:
	$(MKDIR_P) $(@D)
	$(GIT) clone https://github.com/ToruNiina/toml11.git $@
	$(GIT) -C $@ reset --hard $(TOML11_VER)

$(O)/DEPS/mitama-cpp-result:
	$(MKDIR_P) $(@D)
	$(GIT) clone https://github.com/loliGothicK/mitama-cpp-result.git $@
	$(GIT) -C $@ reset --hard $(RESULT_VER)
