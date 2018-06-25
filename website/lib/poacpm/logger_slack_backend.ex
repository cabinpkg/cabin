defmodule Poacpm.LoggerSlackBackend do
  @behaviour :gen_event

  @impl :gen_event
  @spec init(term) :: {:ok, GenEvent.state} | {:ok, GenEvent.state, :hibernate} | {:error, reason :: any()}
  def init({__MODULE__, name}) do
    {:ok, configure(name, [])}
  end

  @impl :gen_event
  @spec handle_call(term, term) :: {:ok, GenEvent.reply, GenEvent.new_state}
                                 | {:ok, GenEvent.reply, GenEvent.new_state, :hibernate}
                                 | {:remove_handler, GenEvent.reply}
  def handle_call(_request, state) do
    {:ok, state, []}
  end

  @impl :gen_event
  @spec handle_event(term, term) :: {:ok, GenEvent.new_state} | {:ok, GenEvent.new_state, :hibernate} | :remove_handler
  def handle_event({level, _gl, {Logger, msg, ts, md}}, %{level: min_level} = state) do
    if is_nil(min_level) or Logger.compare_levels(level, min_level) != :lt do
      level
      |> format_event(msg, ts, md, state)
      |> to_inlinecode
      |> SlackWebhook.async_send()
    end
    {:ok, state}
  end

#  @spec format_event({atom, atom} | [pattern | binary], Logger.level(), Logger.message(), map) :: String.t()
  defp format_event(level, msg, ts, md, %{format: format, metadata: keys}) do
    format
    |> Logger.Formatter.format(level, msg, ts, take_metadata(md, keys))
    |> Enum.join()
    |> String.replace("\"", "'")
  end

  @spec to_inlinecode(String.t()) :: String.t()
  defp to_inlinecode(str) do
    ~s(```#{str}```)
  end

  @impl :gen_event
  @spec handle_info(term, term) :: {:ok, GenEvent.new_state} | {:ok, GenEvent.new_state, :hibernate} | :remove_handler
  def handle_info(_msg, state) do
    {:ok, state}
  end

  defp take_metadata(metadata, keys) do
    metadatas =
      Enum.reduce(keys, [], fn key, acc ->
        case Keyword.fetch(metadata, key) do
          {:ok, val} -> [{key, val} | acc]
          :error     -> acc
        end
      end)
    Enum.reverse(metadatas)
  end

  @impl :gen_event
  @spec terminate(GenEvent.reason, term) :: term
  def terminate(_reason, state) do
    {:ok, state}
  end

  @default_format "$time $metadata[$level] $message\n"
  defp configure(name, opts) do
    env = Application.get_env(:logger, name, [])
    opts = Keyword.merge(env, opts)
    Application.put_env(:logger, name, opts)

    level = Keyword.get(opts, :level)
    format = opts |> Keyword.get(:format, @default_format) |> Logger.Formatter.compile()
    metadata = Keyword.get(opts, :metadata, [])

    %{level: level, format: format, metadata: metadata}
  end
end
