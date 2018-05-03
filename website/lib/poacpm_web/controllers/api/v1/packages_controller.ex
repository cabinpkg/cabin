defmodule PoacpmWeb.Api.V1.PackagesController do
  alias ExAws.Dynamo
  use PoacpmWeb, :controller

  def index(conn, %{"search" => word}) do
    {_resp, tables} = Dynamo.list_tables() |> ExAws.request()
    json(conn, tables["TableNames"])
  end

  def index(conn, _) do
    json(conn, PoacpmWeb.Api.ErrorView.render("404.json", %{}))
  end
end
