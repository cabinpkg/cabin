defmodule Poacpm.ElasticSearch do
  @moduledoc false

  def template() do
    path = "/_template/template_1"
    payload = tmpl_payload() |> Tirexs.HTTP.encode()
    res = sign("POST", payload)
    HTTPoison.post(Application.get_env(:poacpm, :es_url) <> path, payload, res)
  end

#  @spec suggest(String.t()) :: List.t(String.t())
  def suggest(word) do
    index = "package"
    path = "/#{index}/_search"
    payload = word |> create_payload() |> Tirexs.HTTP.encode()
    res = sign("POST", payload)
    HTTPoison.post(Application.get_env(:poacpm, :es_url) <> path, payload, res)
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

#  @spec sign(String.t(), String.t(), String.t()) :: none()
  defp sign(method, payload) do
    config = Application.get_all_env(:poacpm)
    AWSAuth.sign_authorization_header(
      config[:access_key_id],
      config[:secret_access_key],
      method,
      config[:es_url],
      config[:region],
      "es",
      Map.new(),
      payload
    )
#    signed_request = AWSAuth.sign_url(
#      config[:access_key_id],
#      config[:secret_access_key],
#      method,
#      config[:es_url] <> path,
#      config[:region],
#      "es",
#      Map.new(),
#      DateTime.utc_now() |> DateTime.to_naive(),
#      payload
#    )
#    URI.parse(signed_request)
  end
end
