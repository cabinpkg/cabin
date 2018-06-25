defmodule PoacpmWeb.Router do
  use PoacpmWeb, :router

  pipeline :browser do
    plug(:accepts, ["html"])
    plug(:fetch_session)
    plug(:fetch_flash)
#    plug(:protect_from_forgery)
    plug(:put_secure_browser_headers)
    plug(:assign_current_user)
    plug(:assign_access_token)
  end

  pipeline :api do
    plug(:accepts, ["json"])
    plug(:fetch_session)
    plug(:assign_current_user)
    plug(:assign_access_token)
  end


  scope "/api", PoacpmWeb.Api do
    pipe_through(:api)

    scope "/v1", V1 do
      get("/packages", PackagesController, :index)
      get("/hoge", HogeController, :index)
      get("/token", TokenController, :index)
    end

    # The reason why wild-card is placed here
    #  is to avoid matching subsequent wild-card.
    get("/*path", ErrorController, :index)
  end

  scope "/auth", PoacpmWeb do
    pipe_through(:browser)
    get("/", AuthController, :index)
    get("/callback", AuthController, :callback)
    delete("/logout", AuthController, :delete)
  end

  scope "/", PoacpmWeb do
    pipe_through(:browser)
    get("/*path", PageController, :index)
  end


  defp assign_current_user(conn, _) do
    assign(conn, :current_user, get_session(conn, :current_user))
  end
  defp assign_access_token(conn, _) do
    assign(conn, :access_token, get_session(conn, :access_token))
  end
end
