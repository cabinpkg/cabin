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


  def exists(conn, %{"name" => name, "version" => version}) do
    get_docs_list_from_firestore("packages")
    exst = get_docs_list_from_firestore("packages")
           |> Enum.find_value(false, fn x ->
                (name == x.fields["name"].stringValue) and
                (version == x.fields["version"].stringValue)
              end)
    if exst do
      text(conn, "true")
    else
      text(conn, "false")
    end
  end


  defp get_docs_list_from_firestore(collection_id) do
    {:ok, token} = Goth.Token.for_scope("https://www.googleapis.com/auth/cloud-platform")
    conn = GoogleApi.Firestore.V1beta1.Connection.new(token.token)

    {:ok, object} = GoogleApi.Firestore.V1beta1.Api.Projects.firestore_projects_databases_documents_list(
      conn,
      "projects/poac-pm/databases/(default)/documents",
      collection_id
    )
    object.documents
  end

  defp get_token_name(x) do
    # "projects/{}/databases/{}/documents/tokens/ABCDEFG"
    x.name
    |> String.split("/")
    |> List.last() # ABCDEFG
  end

  defp _validate(token, owners) do
    get_docs_list_from_firestore("tokens")
    |> Enum.find_value(false, fn x ->
         (get_token_name(x) == token) and
         Enum.member?(owners, x.fields["owner"].stringValue)
       end)
  end

  def validate(conn, %{"token" => token, "owners" => owners}) do
    if _validate(token, owners) do
      text(conn, "ok")
    else
      text(conn, "err")
    end
  end


  @doc """
  %GoogleApi.Firestore.V1beta1.Model.Value{
      arrayValue: nil,
      booleanValue: nil,
      bytesValue: nil,
      doubleValue: nil,
      geoPointValue: nil,
      integerValue: nil,
      mapValue: nil,
      nullValue: nil,
      referenceValue: nil,
      stringValue: nil,
      timestampValue: nil
    }
  """
  defp _typing(value) do
    case value do
      x when is_list(x) -> %{arrayValue: %{values: Enum.map(x, fn y -> _typing(y) end)}}
      x when is_map(x) -> %{mapValue: %{fields: Enum.map(x, fn {k, v} -> {k, _typing(v)} end) |> Enum.into(%{})}}
      x when is_binary(x) -> %{stringValue: x}
      x when is_integer(x) -> %{integerValue: x}
      x when is_float(x) -> %{doubleValue: x}
      x when is_boolean(x) -> %{booleanValue: x}
      _ -> %{nullValue: nil}
    end
  end
  defp typing(map) do
    map
    |> Enum.map(fn {k, v} -> {k, _typing(v)} end)
    |> Enum.into(%{})
  end

  def upload(conn, %{"user" => user_params}) do
    # Note: This file is temporary, and Plug will remove it
    #  from the directory as the request completes.
    # If we need to do anything with this file, we need to do it before then.
    # https://phoenixframework.org/blog/file-uploads
    setting = user_params["setting"]
              |> Poison.decode!()
              |> Map.get("setting")
              |> YamlElixir.read_from_string!()

    token = user_params["token"]
            |> Poison.decode!()
            |> Map.get("token")

    # Exists token and it owned by owners
    validation = get_docs_list_from_firestore("tokens")
                 |> Enum.find_value(false, fn x ->
                      (get_token_name(x) == token) and
                      Enum.member?(setting["owners"], x.fields["owner"].stringValue)
                    end)

    # Check if package already exists
    # TODO: もし，存在していても，ownerが一致していれば，上書きができる(next version)
    exists = get_docs_list_from_firestore("packages")
             |> Enum.find_value(false, fn x ->
                  setting["name"] == x.fields["name"].stringValue and
                  setting["version"] == x.fields["version"].stringValue
                end)

    if validation and !exists do
      # Write package info to firestore
      data = %{
        :createTime => nil,
        :fields => setting |> typing,
        :name => nil,
        :updateTime => nil
      }

      {:ok, token} = Goth.Token.for_scope("https://www.googleapis.com/auth/cloud-platform")
      conn_goth = GoogleApi.Firestore.V1beta1.Connection.new(token.token)
      {:ok, _object} = GoogleApi.Firestore.V1beta1.Api.Projects.firestore_projects_databases_documents_create_document(
        conn_goth,
        "projects/poac-pm/databases/(default)/documents",
        "packages",
        [body: data]
      )
      # Upload to google cloud storage
      upload_to_gcs(user_params["data"])
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
