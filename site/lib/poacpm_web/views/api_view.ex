defmodule PoacpmWeb.ApiView do
  use PoacpmWeb, :view
  def render("show.json", %{content: content}) do
    %{content: content}
  end
end
