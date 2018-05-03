defmodule PoacpmWeb.Router do
  use PoacpmWeb, :router

  pipeline :browser do
    plug(:accepts, ["html"])
    plug(:fetch_session)
    plug(:fetch_flash)
    plug(:protect_from_forgery)
    plug(:put_secure_browser_headers)
  end

  pipeline :api do
    plug(:accepts, ["json"])
  end


  scope "/api", PoacpmWeb.Api do
    pipe_through(:api)

    scope "/v1", V1 do
      get("/packages", PackagesController, :index)
    end
    # The reason why wild-card is placed here
    #  is to avoid matching subsequent wild-card.
    get("/*path", ErrorController, :index)
  end

  scope "/", PoacpmWeb do
    # Use the default browser stack
    pipe_through(:browser)

    get("/*path", PageController, :index)
  end
end
