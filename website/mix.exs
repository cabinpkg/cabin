defmodule Poacpm.Mixfile do
  use Mix.Project

  def project do
    [
      app: :poacpm,
      version: "0.0.1",
      elixir: "~> 1.4",
      elixirc_paths: elixirc_paths(Mix.env()),
      compilers: [:phoenix, :gettext] ++ Mix.compilers(),
      dialyzer: [plt_add_deps: :transitive],
      start_permanent: Mix.env() == :prod,
      deps: deps()
    ]
  end

  # Configuration for the OTP application.
  #
  # Type `mix help compile.app` for more information.
  def application do
    [
      mod: {Poacpm.Application, []},
      extra_applications: [:logger, :runtime_tools]
    ]
  end

  # Specifies which paths to compile per environment.
  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_), do: ["lib"]

  # Specifies your project dependencies.
  #
  # Type `mix help deps` for examples and options.
  defp deps do
    [
      {:phoenix, "~> 1.3.2"},
      {:phoenix_pubsub, "~> 1.0"},
      {:phoenix_html, "~> 2.11.2"},
      {:phoenix_live_reload, "~> 1.1.5", only: :dev},
      {:cowboy, "~> 1.0"},
      {:junit_formatter, "~> 2.2", only: :test},
      {:credo, "~> 0.9.2", only: [:dev, :test], runtime: false},
      {:dialyxir, "~> 0.5.0", only: [:dev, :test], runtime: false},
      {:ex_aws_dynamo, "~> 2.0"},
      {:poison, "~> 3.0"},
      {:httpoison, "~> 1.2"},
      {:hackney, "~> 1.9"},
      {:aws_auth, "~> 0.7.1"},
      {:oauth2, "~> 0.9.2"},
      {:cors_plug, "~> 1.5"},
      {:timex, "~> 3.2"},
      {:logger_slack_backend, "~> 0.1.0", only: :dev}
    ]
  end
end
