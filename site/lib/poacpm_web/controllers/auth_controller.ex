defmodule PoacpmWeb.AuthController do
  use PoacpmWeb, :controller
  alias ExAws.Dynamo
  alias PoacpmWeb.Api.V1.UserController.User
  import Phoenix.Controller, only: [
    put_new_layout: 2,
    put_new_view: 2,
    redirect: 2
  ]


  @spec index(Plug.Conn.t(), any) :: Plug.Conn.t()
  def index(conn, _params) do
    conn
    # TODO: Reconsider scope
    |> redirect(external: Poacpm.GitHub.authorize_url!(scope: "public_repo,read:org"))
    |> Plug.Conn.halt()
  end

  @doc """
  This action is reached via `/auth/callback` is the the callback URL that
  the OAuth2 provider will redirect the user back to with a `code` that will
  be used to request an access token. The access token will then be used to
  access protected resources on behalf of the user.
  """
  @spec callback(Plug.Conn.t(), map) :: Plug.Conn.t()
  def callback(conn, %{"code" => code}) do
    # Retrieve the token from the returned code OR session
    client = case Plug.Conn.get_session(conn, :access_token) do
      nil -> # Only the beginning
        client = Poacpm.GitHub.get_token!(code: code)
        Plug.Conn.put_session(conn, :access_token, client.token.access_token)
        client
      token ->
        access_token = OAuth2.AccessToken.new(%{"access_token" => token})
        %{Poacpm.GitHub.client() | headers: [], params: %{}, token: access_token}
    end
    # Request the user information acquisition API using the access token
    %{body: user} = OAuth2.Client.get!(client, "/user")
    # Save user information into DynamoDB
    parsed_user = put_dynamo(user)
    # Put user information into session and redirect to root page
    conn
    |> Plug.Conn.put_session(:current_user, parsed_user)
    |> redirect(to: "/")
    |> Plug.Conn.halt()
  end

  @doc """
  If there is no :current_user, it sends 404.
  """
  @spec delete(Plug.Conn.t(), any) :: Plug.Conn.t()
  def delete(conn, _param) do
    case Plug.Conn.get_session(conn, :current_user) do
      nil ->
        conn
        |> Plug.Conn.send_resp(404, "")
      _ ->
        conn
        |> Plug.Conn.delete_session(:current_user)
        |> Plug.Conn.send_resp(200, "")
    end
    |> Plug.Conn.halt()
  end


  @doc """
  If the user does not exist, create a new one.
  Otherwise update the changes except token and published_packages.
  Because do not exist in the value received from GitHub OAuth.
  Why update data every time you log in?
  1. I do not want to have data as much as possible.
  2. I want to synchronize with the data in the GitHub side.
     (After changing at GitHub side, login again, it will be changed.)
  """
  @spec put_dynamo(map) :: map
  defp put_dynamo(user) do
    current_user = Dynamo.get_item("User", %{id: user["login"]})
                   |> ExAws.request!()
                   |> Dynamo.decode_item(as: User)
    IO.inspect(current_user)
    if current_user.id != nil do
      new_user = create_user_for_update(user, current_user.token, current_user.published_packages)
      case get_diff(current_user |> to_enumerable(), new_user |> to_enumerable()) do
        :error ->
          new_user
        diff ->
          request = diff
                    |> to_update_item()
                    |> to_update_item_request()
          Dynamo.update_item("User", %{id: current_user.id}, request)
          |> ExAws.request!()
          |> Map.fetch!("Attributes")
          |> Dynamo.decode_item(as: User)
      end
    else
      put_user = create_user_for_update(user, nil, nil)
      Dynamo.put_item("User", put_user) |> ExAws.request!()
      put_user
    end
  end

#  @spec
  defp create_user_for_update(user, t, p) do
    %User{
      id: user["login"],
      name: user["name"],
      token: t,
      avatar_url: user["avatar_url"],
      github_link: user["html_url"],
      published_packages: p
    }
  end

  @spec to_enumerable(map) :: map
  defp to_enumerable(map), do: map |> Map.to_list() |> Enum.into(%{})

  @spec get_diff(map, map) :: list | atom
  defp get_diff(map1, map2) do
    case MapDiff.diff(map1 |> to_enumerable(), map2 |> to_enumerable()) do
      %{changed: :equal, value: _} ->
        :error
      val ->
        Enum.flat_map(val.value, fn {key, value} ->
          case Map.fetch(value, :added) do
            {:ok, val} -> %{key => val}
            :error -> []
          end
        end)
    end
  end

  @spec to_update_item(list) :: map
  defp to_update_item(items) do
    items
    |> Enum.map(fn {k, v} ->
      %{
        k => %{
          "Value" => Dynamo.Encoder.encode(v),
          "Action" => "PUT"
        }
      }
    end)
    |> Enum.map(fn x -> Map.to_list(x) end)
    |> List.flatten()
    |> Enum.into(%{})
  end

  @spec to_update_item_request(map) :: map
  defp to_update_item_request(item) do
    %{
      "AttributeUpdates" => item,
      "ReturnValues" => "ALL_NEW"
    }
  end
end
