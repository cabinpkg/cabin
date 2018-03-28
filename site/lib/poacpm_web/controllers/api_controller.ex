defmodule PoacpmWeb.ApiController do
  use PoacpmWeb, :controller

  def index(conn, _params) do
    region = System.get_env("AWS_DEFAULT_REGION")
    render(conn, "show.json", region: region)
  end
end
