#
# Deploy on k8s worker
#

FROM poacpm/phoenix

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
CMD MIX_ENV=prod mix phx.server
