## poac graph

標準出力にdot形式でoutput
poac graph > hoge.dot
--output, -o でファイルを直接作成
poac graph -o hoge.dot でdot形式で出力
poac graph -o hoge.png でpng形式で出力
--input, -iで、入力する、poac.ymlファイルを指定. 指定しない場合はカレントディレクトリのを選択
poac graph -i ./deps/boost/poac.yml -o hoge.png
それ以外の拡張子を指定するとエラー
pngを指定時にgraphvisがインストールされていない（コマンドラインが使用できない）時はエラー
dot -Tpng test.dot -o test.png

lockファイルが存在すれば、lockファイルからactivatedに変換するための関数を呼び出し、activated_to_graphを呼ぶ
存在しなければ、resolver内の関数を呼び出して、それのResolved.activatedをactivated_to_graphに渡し、graphの生成と、ついでにlockファイルも作成しておく
