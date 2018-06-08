# poacpm
![Build Status](https://codebuild.ap-northeast-1.amazonaws.com/badges?uuid=eyJlbmNyeXB0ZWREYXRhIjoiUHVPOUZFL1E4a3ltVk44eW1xbTY1WGhRSUNhbzFkRW9Xa2g0bjBDalN1OUhidEJVb0JzS1A1YnBUYnU3QitOSkxVV1d6NDhHVC94UlNDNEEwVGpFNjZFPSIsIml2UGFyYW1ldGVyU3BlYyI6IkpHOXc0L3U2aXp2M0FpN3AiLCJtYXRlcmlhbFNldFNlcmlhbCI6MX0%3D&branch=master)

poacpm is the package manager
To start your Phoenix server:

  * Install dependencies with `mix deps.get`
  * Install Node.js dependencies with `cd assets && npm install`
  * (If you use mac, exec `cd assets && npm i --unsafe-perm node-sass`)
  * Start Phoenix endpoint with `mix phx.server`

Now you can visit [`localhost:4000`](http://localhost:4000) from your browser.

Ready to run in production? Please [check our deployment guides](http://www.phoenixframework.org/docs/deployment).

## Learn more

  * Official website: http://www.phoenixframework.org/
  * Guides: http://phoenixframework.org/docs/overview
  * Docs: https://hexdocs.pm/phoenix
  * Mailing list: http://groups.google.com/group/phoenix-talk
  * Source: https://github.com/phoenixframework/phoenix

## CodeBuild
### buildspec.yml
https://docs.aws.amazon.com/codebuild/latest/userguide/build-env-ref-env-vars.html
https://docs.aws.amazon.com/codebuild/latest/userguide/build-env-ref-env-vars.html

## Local
```bash
$ export AWS_DEFAULT_REGION=$(cat ~/.aws/config | grep 'region' | awk '{printf $3}')
$ export AWS_ACCESS_KEY_ID=$(cat ~/.aws/credentials | grep 'aws_access_key_id' | awk '{printf $3}')
$ export AWS_SECRET_ACCESS_KEY=$(cat ~/.aws/credentials | grep 'aws_secret_access_key' | awk '{printf $3}')
$ mix phx.server
```
