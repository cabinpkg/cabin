## 新しいプロジェクトの作成


まず、[1.2. Hello World](../getting-started/hello-world.md) と同じように`poac new`コマンドを実行してプロジェクトを作成します。
```bash
$ poac new hello

Your "hello" project was created successfully.


Go into your project by running:
    $ cd hello

Start your project with:
    $ poac run
```

作成されたディレクトリに移動してファイルを見ると、`poac.yml`というファイルが作成されています。
このファイルの最後に、以下の内容を加えてみます。
depsは，dependenciesの略で依存関係という意味です。
現在のhelloプロジェクトが使用するパッケージの一覧をdepsキーに示します。
今回の場合は，hello_worldパッケージの0.1.0以上で1.0.0より小さいバージョンを使用するという意味になります。
```yaml
deps:
  hello_world: ">=0.1.0 and <1.0.0"
```

以上の編集をしたら、`poac install`を実行してインストールします。
```bash
$ poac install
==> Resolving packages...
==> Resolving dependencies...
==> Fetching...

  ●  hello_world 0.1.0 (from: poac)

==> Done.
```
*poac.ymlを編集しなくても、`poac install hello_world`でも自動でpoac.ymlを編集してインストール可能です*


今回インストールしたパッケージに合わせて、`main.cpp`を編集します。
```cpp
#include <iostream>
#include <hello_world.hpp>

int main(int argc, char** argv) {
   hello_world::say();
}
```

そして、`poac run`を実行すると、同じように`Hello, world!`が表示され，外部パッケージを使用することができました！
```bash
$ poac run
Compiled: Output to `_build/bin/hello`
Running: `_build/bin/hello`
Hello, world!
```
