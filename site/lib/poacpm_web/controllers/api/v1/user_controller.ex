defmodule PoacpmWeb.Api.V1.UserController do
  defmodule User do
    @derive [ExAws.Dynamo.Encodable]
    defstruct [:id, :name, :token, :avatar_url, :github_link, :published_packages]
  end

  use PoacpmWeb, :controller
  alias ExAws.Dynamo
  import Plug.Conn, only: [get_session: 2]
  import Phoenix.Controller, only: [
    put_new_layout: 2,
    put_new_view: 2,
    json: 2
  ]


  @spec index(Plug.Conn.t(), any) :: Plug.Conn.t()
  def index(conn, _params) do
    case get_session(conn, :current_user) do
      nil ->
        json(conn, %{"error" => "Are you login?"})
      current_user ->
        json(conn, current_user)
    end
  end

  @doc """
  token is nil because it is not required to display user information.
  """
  @spec show(Plug.Conn.t(), map) :: Plug.Conn.t()
  def show(conn, %{"id" => id}) do
    get_user = Dynamo.get_item("User", %{id: id})
           |> ExAws.request!()
           |> Dynamo.decode_item(as: User)
    json(conn, %{
      id: get_user.id,
      name: get_user.name,
      token: nil,
      avatar_url: get_user.avatar_url,
      github_link: get_user.github_link,
      published_packages: get_user.published_packages
    })
  end

  @doc """
  https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/API_UpdateItem_v20111205.html#API_UpdateItem_Examples
  """
  @spec update(Plug.Conn.t(), map) :: Plug.Conn.t()
  def update(conn, params) do
    current_user = Dynamo.get_item("User", %{id: params["id"]})
           |> ExAws.request!()
           |> Dynamo.decode_item(as: User)
    if current_user.id == nil do
      json(conn, %{"error" => "Could not find " <> params["id"]})
    else
      # TODO: I want to update published_packages.
      # Update `User`
      newUser = if current_user.token != params["token"] do
        value = params["token"] |> Dynamo.Encoder.encode()
        %{
          "AttributeUpdates" => %{
            "token" => %{
              "Value" => value,
              "Action" => "PUT"
            }
          },
          "ReturnValues" => "ALL_NEW"
        }
      end
      response = Dynamo.update_item("User", %{id: params["id"]}, newUser)
                 |> ExAws.request!()
                 |> Map.fetch!("Attributes")
                 |> Dynamo.decode_item(as: User)
      json(conn, response)
    end
  end
end
