defmodule PoacpmWeb.PageController do
  use PoacpmWeb, :controller
  import Phoenix.Controller, only: [
    put_new_layout: 2,
    put_new_view: 2,
    render: 2
  ]

  @spec index(Plug.Conn.t(), any) :: Plug.Conn.t()
  def index(conn, _params) do
    render(conn, "index.html")
  end
end
