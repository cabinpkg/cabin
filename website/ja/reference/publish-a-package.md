200MB以上のパッケージはrejectされます。
(無料枠のみ？？？)

documentに、publishにおける条件を書く。
poac.ymlにnameのkeyがかかれ既に存在する他人のものと被らない
depsのsystemに、manualの指定が無いこと
depsのsrcに、githubの指定が無いこと
DパッケージならそれがDに依存（self depents）が無いこと
キチンと依存関係が解決できること。公開されているパッケージは全て依存関係が解決できることが保証されている。（未実装）
publish時に依存関係が正しいかチェックする？？？ -> そのパッケージが依存するパッケージが削除されたら意味がない
また、deps.devはチェックするのに、deps.testはチェックしないの？？という話になってしまう。


パッケージのガイドライン
Poac Package Guidelines
<!-- ここに、publishする時点での規則。
(名前とか、include/hello/hello.hppってできてるのかとか、
depsにmanual buildが含まれていないかどうかとか) -->

Cpp Core guildline に　Namingを補足したガイドラインを使用する。


実際にpublishする場合は、[contribute](../contribution/contribute-as-publisher.md) に詳細なガイドラインが書かれているため、確認してください。


.gitignoreを見て、そこに書かれたファイルはuploadされません。
もし、node_modulesのようなものが含まれていて、.gitignoreで
排除し忘れていたとしても、合計サイズが100MBを下回っていたら、大丈夫です。


ここに書くのは、
実際にpublishしてみよう！ってやつ。
(最終的には、`poac new`で、main.cppにnamespaceも生成してくれると嬉しい。但し、hello-worldとかは、hello_worldに変換する)

```bash
$ poac new $(git config user.name)/hello-world
```



poacは [Semantic Versioning](https://semver.org/lang/ja/) に則ってバージョン管理します。
そのため、semverに従っていないバージョンはrejectされます。




Organization自体は存在しない。
名前として、Organizationのように作れるだけである。
org/name: のみで、org1/org2/name 等の、slashが2つ以上だとエラーになる。
GitHub等でよくあるOrganizationと違うのは、アクセスの制限がない点だ。
つまり、matken11235でなくても、matken11235/hello とかいうパッケージが作成できる。
その点は今後の修正で改善される可能性がある。
現状、　Organizationを作成していないのは、初学者に気軽にパッケージを作成できる機会を与えたいから。
