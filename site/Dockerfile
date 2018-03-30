#
# on k8s worker
#

FROM poacpm/phoenix

ENV PORT 4000
EXPOSE ${PORT}

ADD . /service
WORKDIR /service

RUN mix deps.get
RUN mix compile

WORKDIR assets
RUN npm install
RUN node_modules/brunch/bin/brunch build

WORKDIR /service
RUN mix phx.digest
CMD mix phx.server
