> 日本語版は[こちら](https://doc.poac.pm/ja/getting-started/installation.html)

## Installation

Since all packages through these providers may not be maintained by Poac owners, install them at your own risk.

[![Packaging status](https://repology.org/badge/vertical-allrepos/poac.svg)](https://repology.org/project/poac/versions)

### macOS

```bash
brew install poacpm/tap/poac
```

### Arch Linux

> [poac](https://aur.archlinux.org/packages/poac/), [poac-devel-git](https://aur.archlinux.org/packages/poac-devel-git), and [poac-git](https://aur.archlinux.org/packages/poac-git)
```bash
pacman -S poac
```

### Build from source

Should your environment is not listed on released packages, you will need to build Poac from source.
Poac requires the following compilers, tools, and libraries to build:

#### compilers

* Compilers which support [C++20](https://en.cppreference.com/w/cpp/20)
  * `GCC`: `11` or later
  * `Clang`: `12` or later
  * `Apple Clang`: provided by `macOS Big Sur` or later

#### tools

* [`CMake`](https://gitlab.kitware.com/cmake/cmake): [`3.21`](https://gitlab.kitware.com/cmake/cmake/-/tree/v3.21.6) or later
* [`Ninja`](https://github.com/ninja-build/ninja): [`1.8`](https://github.com/ninja-build/ninja/releases/tag/v1.8.2) or later

#### libraries

* [`boost`](https://github.com/boostorg): [`1.70.0`](https://github.com/boostorg/boost/releases/tag/boost-1.70.0) or later
  * algorithm
  * asio
  * beast
  * dynamic_bitset
  * graph
  * predef
  * property_tree
  * range
  * regex
  * scope_exit
  * uuid
* [`openssl`](https://github.com/openssl/openssl): [`3.0.0`](https://github.com/openssl/openssl/releases/tag/openssl-3.0.0) or later
  * some `SHA256` functions are marked as [deprecated](https://github.com/openssl/openssl/blob/openssl-3.0.0/include/openssl/sha.h#L57-L79) since `3.0.0`

<details>
<summary>The following libraries will be automatically installed when configuring with CMake, so generally, you do not need to care about them.</summary>

---

**dependencies**

* [`fmt`](https://github.com/fmtlib/fmt): [`7.1.3`](https://github.com/fmtlib/fmt/releases/tag/7.1.3) or later
* [`git2-cpp`](https://github.com/ken-matsui/git2-cpp): [`v0.1.0-alpha.0`](https://github.com/ken-matsui/git2-cpp/releases/tag/v0.1.0-alpha.0) or later
* [`glob`](https://github.com/p-ranav/glob): [`v0.0.1`](https://github.com/p-ranav/glob/releases/tag/v0.0.1) or later
* [`libarchive`](https://github.com/libarchive/libarchive): [`v3.6.1`](https://github.com/libarchive/libarchive/tree/master) or later
  * requires [this commit](https://github.com/libarchive/libarchive/commit/a4c3c90bb828ab5f01589718266ac5d3fdccb854)
* [`libgit2`](https://github.com/libgit2/libgit2): [`0.27`](https://github.com/libgit2/libgit2/releases/tag/v0.27.7) or later
* [`mitama-cpp-result`](https://github.com/LoliGothick/mitama-cpp-result): [`master`](https://github.com/LoliGothick/mitama-cpp-result/tree/master) branch
  * requires [this commit](https://github.com/LoliGothick/mitama-cpp-result/commit/80cfbce0382a27262c15339a22a6f35246cac65e)
  * awaiting the next release above [`v9.2.1`](https://github.com/LoliGothick/mitama-cpp-result/releases/tag/v9.2.1)
* [`ninja`](https://github.com/ninja-build/ninja): [`master`](https://github.com/ninja-build/ninja/tree/master) branch
  * requires [`src/status.h`](https://github.com/ninja-build/ninja/blob/ad3d29fb5375c3122b2318ea5efad170b83e74e5/src/status.h)
  * awaiting the next release above [`v1.10.2`](https://github.com/ninja-build/ninja/releases/tag/v1.10.2)
* [`spdlog`](https://github.com/gabime/spdlog): [`1.9.0`](https://github.com/gabime/spdlog/releases/tag/v1.9.0) or later
* [`structopt`](https://github.com/p-ranav/structopt): [`v0.1.2`](https://github.com/p-ranav/structopt/releases/tag/v0.1.2) or later
* [`toml11`](https://github.com/ToruNiina/toml11): [`c26aa01`](https://github.com/ToruNiina/toml11/commit/c26aa013cdc75286f90e6d9f661c14890b3f358f) or later
  * requires [this commit](https://github.com/ToruNiina/toml11/commit/c26aa013cdc75286f90e6d9f661c14890b3f358f)
  * awaiting the next release above [`v3.7.1`](https://github.com/ToruNiina/toml11/releases/tag/v3.7.1)

**dev-dependencies**

* [`μt`](https://github.com/boost-ext/ut): [`v1.1.9`](https://github.com/boost-ext/ut/releases/tag/v1.1.9) or later

---

</details>

After you prepared these requirements, you can build Poac using the following commands:

```bash
$ git clone https://github.com/poacpm/poac.git
$ cd poac
$ cmake -B build -G Ninja -DCMAKE_BUILD_TYPE=Release
$ cd build
$ ninja
$ ninja install
```
