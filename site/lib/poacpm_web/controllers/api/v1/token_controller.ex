defmodule PoacpmWeb.Api.V1.TokenController do
  use PoacpmWeb, :controller
  import Plug.CSRFProtection, only: [get_csrf_token: 0]
  import Phoenix.Controller, only: [put_new_layout: 2, put_new_view: 2, text: 2]
#  import Plug.Conn, only: []

  @spec index(Plug.Conn.t(), any) :: Plug.Conn.t()
  def index(conn, _params) do
#    hoge = Plug.Conn.get_session(conn, :_csrf_token)
#
#    IO.inspect(hoge)
    text(conn, get_csrf_token())
  end
end
