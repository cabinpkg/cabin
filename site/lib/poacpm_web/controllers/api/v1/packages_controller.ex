defmodule PoacpmWeb.Api.V1.PackagesController do
  use PoacpmWeb, :controller
  alias PoacpmWeb.Api.ErrorView
  import Phoenix.Controller, only: [
    put_new_layout: 2,
    put_new_view: 2,
    text: 2,
    json: 2
  ]


  @spec search(Plug.Conn.t(), map) :: Plug.Conn.t()
  def search(conn, %{"q" => word}) do
#    response =
#      word
#      |> ElasticSearch.suggest()
#      |> suggest_to_list()

#    json(conn, %{"packages" => response})
    json(conn, %{"packages" => word})
  end
  def search(conn, _), do: json(conn, ErrorView.render("404.json"))


  def validate(conn, %{"token" => token}) do
    # TODO: もし，tokenが存在していれば
    IO.inspect(token)
    text(conn, "ok")
  end

  def upload(conn, %{"user" => user_params}) do
    # Note: This file is temporary, and Plug will remove it
    #  from the directory as the request completes.
    # If we need to do anything with this file, we need to do it before then.
    # https://phoenixframework.org/blog/file-uploads
    user_params["info"]
    |> Poison.decode!()
    |> Map.get("setting")
    |> YamlElixir.read_from_string!()
    |> IO.inspect()

    # TODO: もう一度Tokenをチェック

    if upload = user_params["data"] do
      # Upload to google cloud storage
      upload_to_gcs(upload)
      json(conn, %{"status" => "ok"})
    else
      json(conn, %{"status" => "err"})
    end
  end

  defp upload_to_gcs(file) do
    # Authenticate
    {:ok, token} = Goth.Token.for_scope("https://www.googleapis.com/auth/cloud-platform")
    conn = GoogleApi.Storage.V1.Connection.new(token.token)

    # Make the API request
    {:ok, _object} = GoogleApi.Storage.V1.Api.Objects.storage_objects_insert_simple(
      conn,
      "re.poac.pm",
      "multipart",
      %{name: file.filename},
      file.path
    )
  end
end
