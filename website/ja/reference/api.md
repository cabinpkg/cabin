### API

##### `GET` /packages/archive/:name/:version
##### `GET` /packages/archive/:org/:name/:version
パッケージの取得

##### `GET` /packages/deps/:name/:version
##### `GET` /packages/deps/:org/:name/:version
依存関係の取得

##### `GET` /packages/exists/:name/:version
##### `GET` /packages/exists/:org/:name/:version
そのパッケージが存在するか検証

##### `GET` /packages/readme/:name/:version
##### `GET` /packages/readme/:org/:name/:version
README.mdの内容を取得

##### `GET` /packages/versions/:name
##### `GET` /packages/versions/:org/:name
公開されているバージョン一覧を取得

##### `POST` /packages/upload
パッケージの公開

##### `POST` /tokens/validate
tokenの所有権等を検証
