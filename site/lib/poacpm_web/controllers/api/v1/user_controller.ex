defmodule PoacpmWeb.Api.V1.UserController do
  defmodule User do
    @derive [ExAws.Dynamo.Encodable]
    defstruct [:id, :name, :token, :avatar_url, :github_link, :published_packages]
  end
  defmodule Token do
    @derive [ExAws.Dynamo.Encodable]
    defstruct [:id, :name, :created_date, :last_used_date]
  end

  use PoacpmWeb, :controller
  alias ExAws.Dynamo
  import Plug.Conn, only: [get_session: 2]
  import Phoenix.Controller, only: [
    put_new_layout: 2,
    put_new_view: 2,
    json: 2
  ]


  @doc """
  Show a user currently logged in.
  """
  @spec index(Plug.Conn.t(), any) :: Plug.Conn.t()
  def index(conn, _params) do
    case check_login(conn) do
      %{"error": err} ->
        json(conn, %{"error": err})
      current_user ->
        case check_exists_user(current_user.id) do
          %{"error": err} ->
            json(conn, %{"error": err})
          user ->
            json(conn, user)
        end
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
               |> Poacpm.Table.token_nil()
    json(conn, get_user)
  end

  @doc """
  https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/API_UpdateItem_v20111205.html#API_UpdateItem_Examples
  """
  @spec update(Plug.Conn.t(), map) :: Plug.Conn.t()
  def update(conn, params) do
    case check_login(conn) do
      %{"error": err} ->
        json(conn, %{"error": err})
      _ ->
        case check_exists_user(params["id"]) do
          %{"error": err} ->
            json(conn, %{"error": err})
          current_user ->
            # TODO: I want to update published_packages.
            # Update `User`
            newUser = if current_user.token != params["token"] do
              value = params["token"] |> insert_now() |> Dynamo.Encoder.encode()
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
            conn
            # Do not put token in session.
            |> Plug.Conn.put_session(:current_user, response |> Poacpm.Table.token_nil())
            |> json(response)
        end
    end
  end

  @spec check_login(Plug.Conn.t()) :: map
  defp check_login(conn) do
    case get_session(conn, :current_user) do
      nil ->
        %{"error" => "Are you login?"}
      current_user ->
        current_user
    end
  end

  @spec check_exists_user(String.t()) :: map
  defp check_exists_user(userId) do
    current_user = Dynamo.get_item("User", %{id: userId})
                   |> ExAws.request!()
                   |> Dynamo.decode_item(as: User)
    if current_user.id === nil do
      %{"error" => "Could not find " <> userId}
    else
      current_user
    end
  end

  @spec insert_now(list) :: list
  defp insert_now(tokenList) do
    Enum.map(tokenList, fn x ->
      if x["created_date"] === "" do
        %{
          id: x["id"],
          name: x["name"],
          created_date: DateTime.utc_now() |> DateTime.to_string(),
          last_used_date: x["last_used_date"],
        }
      else
        x
      end
    end)
  end
end
