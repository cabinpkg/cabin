# This file is responsible for configuring your application
# and its dependencies with the aid of the Mix.Config module.
#
# This configuration file is loaded before any dependency and
# is restricted to this project.
use Mix.Config

# Configures the endpoint
config :poacpm, PoacpmWeb.Endpoint,
  url: [host: "localhost"],
  secret_key_base: "wIFsQhsAoU8wBWnGpv1IO/larV+4Mx4L9zzrMDrsxIJO6TSIAf6MK2y+pKzYHj7+",
  render_errors: [view: PoacpmWeb.Api.ErrorView, accepts: ~w(json)],
  pubsub: [name: Poacpm.PubSub, adapter: Phoenix.PubSub.PG2]

# Import environment specific config. This must remain at the bottom
# of this file so it overrides the configuration defined above.
import_config "#{Mix.env()}.exs"
