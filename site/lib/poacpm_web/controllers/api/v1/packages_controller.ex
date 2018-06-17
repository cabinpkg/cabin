defmodule PoacpmWeb.Api.V1.PackagesController do
  use PoacpmWeb, :controller

  @spec index(any(), map()) :: any()
  def index(conn, %{"search" => word}) do
    response = word
               |> Poacpm.ElasticSearch.suggest()
               |> suggest_to_list()
    json(conn, %{"packages" => response})
  end
  def index(conn, _), do: json(conn, PoacpmWeb.Api.ErrorView.render("404.json"))


  @spec suggest_to_list(map()) :: list()
  defp suggest_to_list(res) do
    res
    |> Map.fetch!("suggest")
    |> Map.fetch!("my-suggestion")
    |> Enum.at(0)
    |> Map.fetch!("options")
    |> Enum.flat_map(fn(x) -> [Map.fetch!(x, "_source")] end)
  end
  #{
  #    "took": 1,
  #    "timed_out": false,
  #    "suggest": {
  #        "my-suggestion": [
  #            {
  #                "text": "hooo",
  #                "options": [
  #                    {
  #                        "text": "hoooo",
  #                        "_type": "information",
  #                        "_source": {
  #                            "name": "hoooo",
  #                            "date": "hoge"
  #                        },
  #                        "_score": 1,
  #                        "_index": "package",
  #                        "_id": "eh_7BWQBlsC6cB-83LYz"
  #                    },
  #                    {
  #                        "text": "hoooooooooooo",
  #                        "_type": "information",
  #                        "_source": {
  #                            "name": "hoooooooooooo",
  #                            "date": "2018"
  #                        },
  #                        "_score": 1,
  #                        "_index": "package",
  #                        "_id": "ex9RBmQBlsC6cB-8y7YE"
  #                    }
  #                ],
  #                "offset": 0,
  #                "length": 4
  #            }
  #        ]
  #    },
  #    "hits": {
  #        "total": 0,
  #        "max_score": 0,
  #        "hits": []
  #    },
  #    "_shards": {
  #        "total": 1,
  #        "successful": 1,
  #        "skipped": 0,
  #        "failed": 0
  #    }
  #}
end
