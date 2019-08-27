## poac uninstall
updateでは、依存の依存を指定することができたが、uninstallではそうはいかない。
poac.ymlに書いたものしかダメ
poac.ymlとpoac.lockの両方が書き換えられるが、poac.ymlを書き換え、それのtimestampを使用して、poac.lockに書き込む

全ての依存関係が消えた時、poac.lockファイルが完全に削除されます。
また、poac.ymlからdepsキーが消えます。

現状、poac.ymlにコメントを書いていた場合、消えてしまいます。これは、他のpoac.ymlを書き換える処理が存在するコマンド全てに共通することで、
解決策を模索中です。
uninstallは、poac.ymlとpoac.lockの依存関係と見比べながら、他に依存されていないことを確認して削除してくれるコマンドですが、
先に、poac.ymlからそのパッケージを消してしまうと、依存関係が確認できないので、消すことはできません。
その時に活用するのが、cleanupコマンドです。

全ての依存をuninstallできる
