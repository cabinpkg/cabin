:warning: こちらの情報は古いバージョンのものです。

## 設定ファイル


設定ファイルとは、poacが扱うpoac.ymlファイルのことで、
設定ファイルが対応しているキーは以下に示す通りです。

```elm
name : String
version : String
cpp_version : Int (98, 03, 11(Include TR1), 14, 17, 20) // https://doc.rust-lang.org/cargo/reference/config.html みたいな感じに！！！
description : String
owners : List String
license : String
links:
  github : Url
  homepage : Url
deps : Deps
build : Build // TODO: lib: true -> Generate library from src/*
test : Test // TODO: framework: boost or google
```

repositoryキーは，applicationでは不要です．libraryの場合のみ必要です．

基本的に全て書く必要はありません。
*これらの key 以外を記述してもエラーにはなりませんが、**JSONと違ってコメントが書ける**ため極力そちらを使用するのが良いでしょう*

また、それぞれのコマンドが必要とする key は以下の通りです。
そのコマンド実行時に必要とされていない場合は存在のチェックは行われません。

// TODO: これ，修正する！
|           | cpp_version | dependencies | build | test |
|:----------|:-----------:|:------------:|:-----:|:----:|
| build     | ◯           | ◯            | ◯     | ×    |
| cache     | ×           | ×            | ×     | ×    |
| cleanup   | ×           | ◯            | ×     | ×    |
| graph     | ×           | ◯            | ×     | ×    |
| init      | ×           | ×            | ×     | ×    |
| install   | ×           | ◯            | ×     | ×    |
| login     | ×           | ×            | ×     | ×    |
| new       | ×           | ×            | ×     | ×    |
| publish   | ◯           | △            | △     | △    |
| root      | ×           | ×            | ×     | ×    |
| run       | ◯           | ◯            | ◯     | ×    |
| search    | ×           | ×            | ×     | ×    |
| test      | ◯           | ◯            | ◯     | ◯    |
| uninstall | ×           | ◯            | ×     | ×    |
| update    | ×           | ◯            | ×     | ×    |

◯ ... 無ければエラー
△ ... あれば読み込む
× ... 完全に無視される


#### deps
depsは、dependenciesの略で依存関係という意味です。
現在のプロジェクトで扱いたいパッケージを指定します。
deps のバージョン指定には、以下の演算子が使用可能です。

等値:
* "0.1.0": 等しい

不等価演算子(`!=`等)の提供がないのは、一つだけのバージョンを拒否することがまず無いからです。
大概はあるバージョン以下を使用したく無い場合が多いため、それは以下の比較演算子で解決可能です。

比較演算子:
* ">=0.1.0": 以上
* "<=0.1.0": 以下
* ">0.1.0": より大きい
* "<0.1.0": より小さい

上に示す演算子は、この後に示す演算子より**優先順位が高い**です。
そして、比較演算子は必ずバージョンとの空白が無いようにする必要があります。

論理演算子:
* ">=0.1.0 and <0.3.0": 0.1.0以上でかつ0.3.0より小さい

論理演算子の場合のみ。左右に空白がないとパースエラーになります。

`or`の提供はありません。
なぜなら、`or`が必要になるような奇妙な依存性を持つ前に、もう一度見直すべきだからです。
それに加え、等値式と比較演算式を論理演算子で結合できません。
それもまた、必要のない依存性になるためです。

また、比較演算子を論理演算子で結合する場合、閉じた範囲にする必要があります。
```
←----┐           ┌-----→
-----◯-----------●------
   1.0.0       3.0.0
```
などは許されないということです。

(この辺りの話がピンとこない場合でも、実際に適当なバージョンを指定して`poac install`を実行すると全て教えてくれます。)

latest:
これを使用する時は最新のバージョンを使用したいがわからない時です。
```yaml
deps:
  poac: latest
```
poac installコマンド実行後に、自動的に存在するバージョンの範囲に置換されます。
```yaml
deps:
  poac: latest
```
置換規則は、command-specification.mdに示した、--outside option に書かれていることと同じです。

以上が提供される演算子です。


実際には以下のように扱います。

```yaml
deps:
  poac: ">=0.1.0 and <0.3.0"
```
以下のようにも書くことができます。
```yaml
deps:
  poac:
    version: ">=0.1.0 and <0.3.0"
```
この書き方はその下にビルド手順を記載したい時に使用できます。
基本的にビルドが必要な(ヘッダーオンリーでない)ライブラリは、指定していなくても、提供者側のpoac.ymlに記載されているので、
そちらでビルドできます。しかし、セキュリティ的に信頼できなかったり、一時的に自分の方法でビルドしたい時は、
```yaml
deps:
  boost/system:
    version: ">=1.60.0 and <1.70.0"
    build: poac
```
のように書くと、提供者側でなく、プロジェクトルートディレクトリに配置したpoac.ymlのビルド方法が優先して選択されます。
system: manualが将来的に実装されるとshellコマンドを直接使用することができますが、セキュリティやその他雑多な理由により実装は先延ばしになっています。

一般的に大量の依存関係をもつパッケージにおいて、等値式を用いることは現実的ではありません。
等値式を頻繁に用いると依存関係の解決に失敗する可能性があるからです。


##### Dependency from GitHub
```yaml
deps:
  github/:user/:package:
    tag: "v0.1.2"
    ...
```
のようにします。

header-only libraryをgithubからインストールする場合、buildが不要なので以下のように書けると便利ですが、0.1.0では未対応です。じきに修正されます。
```yaml
deps:
  github/:user/:package: "v0.1.2"
```

*srcがgithubの時に範囲指定しても動作しません。*


##### Dependencies environment (現状未実装)

dependenciesの種類の指定は、
```yaml
deps:
deps.dev:
deps.test:
```
なにも指定しないdepsは、全ての環境を示します。
内部的には、[ prod, dev, test ] の三つになります。
deps.prodは用意されていません。production時は、depsのみが選択されます。depsは、deps.prodのエイリアスではなく、全ての環境に置いてインストールされるdepsです。
deps.devは、開発時にのみインストールされます。つまり、基本的な開発時においては、depsとdeps.devがインストールされます。
<!-- 開発時というのは、そのプロジェクト単体で使用する時で、
そのパッケージが別のパッケージに依存される時には当てはまりません。 -->

<!-- 依存パッケージの依存パッケージ等は、depsのみが対応しています。
その理由は、2階以上のパッケージに対して、developやtestは行わないためです。 -->

下に幾つか例を挙げます。

```bash
$ POAC_ENV=prod poac install
```
とすると、depsのみがインストールされます。

以下のは、poac installのみと同じ意味になります。
つまり、環境変数を指定せずに`poac install`とすると、自動でPOAC_ENVはdevに設定されます。
```bash
$ POAC_ENV=dev poac install # poac installのみと同じ意味(semantics)になります
```
とすると、depsとdeps.devがインストールされます。

POAC_ENVという環境変数を設定すると、
その環境に設定されます。
指定可能なのは、上記した三つの環境です。

```bash
$ POAC_ENV=prod poac install
```


#### cpp_version
cpp_versionは、98, 03, 11, 14, 17, 20以外を許容しません。
そのため、cpp_versionを使用するコマンドでそれ以外を指定すると、エラーになります。
11は、include TR1です。

#### owners
ownersには、poac.pm にSignupしたのち、GitHubのUser IDを指定する必要があります。
publish時にtokenの所有者であるかどうかも検証されます。

<!-- TODO: buildや他のKEY -->

<!-- #### See Also
[setting-file.md] -->
