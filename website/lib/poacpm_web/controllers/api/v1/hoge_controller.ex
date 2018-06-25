defmodule PoacpmWeb.Api.V1.HogeController do
  use PoacpmWeb, :controller
  import Phoenix.Controller, only: [put_new_layout: 2, put_new_view: 2, json: 2]

  @spec index(Plug.Conn.t(), any) :: Plug.Conn.t()
  def index(conn, _) do
    case get_session(conn, :current_user) do
      nil ->
        json(conn, %{"error" => "Are you login?"})
      user ->
        json(conn, user)
    end
  end
end
