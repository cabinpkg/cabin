defmodule PoacpmWeb.PageController do
  use PoacpmWeb, :controller

  def index(conn, _params) do
    render(conn, "index.html")
  end
end
