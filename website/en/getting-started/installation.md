> 日本語版は[こちら](https://doc.poac.pm/ja/getting-started/installation.html)

## Installation

### Easy install
```bash
curl -fsSL https://sh.poac.pm | bash
```
*On macOS, the installer uses [Homebrew](https://github.com/Homebrew/brew)*.

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
    * powerpc-unknown-linux-gnu (next release)
    * powerpc64-unknown-linux-gnu (next release)
    * powerpc64le-unknown-linux-gnu (next release)
* macOS
    * macOS Catalina or later
* Windows
    * Visual Studio 2017 x86
    * Visual Studio 2017 x64
    * MinGW-w64 (next release)
    * Cygwin (next release)
    * Cygwin64 (next release)

### Why supporting macOS Catalina or later?
On macOS 10.14 or earlier, it doesn't implement std::filesystem, so if we attempt to use it, macOS's compiler will occur an error like the following.

```
error: 'path' is unavailable: introduced in macOS 10.15
/Applications/Xcode-11.0.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/../include/c++/v1/filesystem:739:24:
note: 'path' has been explicitly marked unavailable here
```

Therefore, I need to switch std::filesystem and boost::filesystem each macOS version, but in a C++ code, it can't be switched seamlessly, so I decided to support macOS Catalina or later.

### About distributing an application
#### Linux
On Linux, I have been distributing pre-built binaries linked statically for each architecture at [GitHub Releases](https://github.com/poacpm/poac/releases).

#### macOS
On macOS, I have been distributing using [Homebrew Taps](https://docs.brew.sh/Taps).

Therefore, you can also install Poac with the `brew install poacpm/poac/poac` command.

##### Reasons for still using [Homebrew Taps](https://docs.brew.sh/Taps)
Poac depends on `variant`, the standard library of C++17.
However, previous than macOS High Sierra, `std::visit` is not implemented even though there is superficially an implementation of `variant` library.

If trying to compile, it occurs the following error:

```
Call to unavailable function 'visit': introduced in macOS 10.14
```

Therefore, by depending on [llvm@7](https://formulae.brew.sh/formula/llvm@7) and diverting it to Clang of Homebrew, Poac can be built.

Not less than macOS Mojave does not need this process to build using Apple Clang.

I sent a [Pull Request](https://github.com/Homebrew/homebrew-core/pull/36880#issuecomment-462224649) to Homebrew, and Poac did not merge into Homebrew due to depending on [llvm](https://formulae.brew.sh/formula/llvm); thus, I used [Homebrew Taps](https://docs.brew.sh/Taps).

I plan to send Pull Request again when Homebrew no longer supports macOS High Sierra or earlier.
(If Homebrew merges another package with the same name as poac before that, I will continue to use [Homebrew Taps](https://docs.brew.sh/Taps).)

##### If Poac merged into Homebrew
I distribute pre-built binaries for operating systems excepting macOS.
For macOS, it completely depends on [Homebrew](https://github.com/Homebrew/brew); therefore, Poac should be built at every installation if using [Homebrew Taps](https://docs.brew.sh/Taps).

If Poac merged into Homebrew, [Bottles](https://docs.brew.sh/Bottles) will be available and will not be built at every installation.
Unlike distributions on Linux and Windows, whether or not Poac merged into Homebrew, there is no plan to distribute pre-built binaries for macOS at [GitHub Releases](https://github.com/poacpm/poac/releases).

Poac installed by Homebrew will dynamically link to all dependent libraries.
If Poac merged into Homebrew, Poac can be installed with the `brew install poac` command.

#### Windows
On Windows, I distribute pre-built binaries, which are linked only OpenSSL dynamically, so you will need to manually install OpenSSL.
[Easy install](#easy-install) is not supported for Windows currently.
Please download the exe file from [GitHub Releases](https://github.com/poacpm/poac/releases) directly.
