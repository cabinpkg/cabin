<!-- ## Contribute as publisher

GitHubに公開しているパッケージを利用します．

elmと非常に似ているので，elmでパッケージの公開を行ったことがある人であれば簡単に感じるでしょう．

<https://github.com/elm-lang/elm-make/issues/71>
### 条件
*   リポジトリURLは最後に`.git`を入力する必要があります
*   Privateリポジトリは使用できません．
*   リポジトリはgithub.comそのドメイン名として持っている必要があります。
*   リポジトリは公開されていない可能性があるため、httpsではなくsshベースのURLを使用することをお勧めしますが、これも許可されていません。


GitHubのtagは，SemVerに則する必要があります．
boost-1.66.0とかは有効ではありません．
vも付かない，1.2.0等がベターです．



tagは，現在のブランチから推測されます．
1.2.0が最新だとして，最新の状態にされたmasterブランチにいるなら，1.2.0に推測されます．
過去のtagを公開したい時，`git checkout $TAG_NAME`するのがベターです．



とりあえず，poac publishしか対応しません．
それで，GitHubのlatestのバージョンをリリースできます．
将来的に，poac publish ${VERSION} で，バージョンを指定できるようにします．


owner情報は，githubのユーザー名になります．
これで，そのパッケージの権限管理を行います．GitHubに任せます．
tagを切らないと，新しいバージョンをリリースできませんが，そこのownerでなければ，tagをきれないので，
他人がpoac publishをしても，無視されます．
しかし，新しいtagを切ったまま，publishしていなければ，他人が，git cloneし，そのリポジトリ内で，poac publishすれば，公開されてしまいます．

### バージョン設計
バージョニングは [Semantic Versioning](https://semver.org/lang/ja/) に従ってください。
従っていない場合は、パッケージの公開時にエラーになります。

また、installコマンド実行時も、SemVerに従って依存関係の解決が行われます。(メジャーバージョンがどうとかは関係しませんが、betaやalpha、rcの解決順序には影響します。)



TODO: これを参考にする！！！！！！！！！！！！！！！！！！！！！！！！！！！！！！！！！！！！
<https://rust-lang-nursery.github.io/api-guidelines/about.html>
<https://rust-lang-nursery.github.io/api-guidelines/checklist.html> -->
