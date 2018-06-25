defmodule PoacpmWeb.Api.V1.DynamoController do
  alias ExAws.Dynamo
  use PoacpmWeb, :controller
  import Phoenix.Controller, only: [put_new_layout: 2, put_new_view: 2, json: 2]

  @spec index(Plug.Conn.t(), any) :: Plug.Conn.t()
  def index(conn, _params) do
    {_resp, tables} = Dynamo.list_tables() |> ExAws.request()
    json(conn, tables["TableNames"])
  end
end
