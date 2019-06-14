## プロジェクトをビルドする

```yaml
build:
  system: poac
```
と
```yaml
build: poac
```
は同じ意味に扱われます。
systemがcmakeであってもです。

systemに使用できるのは現状、poac, cmakeの二つになります。

poacの場合は、
```yaml
build:
  system: poac
  bin: true
  lib: true
```
等をつけると、バイナリやリンクライブラリの生成を制御できます。

poac以外の場合は、完全に無視されます。
cmakeであれば、CMakeLists.txtにそういった記述をしてください。


---
依存関係に対しても、同じように指定できます。
```yaml
github/jbeder/yaml-cpp:
    tag: yaml-cpp-0.6.2
    build: cmake
    link: static
```

依存先が、poacのライブラリで、そのライブラリ側に、build方法が記述してあったとしても、
依存元に記述したビルド手法が優先されます。

これは、セキュリティ的に危ない記述をしている場合に回避できたり、
開発者とは違うビルド方法を選択したい場合に使用できます。
