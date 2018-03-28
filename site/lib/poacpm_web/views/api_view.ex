defmodule PoacpmWeb.ApiView do
  def render("show.json", %{region: region}) do
    %{region: region}
  end
end
