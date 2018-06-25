# poacpm
![Build Status](https://codebuild.ap-northeast-1.amazonaws.com/badges?uuid=eyJlbmNyeXB0ZWREYXRhIjoiZ1BXMDFFQXE1cUpHblJNZnlOSFEzWjVoYzFPYUJTSzVGMFJVdHNhemVpdS9hbFlJM2tFcjYvdFlLQURoeVhURjZaaW9yQk8yV1l6UTJabDRuNFRYSmxrPSIsIml2UGFyYW1ldGVyU3BlYyI6IjZSRG9SeENyZU40NmIzTm0iLCJtYXRlcmlhbFNldFNlcmlhbCI6MX0%3D&branch=master)
[![CircleCI](https://circleci.com/gh/poacpm/poacpm.svg?style=svg)](https://circleci.com/gh/poacpm/poacpm)

poacpm is the package manager and [poac](https://github.com/poacpm/poac) is the CLI application provided to the client.


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
$ export AWS_DEFAULT_REGION=$(cat ~/.aws/config | grep 'region' | awk '{printf $3}')
$ export AWS_ACCESS_KEY_ID=$(cat ~/.aws/credentials | grep 'aws_access_key_id' | awk '{printf $3}')
$ export AWS_SECRET_ACCESS_KEY=$(cat ~/.aws/credentials | grep 'aws_secret_access_key' | awk '{printf $3}')
$ export SLACK_WEBHOOK_URL='https://hooks.slack.com/services/AAAAAAA/BBBBBBBB/CCCCCCCCCCCCCCCCCCCC'
$ export GITHUB_CLIENT_ID='00000000000000000000'
$ export GITHUB_CLIENT_SECRET='0000000000000000000000000000000000000000'
$ export GITHUB_REDIRECT_URI='http://localhost:4000/auth/callback'
```

#### Start Phoenix endpoint
```bash
$ mix phx.server
```

Now you can visit [`localhost:4000`](http://localhost:4000) from your browser.
