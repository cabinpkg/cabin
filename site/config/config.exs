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

#config :poacpm,
#  access_key_id: System.get_env("AWS_ACCESS_KEY_ID"),
#  secret_access_key: System.get_env("AWS_SECRET_ACCESS_KEY"),
#  region: System.get_env("AWS_DEFAULT_REGION"),
#  es_url: System.get_env("AWS_ES_ENDPOINT")
#
#config :poacpm, Poacpm.GitHub,
#  client_id: System.get_env("GITHUB_CLIENT_ID"),
#  client_secret: System.get_env("GITHUB_CLIENT_SECRET"),
#  redirect_uri: System.get_env("GITHUB_REDIRECT_URI")
#
#config :ex_aws,
#  access_key_id: [{:system, "AWS_ACCESS_KEY_ID"}, :instance_role],
#  secret_access_key: [{:system, "AWS_SECRET_ACCESS_KEY"}, :instance_role],
#  region: System.get_env("AWS_DEFAULT_REGION")


# Import environment specific config. This must remain at the bottom
# of this file so it overrides the configuration defined above.
import_config "#{Mix.env()}.exs"
