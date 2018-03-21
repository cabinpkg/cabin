#
# Base environment
#

FROM ubuntu:xenial

ENV DEBIAN_FRONTEND noninteractive
RUN apt-get update -qq && apt-get install -y -qq \
      apt-utils \
      locales \
      openssl \
      libssl-dev \
      curl \
      git \
      whiptail \
      inotify-tools

# Locale
RUN locale-gen en_US.UTF-8
ENV LANG en_US.UTF-8
ENV LANGUAGE en_US:en
ENV LC_ALL en_US.UTF-8

# Node.js
RUN apt-get install -y -qq nodejs npm && \
    npm cache clean && \
    npm install -g n && \
    apt-get purge -y -qq nodejs npm && \
    n -q lts && \
    ln -sf /usr/local/bin/node /usr/bin/node && \
    ln -sf /usr/local/bin/npm /usr/bin/npm

# Erlang & Elixir
RUN curl -s https://packages.erlang-solutions.com/erlang-solutions_1.0_all.deb -o /tmp/erlang.deb && \
    dpkg -i /tmp/erlang.deb && \
    rm -rf /tmp/erlang.deb && \
    apt-get update -qq && \
    apt-get upgrade -y -qq && \
    apt-get install -y -qq \
      esl-erlang \
      elixir=1.6.0-1
    ## Show versions -> apt-cache show elixir

# Elm-Lang
RUN npm i -g elm --unsafe-perm=true --allow-root

# Phoenix framework
RUN mix local.hex --force && \
    mix local.rebar --force && \
    mix archive.install https://github.com/phoenixframework/archives/raw/master/phx_new.ez --force

# Clean
RUN apt-get clean && \
    rm -rf /var/lib/apt/lists/*
