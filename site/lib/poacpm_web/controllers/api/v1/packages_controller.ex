defmodule PoacpmWeb.Api.V1.PackagesController do
  use PoacpmWeb, :controller
  alias PoacpmWeb.Api.ErrorView
  alias Poacpm.Util.Firestore
  alias Poacpm.Util.GCS
  import Phoenix.Controller, only: [
    put_new_layout: 2,
    put_new_view: 2,
    text: 2,
    json: 2
  ]


  # Check if package already exists
  defp _exists(name, version) do
    Firestore.get_docs_list("packages")
    |> Enum.find_value(false, fn x ->
         (name == x.fields["name"].stringValue) and
         (version == x.fields["version"].stringValue)
       end)
  end
  def exists(conn, %{"name" => name, "version" => version}) do
    res = (if _exists(name, version), do: "true", else: "false")
    text(conn, res)
  end
  def exists(conn, _), do: json(conn, ErrorView.render("404.json"))


  defp get_token_name(x) do
    # "projects/{}/databases/{}/documents/tokens/ABCDEFG"
    x.name
    |> String.split("/")
    |> List.last() # ABCDEFG
  end

  # Exists token and it owned by owners
  defp _validate(token, owners) do
    Firestore.get_docs_list("tokens")
    |> Enum.find_value(false, fn x ->
         (get_token_name(x) == token) and
         Enum.member?(owners, x.fields["owner"].stringValue)
       end)
  end

  def validate(conn, %{"token" => token, "owners" => owners}) do
    res = (if _validate(token, owners), do: "ok", else: "err")
    text(conn, res)
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


    owners = setting["owners"]
    name = setting["name"]
    version = setting["version"]
    # TODO: もし，存在していても，ownerが一致していれば，上書きができる(next version)
    if _validate(token, owners) and !_exists(name, version) do
      Firestore.create_doc(setting)
      # Upload to google cloud storage
      file = user_params["data"]
      GCS.upload_file(file.filename, file.path)
      text(conn, "ok")
    else
      text(conn, "err")
    end
  end
end
