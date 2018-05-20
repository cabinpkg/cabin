defmodule Poacpm.ElasticSearch do
  @moduledoc false

  def template() do
    path = "/_template/template_1"
    payload = tmpl_payload() |> Tirexs.HTTP.encode()
    HTTPoison.post(Application.get_env(:poacpm, :es_url) <> path, payload, [{"Content-Type", "application/json"}])
  end

#  @spec suggest(String.t()) :: List.t(String.t())
  def suggest(word) do
    index = "package"
    path = "/#{index}/_search"
    payload = word |> create_payload() |> Tirexs.HTTP.encode()
    HTTPoison.post(Application.get_env(:poacpm, :es_url) <> path, payload, [{"Content-Type", "application/json"}])
  end

#  @spec create_payload(String.t()) :: String.t()
  defp tmpl_payload() do
    [
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
  end

#  @spec create_payload(String.t()) :: List.t()
  defp create_payload(word) do
#    "{
#      \"suggest\": {
#        \"my-suggestion\": {
#          \"prefix\": \"#{word}\",
#          \"completion\": {
#            \"field\": \"name\"
#          }
#        }
#      }
#    }"
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
