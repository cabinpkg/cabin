defmodule PoacpmWeb.Api.V1.PackagesController do
  use PoacpmWeb, :controller

  def index(conn, %{"search" => word}) do
    _res = Poacpm.ElasticSearch.template()
#    endpoint = "https://search-poacpm-2gud3ols5i62ko67jhgu2e3z4a.ap-northeast-1.es.amazonaws.com/package/_search"
#    body = "{
#      \"suggest\": {
#        \"my-suggestion\": {
#          \"prefix\": \"#{word}\",
#          \"completion\": {
#            \"field\": \"name\"
#          }
#        }
#      }
#    }"
#    {:ok, response} = HTTPoison.post(endpoint, body, [{"Content-Type", "application/json"}])
#    %HTTPoison.Response{body: res, headers: _, request_url: _, status_code: 200} = response
#    suggest = get_suggest(res)
#    json(conn, suggest)
    res = Poacpm.ElasticSearch.suggest(word)
    json(conn, %{"word" => res})
  end
  def index(conn, _), do: json(conn, PoacpmWeb.Api.ErrorView.render("404.json"))


  defp get_suggest(res) do
    res
    |> Poison.Parser.parse!()
    |> Map.fetch!("suggest")
    |> Map.fetch!("my-suggestion")
    |> Enum.at(0)
    |> Map.fetch!("options")
    |> Enum.flat_map(fn(x) -> [Map.fetch!(x, "text")] end)
  end
end
