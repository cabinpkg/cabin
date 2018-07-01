defmodule PoacpmWeb.Api.V1.PackagesController do
  use PoacpmWeb, :controller
  alias PoacpmWeb.Api.ErrorView
  alias Poacpm.ElasticSearch
  import Phoenix.Controller, only: [
    put_new_layout: 2,
    put_new_view: 2,
    json: 2
  ]


  @spec search(Plug.Conn.t(), map) :: Plug.Conn.t()
  def search(conn, %{"search" => word}) do
    response =
      word
      |> ElasticSearch.suggest()
      |> suggest_to_list()

    json(conn, %{"packages" => response})
  end
  def search(conn, _), do: json(conn, ErrorView.render("404.json"))

#  @spec show(Plug.Conn.t(), map) :: Plug.Conn.t()
#  def show(conn, %{"name" => name}) do
##    user = Dynamo.get_item("User", %{id: id})
##           |> ExAws.request!()
##           |> Dynamo.decode_item(as: User)
##    # そのユーザーでログインしてなければ，apikeyがnull
##    json(conn, user)
#  end
#
#  @spec create(Plug.Conn.t(), map) :: Plug.Conn.t()
#  def create(conn, %{"name" => package}) do
#    # FILE include
#    # * src/
#    # * poac.yml
#    # * README.md
#    # * LICENSE
#    # * test/ ... 可能ならば．(全てテストする)
#  end


  @spec suggest_to_list(map) :: list
  defp suggest_to_list(res) do
    res
    |> Map.fetch!("suggest")
    |> Map.fetch!("my-suggestion")
    |> Enum.at(0)
    |> Map.fetch!("options")
    |> Enum.flat_map(fn x -> [Map.fetch!(x, "_source")] end)
  end
end
