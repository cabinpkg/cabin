defmodule PoacpmWeb.ApiView do
  def render("show.json", %{content: content}) do
    %{content: content}
  end
end
