defmodule Poacpm.GitHub do
  @behaviour OAuth2.Strategy
  alias OAuth2.Client


  @spec config :: Keyword.t()
  defp config() do
    [
      strategy: __MODULE__,
      site: "https://api.github.com",
      authorize_url: "https://github.com/login/oauth/authorize",
      token_url: "https://github.com/login/oauth/access_token"
    ]
  end

  @spec client :: Client.t()
  def client() do
    Application.get_env(:poacpm, __MODULE__)
    |> Keyword.merge(config())
    |> Client.new()
  end

  @spec authorize_url!(list) :: binary
  def authorize_url!(params \\ []) do
    Client.authorize_url!(client(), params)
  end

  @spec get_token!(list) :: Client.t() | OAuth2.Error.t()
  def get_token!(params \\ []) do
    Client.get_token!(client(), Keyword.merge(params, client_secret: client().client_secret))
  end


  @impl OAuth2.Strategy
  @spec authorize_url(Client.t(), Client.params()) :: Client.t()
  def authorize_url(client, params) do
    OAuth2.Strategy.AuthCode.authorize_url(client, params)
  end

  @impl OAuth2.Strategy
  @spec get_token(Client.t(), Client.params(), Client.headers()) :: Client.t()
  def get_token(client, params, headers) do
    client
    |> Client.put_header("Accept", "application/json")
    |> OAuth2.Strategy.AuthCode.get_token(params, headers)
  end
end
