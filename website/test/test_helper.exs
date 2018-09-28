# From https://github.com/victorolinasc/junit-formatter#readme
# 　If you want to keep using the default formatter alongside
#   the JUnitFormatter your test/test_helper.exs file should look like this:
ExUnit.configure(formatters: [JUnitFormatter, ExUnit.CLIFormatter])
# Exclude all external tests from running
ExUnit.configure exclude: [external: true]
ExUnit.start()
