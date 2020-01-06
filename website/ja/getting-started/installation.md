> English version is [here](https://doc.poac.pm/en/getting-started/installation.html)

## インストール

### 簡単にインストールする
```bash
curl -fsSL https://sh.poac.pm | bash
```
*お使いのPCのOSがmacOSの時、インストーラは [Homebrew](https://github.com/Homebrew/brew) を使用します*

### 手動でインストールする (ビルド)
poac はビルドするために以下のツールとパッケージが必要です:
* [`cmake`](https://github.com/Kitware/CMake): `3.0` もしくはそれ以上
* [`boost`](https://github.com/boostorg): `1.66.0` もしくはそれ以上
* [`openssl`](https://github.com/openssl/openssl): できるだけ最新のもの
* [`yaml-cpp`](https://github.com/jbeder/yaml-cpp): `0.6.0` もしくはそれ以上

```bash
$ git clone https://github.com/poacpm/poac.git
$ cd poac
$ mkdir build && cd $_
$ cmake ..
$ make
$ make install
```

### サポートしているOS
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


### アプリケーションの配布について
#### Linux
Linuxでは、それぞれのアーキテクチャ向けにStatic Linkされたビルド済みバイナリを、[GitHub Releases](https://github.com/poacpm/poac/releases)で配布しています。

#### macOS
macOS向けでは、[Homebrew Taps](https://docs.brew.sh/Taps)を利用して配布しています。

そのため、`brew install poacpm/poac/poac`コマンドでもインストール可能です。

##### [Homebrew Taps](https://docs.brew.sh/Taps)を未だに利用している理由
Poacは、C++17の標準ライブラリである`variant`に依存しています。
ところが、macOS High Sierra以前では、`variant`ライブラリの実装があるにも拘らず、`std::visit`が実装されていません。
実際にコンパイルしようとすると、以下のエラーが表示されます。

```
Call to unavailable function 'visit': introduced in macOS 10.14
```

そのため、PoacはHomebrewのパッケージである、[llvm@7](https://formulae.brew.sh/formula/llvm@7)に依存し、HomebrewのClangに迂回させることでPoacをビルドしています。

macOS Mojave以降はこの作業が必要無いため、Apple Clangを用いてビルドしています。

本家に[Pull Request](https://github.com/Homebrew/homebrew-core/pull/36880#issuecomment-462224649)を出しましたが、[llvm](https://formulae.brew.sh/formula/llvm)に依存するパッケージは本家に入れることができませんでした。
そのため、[Homebrew Taps](https://docs.brew.sh/Taps)を利用しています。

HomebrewがmacOS High Sierra以前をサポート対象外にするタイミングで本家にPull Requestを出す予定です。
(その前にHomebrewにPoacと同じ名前の別のパッケージが取り込まれた場合、Tapsを利用し続けることになります。)

##### 本家に取り込まれた場合
macOS **以外** のOS向けには、ビルド済みバイナリを配布しています。
macOS向けには、[Homebrew](https://github.com/Homebrew/brew)に完全に依存しています。
そのため、Tapsを利用している現状では、インストール毎にビルドが行われます。

本家に取り込まれた時点で、[Bottles](https://docs.brew.sh/Bottles)が利用できるようになるため、インストール毎にビルドが行われることはありません。
LinuxやWindowsでの配布と違い、本家に取り込まれたかどうかに関わらず、[GitHub Releases](https://github.com/poacpm/poac/releases)でmacOS向けのビルド済みバイナリを配布する予定はありません。

HomebrewでインストールされるPoacは、全ての依存ライブラリにDynamic Linkします。
本家に取り込まれた場合、`brew install poac`コマンドでインストールできるようになります。

#### Windows
Windowsでは、OpenSSLのみDynamic Linkしたバイナリを配布します。
そのため、OpenSSLのdllのみ手動でインストールする必要があります。
現在、[sh.poac.pmのスクリプトを使用してインストールする方法](#簡単にインストールする)はサポートしていません。
[GitHub Releases](https://github.com/poacpm/poac/releases)から、直接exeファイルをダウンロードしてください。
