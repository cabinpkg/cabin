defmodule Poacpm.Util.Firestore do
  alias GoogleApi.Firestore.V1beta1.Api.Projects, as: Api


  @project_id "poac-pm"
  @database_id "(default)"


  defp create_connection() do
    {:ok, token} = Goth.Token.for_scope("https://www.googleapis.com/auth/cloud-platform")
    GoogleApi.Firestore.V1beta1.Connection.new(token.token)
  end

  def get_docs_list(collection_id) do
    {:ok, object} = Api.firestore_projects_databases_documents_list(
      create_connection(),
      "projects/#{@project_id}/databases/#{@database_id}/documents",
      collection_id
    )
    object.documents
  end

  def create_doc(body) do
    # Write package info to firestore
    data = %{
      :createTime => nil,
      :fields => body |> typing,
      :name => nil,
      :updateTime => nil
    }
    {:ok, _object} = Api.firestore_projects_databases_documents_create_document(
      create_connection(),
      "projects/#{@project_id}/databases/#{@database_id}/documents",
      "packages",
      [body: data]
    )
  end


  defp _typing(value) do
    case value do
      x when is_list(x) ->
        %{arrayValue:
          %{values:
            Enum.map(x, fn y ->
              _typing(y)
            end)
          }
        }
      x when is_map(x) ->
        %{mapValue:
          %{fields:
            Enum.map(x, fn {k, v} ->
              {k, _typing(v)}
            end)
            |> Enum.into(%{})
          }
        }
      x when is_binary(x) ->
        %{stringValue: x}
      x when is_integer(x) ->
        %{integerValue: x}
      x when is_float(x) ->
        %{doubleValue: x}
      x when is_boolean(x) ->
        %{booleanValue: x}
      _ ->
        %{nullValue: nil}
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
  def typing(map) do
    map
    |> Enum.map(fn {k, v} ->
         {k, _typing(v)}
       end)
    |> Enum.into(%{})
  end
end
