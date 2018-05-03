defmodule PoacpmWeb.Api.ErrorController do
  use PoacpmWeb, :controller

  def index(conn, _params) do
    render(conn, "404.json", [])
  end
end
