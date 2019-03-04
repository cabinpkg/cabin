## インストール

### サポートしているOS
* macOS (>= sierra)
* Linux (= x86_64 GNU/Linux)
* Windows (= Windows Subsystem for Linux)

### 簡単にインストールする
```bash
curl -fsSL https://sh.poac.io | bash
```
*お使いのPCのOSがmacOSの時、内部では [Homebrew](https://github.com/Homebrew/brew) を使用します*

### 手動でインストールする (ビルド)
poac はビルドするために以下のツールとパッケージが必要です:
* [`boost`](https://github.com/boostorg): `1.66.0` もしくはそれ以上
* [`cmake`](https://github.com/Kitware/CMake): `3.0` もしくはそれ以上
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

<!--
既に poac がインストール済みであれば、poac を使用してビルドすることが可能です:
```bash
$ poac build
```

---

### ビルド済みバイナリ
poacは、基本的にビルド済みバイナリで配布されます。
これらの操作は本来ユーザーが関知する必要はありませんが、
シェルスクリプトでのインストールを望まない場合は、
ビルド済みバイナリを直接ダウンロードすることができます(現状未対応)。

macOSに対しては、homebrewのbottleによってビルド済みバイナリが配布されます。
それ以外のOSに対しては、CircleCI上で各アーキテクチャごとのビルド済みバイナリを生成し、
それらを配布します。
それらはGitHub releaseに配置されるため、
[そこ](https://github.com/poacpm/poac/releases)からダウンロードできます。

homebrewで，sierra以下だと，llvmをインストールして，それでpoacのビルドを行いますが，
これは，<variant>内のstd::visitが　error: call to unavailable function 'visit': introduced in macOS 10.14
となるからです．
mojave以降では，llvmを必要とはしません．
-->
