[![CircleCI](https://circleci.com/gh/poacpm/poac.pm.svg?style=shield)](https://circleci.com/gh/poacpm/poac.pm)
[![Language grade: JavaScript](https://img.shields.io/lgtm/grade/javascript/g/poacpm/poac.pm.svg?logo=lgtm&logoWidth=18)](https://lgtm.com/projects/g/poacpm/poac.pm/context:javascript)

## poac.pm

poac is the package manager and CLI application provided to the client.


### Links
> Official website:<br>
https://poac.io/

> Docs:<br>
https://docs.poac.io/


### Execute on local (hosting)

#### Install Node.js & Elm dependencies
```bash
$ cd hosting/
hosting/$ npm install
hosting/$ elm-package install
```

#### Start endpoint
```bash
hosting/$ npm run watch
$ firebase serve --only hosting
```

Now you can visit [`localhost:5000`](http://localhost:5000) from your browser.


### functions

```bash
$ firebase functions:config:set algolia.app_id="Application ID"
$ firebase functions:config:set algolia.search_key="Search-Only API Key"
$ firebase functions:config:set algolia.api_key="Admin API Key"
```

#### Start endpoint
```bash
$ firebase serve --only functions
```
