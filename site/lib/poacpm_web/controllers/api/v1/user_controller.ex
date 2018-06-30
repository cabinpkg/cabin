defmodule PoacpmWeb.Api.V1.UserController do
  defmodule User do
    @derive [ExAws.Dynamo.Encodable]
    defstruct [:id, :name, :avatar, :apikey, :github, :published_packages]
  end

  use PoacpmWeb, :controller
  alias ExAws.Dynamo
  import Plug.Conn, only: [get_session: 2]
  import Phoenix.Controller, only: [
    put_new_layout: 2,
    put_new_view: 2,
    json: 2
  ]


  @spec index(Plug.Conn.t(), any) :: Plug.Conn.t()
  def index(conn, _) do
    case get_session(conn, :current_user) do
      nil ->
        json(conn, %{"error" => "Are you login?"})
      current_user ->
        json(conn, current_user)
    end
  end

  @spec show(Plug.Conn.t(), map) :: Plug.Conn.t()
  def show(conn, %{"id" => id}) do
    user = Dynamo.get_item("User", %{id: id})
           |> ExAws.request!()
           |> Dynamo.decode_item(as: User)
    # そのユーザーでログインしてなければ，apikeyがnull
    json(conn, user)
  end
end
