defmodule Poacpm.Util.GCS do
  alias GoogleApi.Storage.V1.Api.Objects, as: Api

  @bucket_name "re.poac.pm"

  defp create_connection() do
    {:ok, token} = Goth.Token.for_scope("https://www.googleapis.com/auth/cloud-platform")
    GoogleApi.Storage.V1.Connection.new(token.token)
  end

  def upload_file(filename, filepath) do
    {:ok, _object} = Api.storage_objects_insert_simple(
      create_connection(),
      @bucket_name,
      "multipart",
      %{name: filename},
      filepath
    )
  end
end
