## poac cleanup
cleanupでは、poac.ymlから消しているのに、./depsに取り残されてしまっているパッケージを削除してくれます。
installコマンドのバックグラウンドで自動で実行されていますが、手動で実行することも可能です。
動作としては不要なものを消してくれる機能です。

Build Commandsとして置いていますが、`./deps`と`./_build`を綺麗にするコマンドです。
cleanupからcleanに変更するかもしれないです。
