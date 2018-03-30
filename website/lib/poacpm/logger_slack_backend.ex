defmodule Poacpm.LoggerSlackBackend do
  @behaviour :gen_event

  def init({__MODULE__, name}) do
    {:ok, configure(name, [])}
  end

  def handle_call(_request, state) do
    {:ok, state}
  end

  def handle_event({level, _gl, {Logger, msg, ts, md}}, %{level: min_level} = state) do
    if (is_nil(min_level) or Logger.compare_levels(level, min_level) != :lt) do
      format_event(level, msg, ts, md, state)
      |> to_inlinecode
      |> SlackWebhook.async_send
    end
    {:ok, state}
  end

  defp format_event(level, msg, ts, md, %{format: format, metadata: keys}) do
    Logger.Formatter.format(format, level, msg, ts, take_metadata(md, keys))
    |> Enum.join
    |> String.replace("\"", "'")
  end
  defp to_inlinecode(str) do
    ~s(```#{str}```)
  end

  def handle_info(_msg, state) do
    {:ok, state}
  end

  defp take_metadata(metadata, keys) do
    metadatas = Enum.reduce(keys, [], fn key, acc ->
      case Keyword.fetch(metadata, key) do
        {:ok, val} -> [{key, val} | acc]
        :error     -> acc
      end
    end)
    Enum.reverse(metadatas)
  end

  def terminate(_reason, state) do
    {:ok, state}
  end


  @default_format "$time $metadata[$level] $message\n"
  defp configure(name, opts) do
    env = Application.get_env(:logger, name, [])
    opts = Keyword.merge(env, opts)
    Application.put_env(:logger, name, opts)

    level    = Keyword.get(opts, :level)
    format   = Keyword.get(opts, :format, @default_format) |> Logger.Formatter.compile
    metadata = Keyword.get(opts, :metadata, [])

    %{level: level, format: format, metadata: metadata}
  end
end
