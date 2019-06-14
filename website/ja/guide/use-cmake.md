> Note: この機能はまだ実装されていません

## [WIP] CMakeを使う

```yaml
build:
  system: cmake
```

現状、systemに対して、poacを指定すると、poac標準のビルドシステムを使うことができますが、
cmake等を使うこともできます。
systemにcmakeを指定すると、poac build時に、プロジェクトルートから、CMakeLists.txtを選択してビルドできます。

内部では以下の処理が行われます。
```
mkdir _build && cd _build
cmake ..
make
```
という処理を行ってくれます。
cacheによって、適宜途中の処理は省かれます。
CMakeLists.txtの編集がなければ`make`のみ行われるといった処理。


引数を渡すこともできます
```yaml
build:
  system: cmake
  cmake_args:
    Boost_DEBUG: 1
  make_commands:
    - ""
    - "test"
    - "-j4 install" # 最適なスレッド数を計算、${THREAD}
```

これらはそれぞれそのまま、`-DBoost_DEBUG=1`の形で渡されます。
cmakeの場合は、渡した引数が全て一つのコマンドにまとめられます。

が、makeの場合は、渡した分コマンドが実行されます。
上の例だと、
make
make test
make -j4 install


build方法を分岐させたい場合は、以下のように書くことができます。
```yaml
build.gcc:
  system: poac
build.clang:
  system: cmake
build.msvc:
  system: cmake
```
