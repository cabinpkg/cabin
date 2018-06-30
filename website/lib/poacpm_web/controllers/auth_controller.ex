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

  @spec delete(Plug.Conn.t(), any) :: Plug.Conn.t()
  def delete(conn, _param) do
    conn
    |> Plug.Conn.delete_session(:current_user)
    |> Plug.Conn.send_resp(:ok, "") # 200
    |> Plug.Conn.halt()
  end


  @spec put_dynamo(map) :: map
  defp put_dynamo(user) do
    get_user = Dynamo.get_item("User", %{id: user["login"]})
               |> ExAws.request!()
               |> Dynamo.decode_item(as: User)
    put_user = %User{
      id: user["login"],
      name: user["name"],
      avatar: user["avatar_url"],
      apikey: get_user.apikey,
      github: user["html_url"],
      published_packages: get_user.published_packages
    }
    Dynamo.put_item("User", put_user) |> ExAws.request!()
    put_user
  end
end
