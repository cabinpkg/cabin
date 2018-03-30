defmodule PoacpmWeb.ApiController do
  alias ExAws.Dynamo
  use PoacpmWeb, :controller

  def index(conn, _params) do
    # region = System.get_env("AWS_DEFAULT_REGION")
    #    ret = Dynamo.list_tables()
    #    IO.inspect(ret)
    {_resp, tables} = Dynamo.list_tables() |> ExAws.request()
    render(conn, "show.json", content: tables["TableNames"])
  end
end
