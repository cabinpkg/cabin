defmodule PoacpmWeb.Api.V1.DynamoController do
  alias ExAws.Dynamo
  use PoacpmWeb, :controller

  def index(conn, _params) do
    {_resp, tables} = Dynamo.list_tables() |> ExAws.request()
    json(conn, tables["TableNames"])
  end
end
