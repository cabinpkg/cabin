# Installing from Source

> [!CAUTION]
> This document explains how to build Cabin from source. Building from source is not recommended unless you are familiar with the process. If your goal is simply to install Cabin, refer to the [README.md](README.md) instead.

You will require the following compilers, commands, and libraries:

## Compilers (that support C++20)

* GCC: `12` or later
* Clang: `19` or later
* Apple Clang: provided by `macOS Sonoma (14)` or later

(generally, the latest 3 versions are supported to build Cabin.)

## Commands

* GNU Make
* Git
* pkg-config
* find
* grep
* mkdir
* rm

## Libraries

* fmt: `>=9 && <12`
    * `libfmt-dev` on APT (Debian/Ubuntu)
    * `fmt-devel` on DNF (Fedora)
    * `fmt` on Homebrew
* libgit2: `>=1.7 && <1.10`
    * `libgit2-dev` on APT (Debian/Ubuntu)
    * `libgit2-devel` on DNF (Fedora)
    * `libgit2` on Homebrew
* libcurl: `>=7.79.1 && <9`
    * `libcurl4-openssl-dev` on APT (Debian/Ubuntu)
    * `libcurl-devel` on DNF (Fedora)
    * `curl` on Homebrew
* nlohmann_json: `>=3.10.5 && <4`
    * `nlohmann-json3-dev` on APT (Debian/Ubuntu)
    * `json-devel` on DNF (Fedora)
    * `nlohmann-json` on Homebrew
* oneTBB: `>=2021.5.0 && <2023`
    * `libtbb-dev` on APT (Debian/Ubuntu)
    * `tbb-devel` on DNF (Fedora)
    * `tbb` on Homebrew
* spdlog: `>=1.8.0 && <2.0.0`
    * `libspdlog-dev` on APT (Debian/Ubuntu)
    * `spdlog-devel` on DNF (Fedora)
    * `spdlog` on Homebrew

Installation scripts for libraries:

* APT (Debian/Ubuntu):
  ```sh
  sudo apt-get update
  sudo apt-get install -y libfmt-dev libgit2-dev libcurl4-openssl-dev nlohmann-json3-dev libtbb-dev libspdlog-dev
  ```
* DNF (Fedora):
  ```sh
  sudo dnf install -y fmt-devel libgit2-devel libcurl-devel json-devel tbb-devel spdlog-devel
  ```
* Pacman (Arch/Manjaro):
  ```sh
  sudo pacman -Syu
  sudo pacman -S fmt libgit2 curl nlohmann-json tbb spdlog
  ```
* Homebrew:
  ```sh
  brew install fmt libgit2 curl nlohmann-json tbb spdlog
  ```

When running Make, the following libraries will be installed automatically.

* [toml11](https://github.com/ToruNiina/toml11): [`v4.2.0`](https://github.com/ToruNiina/toml11/releases/tag/v4.2.0)
* [mitama-cpp-result](https://github.com/loliGothicK/mitama-cpp-result): [`v10.0.0`](https://github.com/loliGothicK/mitama-cpp-result/releases/tag/v10.0.0)

Once you have all the necessary requirements in place, you can build Cabin by the following commands:

```bash
git clone https://github.com/cabinpkg/cabin.git
cd cabin
make BUILD=release -j$(nproc) install
```
