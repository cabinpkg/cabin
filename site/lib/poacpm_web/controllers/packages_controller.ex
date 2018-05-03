defmodule PoacpmWeb.PackagesController do
  alias ExAws.Dynamo
  use PoacpmWeb, :controller

  def index(conn, %{"search" => word}) do
    IO.inspect(%{"search" => word})
    {_resp, tables} = Dynamo.list_tables() |> ExAws.request()
    json(conn, tables["TableNames"])
  end
end
