#
# on k8s worker
#

FROM poacpm/phoenix

ENV PORT 4000
EXPOSE ${PORT}
ENV MIX_ENV prod

ADD . /service
WORKDIR /service

RUN mix deps.get --only prod
RUN MIX_ENV=prod mix compile
RUN mix phx.digest

WORKDIR assets
RUN npm install
RUN node_modules/brunch/bin/brunch build --production

WORKDIR /service
CMD mix phx.server
