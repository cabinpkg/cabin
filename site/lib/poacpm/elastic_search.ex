defmodule Poacpm.ElasticSearch do
  @moduledoc false

  @tmpl_body [
    index_patterns: ["package*"],
    settings: [
      number_of_shards: 1
    ],
    mappings: [
      information: [
        properties: [
          name: [
            type: "completion",
            analyzer: "simple",
            search_analyzer: "simple"
          ],
          date: [
            type: "text"
          ]
        ]
      ]
    ]
  ]
  def template() do
    endpoint = Application.get_env(:poacpm, :es_url)
    path = "/_template/template_1"
    body = @tmpl_body |> Tirexs.HTTP.encode()
    headers = [{"Content-Type", "application/json"}]
    {:ok, %HTTPoison.Response{body: return}} = HTTPoison.put(endpoint <> path, body, headers)
    Poison.decode!(return)
  end

  def delete() do
    endpoint = Application.get_env(:poacpm, :es_url)
    index = "package"
    path = "/#{index}"
    headers = [{"Content-Type", "application/json"}]
    {:ok, %HTTPoison.Response{body: return}} = HTTPoison.delete(endpoint <> path, headers)
    Poison.decode!(return)
  end

#  @spec suggest(String.t()) :: List.t(String.t())
  def suggest(word) do
    endpoint = Application.get_env(:poacpm, :es_url)
    index = "package"
    path = "/#{index}/_search"
    body = word |> create_payload() |> Tirexs.HTTP.encode()
    headers = [{"Content-Type", "application/json"}]
    {:ok, %HTTPoison.Response{body: return}} = HTTPoison.post(endpoint <> path, body, headers)
    Poison.decode!(return)
  end

#  @spec create_payload(String.t()) :: List.t()
  defp create_payload(word) do
    [
      suggest: [
        "my-suggestion": [
          prefix: "#{word}",
          completion: [
            field: "name"
          ]
        ]
      ]
    ]
  end
end
