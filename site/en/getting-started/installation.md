> 日本語版は[こちら](https://doc.poac.pm/ja/getting-started/installation.html)

## Installation

### Easy install
```bash
curl -fsSL https://sh.poac.pm | bash
```
*When your OS is macOS, use [Homebrew](https://github.com/Homebrew/brew)*

### Manual install (Build)
Poac requires the following tools and packages to build:
* [`cmake`](https://github.com/Kitware/CMake): `3.0` or higher
* [`boost`](https://github.com/boostorg): `1.66.0` or higher
* [`openssl`](https://github.com/openssl/openssl): as new as possible
* [`yaml-cpp`](https://github.com/jbeder/yaml-cpp): `0.6.0` or higher

```bash
$ git clone https://github.com/poacpm/poac.git
$ cd poac
$ mkdir build && cd $_
$ cmake ..
$ make
$ make install
```

### Supported Operating Systems
* Linux
    * x86_64-unknown-linux-gnu
    * mips-unknown-linux-gnu (next release)
    * powerpc-unknown-linux-gnu (next release)
    * powerpc64-unknown-linux-gnu (next release)
    * powerpc64le-unknown-linux-gnu (next release)
* macOS
    * macOS Sierra
    * macOS High Sierra
    * macOS Mojave
* Windows
    * Visual Studio 2017 x86
    * Visual Studio 2017 x64
    * MinGW-w64 (next release)
    * Cygwin (next release)
    * Cygwin64 (next release)


### About to distribute application
#### Linux
On Linux, I have been distributing pre-built binaries statically linked for each architecture at [GitHub Releases](https://github.com/poacpm/poac/releases).

#### macOS
On macOS, I have been distributing using [Homebrew Taps](https://docs.brew.sh/Taps).

Therefore you can also install poac with `brew install poacpm/poac/poac` command.

##### Reasons for still using [Homebrew Taps](https://docs.brew.sh/Taps)
Poac depends on `variant`, standard library of C++17.
However, previous than macOS High Sierra, `std::visit` is not implemented even though there is an implementation of `variant` library.

Try to compile, occur the following error:

```
Call to unavailable function 'visit': introduced in macOS 10.14
```

Therefore by depends on [llvm@7](https://formulae.brew.sh/formula/llvm@7) and diverting it to Clang of Homebrew, it is possible to build poac.

Not less than macOS Mojave does not need this process to build using Apple Clang.

I sent [Pull Request](https://github.com/Homebrew/homebrew-core/pull/36880#issuecomment-462224649) to Homebrew but a package that depend on [llvm](https://formulae.brew.sh/formula/llvm) were not merged into Homebrew.
Thus I have been using [Homebrew Taps](https://docs.brew.sh/Taps).

I'm going to send Pull Request when Homebrew no longer supports macOS High Sierra or earlier.
(If, Homebrew merges another package with the same name as poac before that, we will continue to use [Homebrew Taps](https://docs.brew.sh/Taps).)

##### If poac is merged into Homebrew
I distribute pre-built binaries for operating systems excepting macOS.
For macOS, it completely depends on [Homebrew](https://github.com/Homebrew/brew).
Therefore, to build at every installation if the status quo using Taps.

If merged into Homebrew, [Bottles](https://docs.brew.sh/Bottles) will be available and will not be built at every installation.
Unlike distributions on Linux and Windows, whether or not poac are merged into Homebrew, there are no plans to distribute pre-built binaries for macOS at [GitHub Releases](https://github.com/poacpm/poac/releases).

Poac installed by Homebrew will dynamically link to all dependent libraries.
If merged into Homebrew, it can be installed with the `brew install poac` command.

#### Windows
On Windows, I have been distributing pre-built binaries, which are linked only OpenSSL dynamically.
Because, you need to manually install OpenSSL.
[Easy install](#easy+install) is not supported for Windows, currently.
Please download the exe file directly from [GitHub Releases](https://github.com/poacpm/poac/releases).
