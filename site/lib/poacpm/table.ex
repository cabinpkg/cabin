defmodule Poacpm.Table do
  def token_nil(user) do
    %{
      id: user.id,
      name: user.name,
      token: nil,
      avatar_url: user.avatar_url,
      github_link: user.github_link,
      published_packages: user.published_packages
    }
  end
end