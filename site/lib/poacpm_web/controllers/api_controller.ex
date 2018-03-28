defmodule PoacpmWeb.ApiController do
  alias  ExAws.Dynamo
  use PoacpmWeb, :controller

  def index(conn, _params) do
    # region = System.get_env("AWS_DEFAULT_REGION")
    content = Dynamo.get_item("packageinfo", "test")
      |> ExAws.request!
      |> Dynamo.decode_item(as: Content)
    render(conn, "show.json", content: content)
  end
end
