#
# Deploy on k8s worker
#

FROM poacpm/phoenix

ENV PORT 4000
EXPOSE ${PORT}

ADD . /service
WORKDIR /service

RUN mix deps.get
# --only prod
# MIX_ENV=prod
RUN mix compile

WORKDIR assets
RUN npm install
RUN npm run deploy

WORKDIR /service
RUN mix phx.digest
CMD MIX_ENV=prod mix phx.server
