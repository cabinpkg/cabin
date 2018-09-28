# poacpm
[![CircleCI](https://circleci.com/gh/poacpm/poac.pm.svg?style=svg)](https://circleci.com/gh/poacpm/poac.pm)

poac is the package manager and CLI application provided to the client.


## Links
> Official website:<br>
https://poac.pm/

> Docs:<br>
https://poacpm.github.io/poac/


## Execute on local

#### Install dependencies
```bash
$ mix deps.get
```

#### Install Node.js dependencies
```bash
$ cd assets && npm install
```

#### Install Elm packages
```bash
$ cd assets/elm && elm-package install
```

#### Export required variables
```bash
$ export GOOGLE_APPLICATION_CREDENTIALS=/path/to/credential.json
```

#### Start Phoenix endpoint
```bash
$ mix phx.server
```

Now you can visit [`localhost:4000`](http://localhost:4000) from your browser.
