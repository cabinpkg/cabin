#
# Deploy on k8s worker
#

FROM matken11235/phoenix:latest

ENV PORT 4000
EXPOSE ${PORT}

ADD . /service
WORKDIR /service

RUN mix deps.get --only prod
RUN MIX_ENV=prod mix compile

WORKDIR assets
RUN npm install
RUN npm run deploy

WORKDIR /service
RUN MIX_ENV=prod mix phx.digest
CMD PORT=4000 MIX_ENV=prod mix phx.server
