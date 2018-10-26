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


  # boost/config -> boost-config
  def orgToPkg(name) do
    name
    |> String.replace("/", "-")
  end

  @api_url "https://www.googleapis.com/storage/v1/b/re.poac.pm/o/"
  def getMd5Hash(filename) do
    HTTPoison.start()
    res = HTTPoison.get!(@api_url <> filename)
    case res do
      %{status_code: 200, body: body} ->
        body
        |> Poison.decode!()
        |> Map.get("md5Hash")
    end
  end

  def calc_file_size(path) do
    case File.stat(path) do
      {:ok, %{size: size}} -> size
      {:error, reason} -> reason # handle error
    end
  end

  @doc """
  IF >200MB, invalid
  Note: There is a possibility of difference depending on OS and file system.
  Ret: ture -> valid, false -> invalid
  """
  def check_file_size(size) do
    size < 200000000
  end


  @doc """
  Note: This file is temporary, and Plug will remove it
         from the directory as the request completes.
        If we need to do anything with this file,
         we need to do it before then.
  Ref: https://phoenixframework.org/blog/file-uploads
  """
  def upload(conn, %{"user" => user_params}) do
    file = user_params["data"]
    file_size = file.path
                |> calc_file_size()
    check_ret = file_size
                |> check_file_size()

    if check_ret do
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
        # Upload to google cloud storage
        GCS.upload_file(file.filename, file.path) # TODO: このfile.filenameを使用してしてしまうと，hackされる恐れが高まる

        # Wait pending... (10s)
        Process.sleep(10000)

        setting
        |> Map.put("object_name", file.filename)
        |> Firestore.create_doc()

        # Wait pending... (5s)
        Process.sleep(5000)

        text(conn, "ok")
      else
        text(conn, "err")
      end

    else
      text(conn, "err")
    end
  end
end
