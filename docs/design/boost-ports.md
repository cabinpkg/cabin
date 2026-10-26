# Boost 1.92.0 foundation ports

This is the working ledger for porting every Boost 1.92.0 library as a `cabin-ports/boost-*`
[foundation port](foundation-ports.md).  It records the inventory, the rules each port follows
and the evidence behind them, the Cabin gaps the work found, and per-library status.  It is
reconciled against the repository, open pull requests, and the registry whenever the work resumes.

## Release

- Tag `boost-1.92.0`, superproject commit `afdfa32505af73e3d208144b3f623f0096cb62b6`.  The
  *Commit* column below is each library's submodule commit at that tag.
- Each library is its own port sourced from its repository's tag archive,
  `https://github.com/boostorg/<repo>/archive/refs/tags/boost-1.92.0.tar.gz` with `strip-prefix =
  "<repo>-boost-1.92.0"`, pinned by SHA-256.
- Nothing publishes until the whole corpus is merged and locally validated.  Then one
  `ports-publish.yml` dispatch publishes the set, and `registry-verify.yml` and a clean
  credential-free consumer confirm it.

## Inventory

`boostdep --list-dependencies` at the tag lists 158 modules and no dependency cycles.  `headers` is
b2 build infrastructure (an empty `include/boost/headers/.gitkeep`), not a library, so it has no
package; 157 packages remain.

| Kind | Count | Meaning |
| --- | --- | --- |
| header-only | 127 | Upstream CMake declares an `INTERFACE` library, with one exception: `date_time`'s library compiles only an exported dummy function, so it is header-only too.  `exception` and `regex` ship sources that upstream CMake never builds. |
| compiled | 30 | Upstream CMake builds a static or shared library. |

## Rules

### Naming

`cabin-ports/boost-<repo>` with `_` spelled `-`; the numeric sub-libraries take their repository
name (`boost-interval`, `boost-numeric-conversion`, `boost-odeint`, `boost-ublas`).  The library
target is `boost_<module>` (`numeric~odeint` is `boost_numeric_odeint`).

### Dependencies

A port depends on its module's boostdep primary dependencies, each with the exact requirement
`=1.92.0` and as a `public = true` target edge, since Boost headers include their dependencies'
headers.  A package with several library targets is referenced as `package:target`.

Six boostdep edges are dropped.  Each exists only through an opt-in integration header that the
upstream CMake target also omits or that needs an SDK no port can provide, and each would
otherwise force a blocked SDK, a cycle, or a stricter standard on every consumer:

| Edge | Only through | Kept out because |
| --- | --- | --- |
| `mpi` -> `python` | `boost/mpi/python/*` | Python bindings need the Python SDK |
| `numeric~odeint` -> `mpi` | `boost/numeric/odeint/external/mpi/*` | MPI SDK |
| `numeric~odeint` -> `compute` | `boost/numeric/odeint/external/compute/*` | OpenCL SDK |
| `mysql` -> `pfr` | `boost/mysql/pfr.hpp` | `pfr` requires C++14 (`#error` below it); `mysql` supports C++11 |
| `histogram` -> `serialization` | `boost/histogram/serialization.hpp` | a compiled library every histogram consumer would link |
| `numeric~ublas` -> `compute` | `boost/numeric/ublas/opencl*.hpp` | those headers also need the OpenCL SDK, so `compute` can never be a port; upstream CMake keeps this edge |

### Language standards

`interface-cxx-standard` is the effective floor over the dependency closure: the larger of the
module's own floor and every dependency's.  The module's own floor is `cxxstd` from
`meta/libraries.json`, raised by its main CMake target's `cxx_std_NN` (ignoring the
`BOOST_USE_MODULES` variants).  Where the metadata is missing or stale, the main target's CMake
requirement decides: `compute`, `coroutine`, `describe`, `numeric~ublas` (C++11); `detail`,
`foreach`, `local_function`, `polygon`, `scope_exit`, `units`, `xpressive` (C++03, spelled
`c++98`).  `describe` declares C++11 because `container_hash` includes it unconditionally; its
macros expand to nothing before C++14 by design.

A compiled port builds at C++14, since MSVC has no `/std:c++11`, and its floor is at least that
build standard, so its dependents' floors are too.  With Apple clang 21's libc++, a C++14 object
and a C++11 object that both look up a `std::map<std::string, ...>` can crash when linked together
(reproduced with a C++14 library and a C++11 caller), so a C++11 consumer of a C++14 library is
unsafe on macOS.

### Patches

Each patch is the smallest correction that makes the port correct under Cabin.

| Port | Patch | Why |
| --- | --- | --- |
| `boost-config` | `user.hpp` defines `BOOST_ALL_NO_LIB` | MSVC auto-linking (`#pragma comment(lib, ...)`) names b2-style archives Cabin never produces; Cabin links each compiled library through target `deps`.  vcpkg ships the same change. |
| `boost-pfr` | exclude clang from a GCC < 8 alignment workaround in `offset_based_getter.hpp` | clang defines `__GNUC__` as 4, so on non-x86_64 hosts (arm64 macOS and Linux) the C++14 field getter capped alignment at 4 and read `double` and `long long` fields at the wrong offsets |
| `boost-safe-numerics` | `checked_result_operations.hpp` includes `<exception>` | it calls `std::terminate` without the include, which fails on libc++ |
| `boost-sort` | `common/util/algorithm.hpp` and `common/scheduler.hpp` include `<cstdint>` | the headers use `uint32_t` and friends without the include, which fails on libstdc++ |
| `boost-sort` | `common/util/traits.hpp` includes `<string>` | the header uses `std::string` without the include, which fails on MSVC |

### Consumer tests

Every port has a consumer test at `ports/boost-<x>/1.92.0/tests/main.cc` that includes the
library's public headers and checks real behavior at run time.  The tests are portable across
GCC, Clang, and MSVC, use no other Boost library as scaffolding, and compile at the port's
declared floor and at C++20.  The preflight builds each test against the published package, which
makes any include outside the port's dependency closure a build failure, and runs it on Linux,
macOS, and Windows.

## Cabin gaps and blockers

| Gap | Libraries | State |
| --- | --- | --- |
| Platform-specific sources are not self-guarded | `thread`, `locale`, `log`, `fiber`, `stacktrace`, `context`, others | done: `[target.'cfg(...)'.sources]` tables (#2035) |
| Assembly sources (`.S` through the GCC/Clang driver, MSVC `.asm` through `ml64`) | `context`, and through it `coroutine`, `coroutine2`, `fiber`, `cobalt`, `asio`, `beast`, `process`, `log` | done: `.S` and `.asm` sources (#2037) |
| Archive above the operator `max_archive_bytes` (16 MiB) | `math` (30.6 MB compressed; the cap is the same in every quota class), and everything that depends on it | done: `[package] include` archives only the listed paths (#2038); `include/` alone is 2.4 MB |
| Defines a consumer must set: Cabin propagates no defines to consumers | `stacktrace` (`BOOST_STACKTRACE_LINK`); `contract` ships header-only instead of requiring `BOOST_CONTRACT_STATIC_LINK` | open |
| MSVC `/bigobj`: ports cannot pass compiler flags | `json`, `log`, `xpressive`, and the large consumer tests | not hit: the Windows dry-run builds them and their consumer tests without it (#2052) |
| SDKs Cabin cannot provide: ports may not declare system dependencies, and these SDKs cannot be built as ports | `python`, `parameter_python` (Python); `mpi`, `graph_parallel`, `property_map_parallel` (MPI); `compute` (OpenCL); `mysql`, `redis` (OpenSSL) | needs a maintainer decision |

## Status

`not_started` -> `in_progress` -> `locally_validated` (the port and its consumer test pass the
local preflight against a disposable registry) -> `merged` -> `published_pending` -> `verified`.
`blocked` names its blocker.

| Module | Package | Commit | Kind | Std | Direct deps | Status | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `accumulators` | `boost-accumulators` | `6bd97b3843f3488af28c18aa9d0f06275b6c6c75` | header-only | c++14 | 19 | merged |  |
| `algorithm` | `boost-algorithm` | `fd1cf19c0f84c483b3310c34fd600fe8b2725ccc` | header-only | c++11 | 16 | merged |  |
| `align` | `boost-align` | `440281d63d1c0b7c7fde63ded67b4860b57d5756` | header-only | c++98 | 3 | merged |  |
| `any` | `boost-any` | `3312581e774129462e06bbdc752b919dca37e85c` | header-only | c++11 | 4 | merged |  |
| `array` | `boost-array` | `3df3aafd1924084d46988590bd94cf4c1b362859` | header-only | c++98 | 3 | merged |  |
| `asio` | `boost-asio` | `4fa4abee89a62fdeeccac2585caece625f40647e` | header-only | c++14 | 7 | merged | the SSL headers need OpenSSL, which has no port; they compile only for a consumer that supplies it |
| `assert` | `boost-assert` | `fc2a476cc7d9f42b65ec104e90d24bfd6290efdc` | header-only | c++98 | 1 | merged |  |
| `assign` | `boost-assign` | `ed938773ebdaf8cec5aba034be3d22ab20f6f2d3` | header-only | c++11 | 10 | merged |  |
| `atomic` | `boost-atomic` | `84dccb548a7d8353c8ae4021aa7903bc94f80e0b` | compiled | c++14 | 7 | merged | generic address search: the SSE2/SSE4.1 variants need per-source ISA flags |
| `beast` | `boost-beast` | `7c1e061f91e2ef542217b76286c314d006c0c8fc` | header-only | c++14 | 19 | merged | the SSL headers need OpenSSL, which has no port; they compile only for a consumer that supplies it |
| `bimap` | `boost-bimap` | `705a7ca7f49f197a803134ade3a8c64473617a24` | header-only | c++11 | 13 | merged |  |
| `bind` | `boost-bind` | `8cc29fc19db49e791743c821821e201b46ab9c66` | header-only | c++11 | 2 | merged |  |
| `bloom` | `boost-bloom` | `0909b9955e2dc02a0356351a2dfb567d4d5af772` | header-only | c++11 | 6 | merged |  |
| `callable_traits` | `boost-callable-traits` | `2a56a3a2496cdb66496f844db55085dd992d5e49` | header-only | c++11 | 0 | merged |  |
| `charconv` | `boost-charconv` | `4475d6550fafc6dca45b333dccaf45d96d1cb196` | compiled | c++14 | 3 | merged |  |
| `chrono` | `boost-chrono` | `1207f5b403459f3fa2900252edcde9051f904d22` | compiled | c++14 | 14 | merged |  |
| `circular_buffer` | `boost-circular-buffer` | `5d8bebb7614157768d49372618ebbe3049375820` | header-only | c++98 | 7 | merged |  |
| `cobalt` | `boost-cobalt` | `8b1cecba91d0e868ee0947b40fb04e3594680297` | compiled | c++20 | 16 | merged | `boost_cobalt` and `boost_cobalt_io`; `boost_cobalt_io_ssl` needs OpenSSL and is left out; GCC 11 or later, since GCC 10 needs `-fcoroutines` |
| `compat` | `boost-compat` | `349fb928b5ef800d1b8544cb4c382c39ecde0b3c` | header-only | c++11 | 3 | merged |  |
| `compute` | `boost-compute` | `763f995ed15661701a6ade23159c31747711ed31` | header-only | c++11 | 27 | blocked | needs OpenCL headers and library: a system dependency, which ports may not declare |
| `concept_check` | `boost-concept-check` | `235e54ebf23be678045e0eeae90f47ca0e2c95ce` | header-only | c++98 | 3 | merged |  |
| `config` | `boost-config` | `115e718e0fd72329e69fc776dac99811385d6f77` | header-only | c++98 | 0 | merged |  |
| `container` | `boost-container` | `f74270de43714edccf17b33eca454348d47402b7` | compiled | c++14 | 4 | merged | `alloc_lib.c` is C, so its four C++-only edges carry `ignore-interface-standard` |
| `container_hash` | `boost-container-hash` | `2698b43803c012601e6bb1a6116e83767b97986c` | header-only | c++11 | 3 | merged |  |
| `context` | `boost-context` | `cc1b9e02cd530d6607becc984e1e5bfa4f7b3600` | compiled | c++14 | 7 | merged | fcontext with each platform's `.S` or MSVC `.asm` sources (#2037); Windows is x64 only, since arm64 MSVC needs armasm |
| `contract` | `boost-contract` | `723fe65e41c1c942e6b7637c3bdc72e283d7beca` | compiled | c++14 | 15 | merged | ported header-only: upstream's mode for consumers that do not define `BOOST_CONTRACT_STATIC_LINK` |
| `conversion` | `boost-conversion` | `71b14ad1dae1d2be91ad310007c749ca93dc2e72` | header-only | c++11 | 4 | merged |  |
| `convert` | `boost-convert` | `6dd61f248aba9e895346bc1c3bacc2ad007bcc9c` | header-only | c++14 | 11 | merged | `boost/convert/charconv.hpp` needs C++17 |
| `core` | `boost-core` | `a90a31934fe8bcb6e6be6dfea77b80492c7b6c81` | header-only | c++98 | 3 | merged |  |
| `coroutine` | `boost-coroutine` | `8b09bf7d0e8a13d1ef59aaff02a84d42acb034f3` | compiled | c++14 | 10 | merged | |
| `coroutine2` | `boost-coroutine2` | `a19f882791820f30c145663848f8bfd882815ef6` | header-only | c++14 | 3 | merged | |
| `crc` | `boost-crc` | `d547322aa734b37901fa79e7b5e71040d698ce69` | header-only | c++11 | 0 | merged |  |
| `date_time` | `boost-date-time` | `6248d64ed94916e446c83a038e8c8208ebb262ee` | header-only | c++14 | 14 | merged |  |
| `decimal` | `boost-decimal` | `58c7cca6b0a7d80884428315d80de3ac7254d396` | header-only | c++14 | 0 | merged |  |
| `describe` | `boost-describe` | `5e7b4b84c8d105093687b940a45ac22df47b1ab4` | header-only | c++11 | 1 | merged |  |
| `detail` | `boost-detail` | `965826dc374165d71530d9814ecd5f4628365522` | header-only | c++98 | 4 | merged |  |
| `dll` | `boost-dll` | `de905f331abdfac9aabe895b2bbbf1fbca71a002` | header-only | c++14 | 10 | merged |  |
| `dynamic_bitset` | `boost-dynamic-bitset` | `b2c17416ea930e8b6b8eb6f891970a3fd29bd134` | header-only | c++11 | 5 | merged |  |
| `endian` | `boost-endian` | `4bffdf3defc2836409e72622066a40d8396088ae` | header-only | c++11 | 1 | merged |  |
| `exception` | `boost-exception` | `afcb28d8f2517eda7b6b2cba1cd8b6dc3bbaf0d9` | header-only | c++11 | 7 | merged |  |
| `fiber` | `boost-fiber` | `159c5c2ee465f1c0fe2eaded6a15881db3ad76d7` | compiled | c++14 | 10 | merged | `boost_fiber` and `boost_fiber_numa` |
| `filesystem` | `boost-filesystem` | `d73863751bf0570fc82f1b49246f909cea61083b` | compiled | c++14 | 14 | merged |  |
| `flyweight` | `boost-flyweight` | `fcd6671d978652a7debe9e2530021b047d50ae41` | header-only | c++14 | 14 | merged |  |
| `foreach` | `boost-foreach` | `904451bb8b094b426a69fbf33825c06677f2835f` | header-only | c++11 | 6 | merged |  |
| `format` | `boost-format` | `66075102416bc5d0f8e0bd151a19013dadbf94ce` | header-only | c++11 | 7 | merged |  |
| `function` | `boost-function` | `18650af5175ea247aebc60ff12db1b477123d5dc` | header-only | c++11 | 5 | merged |  |
| `function_types` | `boost-function-types` | `e454e797fbd2e1df704306e8ef70836e8bcb71ae` | header-only | c++98 | 6 | merged |  |
| `functional` | `boost-functional` | `6a573e4b8333ee63ee62ce95558c3667348db233` | header-only | c++11 | 9 | merged |  |
| `fusion` | `boost-fusion` | `017a8399fd62e81cf11ecea8c4063d055088458e` | header-only | c++11 | 10 | merged |  |
| `geometry` | `boost-geometry` | `90c27ef0bf9d1700f553b85f890f43461eced529` | header-only | c++14 | 28 | merged | |
| `gil` | `boost-gil` | `8420c6c4e949087df84f2748419cf53fa2f49c9b` | header-only | c++14 | 12 | merged | the jpeg/png/raw/tiff io backends need libraries the consumer supplies |
| `graph` | `boost-graph` | `fc5dac2c552f945846449fd123a9b8335d5c063a` | compiled | c++14 | 41 | merged | |
| `graph_parallel` | `boost-graph-parallel` | `f91d58beb1b685005ee15899c8bce8b025cb7e85` | compiled |  | 25 | blocked | needs MPI (see `mpi`) |
| `hana` | `boost-hana` | `bc49ee25638e59d977edff5737b4e6bf12c1e5ea` | header-only | c++14 | 5 | merged |  |
| `hash2` | `boost-hash2` | `8663d080190ec3128947c1f7e61d15bafcd9c775` | header-only | c++11 | 5 | merged |  |
| `heap` | `boost-heap` | `a43cc4b7e7dfa51b4f1c163f3bf29cd785047bd1` | header-only | c++14 | 9 | merged |  |
| `histogram` | `boost-histogram` | `5e9b7b162e8a2801b77ccf199dedea88fa44325c` | header-only | c++14 | 7 | merged | |
| `hof` | `boost-hof` | `63ac019fca325038bfb4f56db17fbb281d2b5f49` | header-only | c++11 | 0 | merged |  |
| `icl` | `boost-icl` | `7de3f55655984e22f843238f7bd795fea3a5e08d` | header-only | c++14 | 14 | merged |  |
| `integer` | `boost-integer` | `e513075061f125c631a420e8960e0c606bd4b810` | header-only | c++98 | 5 | merged |  |
| `interprocess` | `boost-interprocess` | `79d5b935c51b302cefbcf8c8182fd584d586b39a` | header-only | c++14 | 6 | merged |  |
| `intrusive` | `boost-intrusive` | `b089da5af88981d6e87392680b0b68fd30be0b12` | header-only | c++98 | 3 | merged |  |
| `io` | `boost-io` | `342e4c6d10d586058818daa84201a2d301357a53` | header-only | c++98 | 1 | merged |  |
| `iostreams` | `boost-iostreams` | `dacdb3db57e9f372ad575f02a45b5efb439d8889` | compiled | c++14 | 17 | merged | zlib and gzip filters (through `cabin-ports/zlib`); bzip2, lzma, zstd have no ports |
| `iterator` | `boost-iterator` | `031662886357f9172b448604d11127f629efbc0b` | header-only | c++11 | 10 | merged |  |
| `json` | `boost-json` | `c57359dff379f278d7d5f8fa332d3dea684ba5fa` | compiled | c++14 | 10 | merged |  |
| `lambda` | `boost-lambda` | `a14036e312ffadfd65cb333b15602d2b3ab45ff3` | header-only | c++11 | 9 | merged |  |
| `lambda2` | `boost-lambda2` | `dda22ab8ee70bc67d89841ce9365d6913620ceda` | header-only | c++14 | 0 | merged |  |
| `leaf` | `boost-leaf` | `e8fe60e0cc48a559199c3a2cac64ec8579dc6a67` | header-only | c++11 | 0 | merged |  |
| `lexical_cast` | `boost-lexical-cast` | `35d8af6ce21fa7a163b68a2fb27437d3fb737232` | header-only | c++14 | 4 | merged |  |
| `local_function` | `boost-local-function` | `258aeaa6c1a9618c02ca4e4af2890e70833035f0` | header-only | c++11 | 7 | merged |  |
| `locale` | `boost-locale` | `94f4d99619e54e11dbb3117bad7af0cca76fd51c` | compiled | c++14 | 7 | merged | std, POSIX (iconv) and Windows backends; ICU has no port |
| `lockfree` | `boost-lockfree` | `9a06cd191f3226d4b77a1bc304df991ab470246f` | header-only | c++14 | 8 | merged |  |
| `log` | `boost-log` | `f0d196ee03609d8cea8448bc4ad5889d52d9e4a0` | compiled | c++14 | 36 | merged | `boost_log` and `boost_log_setup`; generic dump code, since the SSSE3/AVX2 variants need per-source ISA flags; Windows builds without the event log, which needs `mc.exe` |
| `logic` | `boost-logic` | `9b8703a2d6623405323b892f1d126a6b17ca1651` | header-only | c++98 | 2 | merged |  |
| `math` | `boost-math` | `f12be767c04d9499aaa6f88b22d5f1b464c1c27a` | header-only | c++14 | 9 | merged | `[package] include` archives `LICENSE` and `include/` (#2038); the 30.6 MB release tree is above the 16 MiB archive cap |
| `metaparse` | `boost-metaparse` | `5a7d50bda78dc7a2ef667a3b10ddef05f26c0256` | header-only | c++98 | 5 | merged |  |
| `move` | `boost-move` | `b1ecb39a75ed2de17c23c46a005ac79f59a528ca` | header-only | c++98 | 1 | merged |  |
| `mp11` | `boost-mp11` | `c3eba6ac6be2e21af33a1b2ec97634cd01bcc447` | header-only | c++11 | 0 | merged |  |
| `mpi` | `boost-mpi` | `536921fe649e4a712bc266f472602c0a1d185236` | compiled |  | 17 | blocked | needs an MPI implementation: a system dependency, which ports may not declare |
| `mpl` | `boost-mpl` | `9d1f81ffeb055ea1ce0d96370bf07c40a2843878` | header-only | c++98 | 6 | merged |  |
| `mqtt5` | `boost-mqtt5` | `0e08e614a4c00e35e7bcf7b9d0f9009712fb4d34` | header-only | c++17 | 11 | merged | the SSL headers need OpenSSL, which has no port; they compile only for a consumer that supplies it |
| `msm` | `boost-msm` | `56d843170ef32cdab1471c467431d9cecbc0b073` | header-only | c++14 | 19 | merged |  |
| `multi_array` | `boost-multi-array` | `a31951fd81aaf02530f3819681b0b79250cde79f` | header-only | c++11 | 9 | merged |  |
| `multi_index` | `boost-multi-index` | `b6a0722f11cd96bf5f12809efe9ae6b46fe0b5b6` | header-only | c++11 | 13 | merged |  |
| `multiprecision` | `boost-multiprecision` | `ce3db86f748eb78f01cd4893295c1aed6520b381` | header-only | c++14 | 9 | merged | the GMP, MPFR, MPFI, MPC, tommath, Eigen, and libquadmath backends need libraries with no port |
| `mysql` | `boost-mysql` | `466675d89aaac199118c5ff65676fc9a552ce273` | header-only | c++11 | 16 | blocked | needs OpenSSL: a system dependency, which ports may not declare |
| `nowide` | `boost-nowide` | `e45509e57f2c2b2afa08b6df538d6933fac60fcd` | compiled | c++14 | 2 | merged |  |
| `numeric~conversion` | `boost-numeric-conversion` | `d1b479f7a4aa54d8ffb93d8dc4ee0c24670210d8` | header-only | c++11 | 7 | merged |  |
| `numeric~interval` | `boost-interval` | `2939307e2797fda9df945ab25ab5d6b258d776d4` | header-only | c++98 | 3 | merged |  |
| `numeric~odeint` | `boost-odeint` | `485a46baa908df8569756ae80d72b0861427788c` | header-only | c++14 | 17 | merged | |
| `numeric~ublas` | `boost-ublas` | `1e660d66d2bcc6e7afddbb0d0c91507f4c252bf1` | header-only | c++14 | 12 | merged | the `compute` edge is dropped (Dependencies) |
| `openmethod` | `boost-openmethod` | `79b6f7ff73876267f094aced56e681495fa42e73` | header-only | c++17 | 7 | merged |  |
| `optional` | `boost-optional` | `c0648f5f2d7b7d8e59c6e2c1d203f34c7aec129e` | header-only | c++11 | 5 | merged |  |
| `outcome` | `boost-outcome` | `8580f7690fae0f99e408c32faf50a51949a08043` | header-only | c++14 | 5 | merged |  |
| `parameter` | `boost-parameter` | `9984eff746f0ceb7b553ff89699d0405a3bfa896` | header-only | c++11 | 10 | merged |  |
| `parameter_python` | `boost-parameter-python` | `1f7f9ce9930119f0dda7dcd5e1ec3b5ed7c6b091` | header-only | c++14 | 4 | blocked | needs Python (see `python`) |
| `parser` | `boost-parser` | `202b991f982f0d27e5f0f2a93acabc9dad7c8288` | header-only | c++17 | 4 | merged |  |
| `pfr` | `boost-pfr` | `401385c240027423acbb1eb6dea2abe0043db5aa` | header-only | c++14 | 0 | merged |  |
| `phoenix` | `boost-phoenix` | `8913607a3788cb82d48ed461ea59c919b7bad3df` | header-only | c++11 | 13 | merged |  |
| `poly_collection` | `boost-poly-collection` | `8adeacb3f4ad35379391e2f85ea74e2d42d71b8a` | header-only | c++14 | 8 | merged |  |
| `polygon` | `boost-polygon` | `8ba35b57c1436c4b36f7544aadd78c2b24acc7db` | header-only | c++98 | 1 | merged |  |
| `pool` | `boost-pool` | `740c8076f9d02f0216e8f3dbb15d2fd80f67d7f4` | header-only | c++98 | 6 | merged |  |
| `predef` | `boost-predef` | `e1211a4ca467bb6512e99025772ca25afa8d6159` | header-only | c++98 | 0 | merged |  |
| `preprocessor` | `boost-preprocessor` | `cd1b1bd03900b68505822cfa25cb16851bd6caf1` | header-only | c++98 | 0 | merged |  |
| `process` | `boost-process` | `696b785c77352491ebe0e7c4a8dc348bbc9d5d53` | compiled | c++14 | 18 | merged | Process v2 on Boost.Filesystem |
| `program_options` | `boost-program-options` | `ae25c112b7032d288df90be95e10876ece30bee3` | compiled | c++14 | 12 | merged |  |
| `property_map` | `boost-property-map` | `e50f16089854fe0a987f8b10d65c19e69021ff78` | header-only | c++14 | 14 | merged |  |
| `property_map_parallel` | `boost-property-map-parallel` | `d63b8a0153f622f30a480bf24a48983cc8a6f0fc` | header-only | c++14 | 12 | blocked | needs MPI (see `mpi`) |
| `property_tree` | `boost-property-tree` | `7a7b02b09d38c542ff84d10c14eafdabc21105f1` | header-only | c++14 | 13 | merged |  |
| `proto` | `boost-proto` | `ecdc3411943314c0082d76c128141dab24a9cbc5` | header-only | c++11 | 9 | merged |  |
| `ptr_container` | `boost-ptr-container` | `43d147a4bd1c05a0e669698e2f6137e94eccfe32` | header-only | c++11 | 12 | merged |  |
| `python` | `boost-python` | `ed1e5e9be00558ffe1196ab72eb0053e0e27d0c4` | compiled |  | 20 | blocked | needs the Python SDK: a system dependency, which ports may not declare |
| `qvm` | `boost-qvm` | `2e574dd16f812b99423ad20fa72b3738ff053390` | header-only | c++98 | 0 | merged |  |
| `random` | `boost-random` | `08e70454559a1a58beed788dafa1631cdf9473ba` | compiled | c++14 | 10 | merged |  |
| `range` | `boost-range` | `7481e429b023655a6e77799d5f2cf4788145d494` | header-only | c++11 | 16 | merged |  |
| `ratio` | `boost-ratio` | `6eceadad2eb04f00b8cae5e400f7bee34c2d501f` | header-only | c++11 | 0 | merged |  |
| `rational` | `boost-rational` | `421b19c3eaa3fc8019b435faeb02666ad929493b` | header-only | c++11 | 7 | merged |  |
| `redis` | `boost-redis` | `5770217136a1fac160afc77a2de99e0384b12767` | header-only | c++17 | 7 | blocked | needs OpenSSL: a system dependency, which ports may not declare |
| `regex` | `boost-regex` | `7760ef2a61d643c1770026f0392b440bdfb5687d` | header-only | c++11 | 7 | merged |  |
| `safe_numerics` | `boost-safe-numerics` | `777e0be5ec763d0333a717c5e421a4f7c5e5bdc9` | header-only | c++14 | 6 | merged |  |
| `scope` | `boost-scope` | `c979d5e11b43cc36d5d5b8883e5b8a243478794f` | header-only | c++11 | 4 | merged |  |
| `scope_exit` | `boost-scope-exit` | `60baaae454b2da887a31cf939e22015b6263c9e4` | header-only | c++11 | 5 | merged |  |
| `serialization` | `boost-serialization` | `91c265be640294fbac4190b5084f9570f06f72ba` | compiled | c++14 | 22 | merged | two targets: reference `boost-serialization:boost_serialization` or `:boost_wserialization` |
| `signals2` | `boost-signals2` | `e9fc828dd873cc1939a976ad39a585fa57f6c0ff` | header-only | c++11 | 16 | merged |  |
| `smart_ptr` | `boost-smart-ptr` | `6e945160d788b8efdfc49ba4af1f8797cacd7c97` | header-only | c++11 | 4 | merged |  |
| `sort` | `boost-sort` | `48c424bee18ee2d886f83571cc592f0319b501e3` | header-only | c++11 | 4 | merged |  |
| `spirit` | `boost-spirit` | `64be6b02c5713cd175fbd2463e812cdcc8fc5941` | header-only | c++14 | 28 | merged |  |
| `stacktrace` | `boost-stacktrace` | `4f8cabb87fb35044d5df44ae44cbf57904c65189` | compiled | c++14 | 6 | merged | consumers define `BOOST_STACKTRACE_LINK`; basic (POSIX) and windbg (Windows) backends |
| `statechart` | `boost-statechart` | `e930707f777489543a03114e5fcc3213876e0ed5` | header-only | c++14 | 11 | merged |  |
| `static_assert` | `boost-static-assert` | `858097c08ebb53faaf1fba3201c0a76a96e49584` | header-only | c++98 | 1 | merged |  |
| `static_string` | `boost-static-string` | `2ff9d3535e853f4913e85e7fea28f84b04ea5d81` | header-only | c++11 | 6 | merged |  |
| `stl_interfaces` | `boost-stl-interfaces` | `48375e95648bed776a2f6d34148185daf2cdce96` | header-only | c++14 | 3 | merged |  |
| `system` | `boost-system` | `bc7c00fa67501ceadfde8e920835502340e8b899` | header-only | c++11 | 7 | merged |  |
| `test` | `boost-test` | `380722343f4edbda1bfb00c92836986d48297dd3` | compiled | c++14 | 17 | merged | `boost_unit_test_framework` only: the other two libraries define competing `main`s |
| `thread` | `boost-thread` | `608459ab7c50092f6d91dbbe48caf349ec16ed3b` | compiled | c++14 | 24 | merged |  |
| `throw_exception` | `boost-throw-exception` | `0924b53b40d1da33301f94fb97518f5a7df31e9b` | header-only | c++98 | 2 | merged |  |
| `timer` | `boost-timer` | `b7f65f09a0975f26cb294eebd8b17e91a5beff3f` | compiled | c++14 | 3 | merged |  |
| `tokenizer` | `boost-tokenizer` | `743082f58e964e7cef353a9678edaae055691ca0` | header-only | c++11 | 5 | merged |  |
| `tti` | `boost-tti` | `03734c54a51b6372ac3296d2fe5103b7360bcd3f` | header-only | c++98 | 5 | merged |  |
| `tuple` | `boost-tuple` | `704830d883825357a83e49a2aed2a07e734a74bb` | header-only | c++98 | 3 | merged |  |
| `type_erasure` | `boost-type-erasure` | `5281f14d3cb59f978c8cb6cbb90ce92dde4e3e13` | compiled | c++14 | 14 | merged |  |
| `type_index` | `boost-type-index` | `af648a10037497055d6f0823e22cf7e394e38458` | header-only | c++11 | 3 | merged |  |
| `type_traits` | `boost-type-traits` | `e6275ccf01c9cf8775ef0cb6188bd58f3b167a0f` | header-only | c++98 | 1 | merged |  |
| `typeof` | `boost-typeof` | `06748a1d65182a2dacfb9e0aa5dfc6230353b66d` | header-only | c++11 | 1 | merged |  |
| `units` | `boost-units` | `f39b667d7803a6428d869c0087d92128b1212685` | header-only | c++14 | 11 | merged | |
| `unordered` | `boost-unordered` | `636164f1357cf217374313820a457c31b50fcfc7` | header-only | c++11 | 7 | merged |  |
| `url` | `boost-url` | `a34f3a4a1a0e695f679453af71dfc050a59170e6` | compiled | c++14 | 10 | merged |  |
| `utility` | `boost-utility` | `8679ac0f1f769fa8d705a1d2329afb5fb6a1eaf2` | header-only | c++98 | 7 | merged |  |
| `uuid` | `boost-uuid` | `9c5f195bcdb5d85a857831e22bd182589c3374d0` | header-only | c++11 | 4 | merged |  |
| `variant` | `boost-variant` | `e8cb128f997ff639cb678adfcb4a9e14ad9d84e1` | header-only | c++11 | 12 | merged |  |
| `variant2` | `boost-variant2` | `dde1a3ac91d6986bae27b7f740689804d56cff61` | header-only | c++11 | 3 | merged |  |
| `vmd` | `boost-vmd` | `34cad2c1a574d445812c7c2432d3a5a5c843b412` | header-only | c++98 | 1 | merged |  |
| `wave` | `boost-wave` | `240769e26ccc5e0e667bc7387fc62223a837828d` | compiled | c++14 | 18 | merged |  |
| `winapi` | `boost-winapi` | `094612ee33a8dce960cd075537266fdb4788d059` | header-only | c++98 | 2 | merged |  |
| `xpressive` | `boost-xpressive` | `09002924ba85a6f4c82a3260915c4ef535a8f60d` | header-only | c++14 | 20 | merged |  |
| `yap` | `boost-yap` | `2d44666933c2993c992a3fc348776ba62cafad6b` | header-only | c++14 | 3 | merged |  |
