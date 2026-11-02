# VHS tape (https://github.com/charmbracelet/vhs) for the website's
# demo.gif. Record it through `cargo demo-record`, which supplies the
# fresh session and checks it; the typed commands must match the
# xtask-demo `COMMANDS` list.

Output demo.gif
Output demo.txt

Require zsh
Require cabin
Require bat

Set Shell "zsh"
Set FontSize 32
Set FontFamily "FiraCode Nerd Font"
Set Width 1920
Set Height 1080
Set Padding 20
Set Margin 20
Set MarginFill "#000000"
Set BorderRadius 20
# Each Wait below returns once the prompt is back, or fails the
# recording after the timeout.
Set WaitPattern /❯\s*$/
Set WaitTimeout 2m

Hide
Type 'source "$DEMO_RC" && clear' Enter Wait
Show

Type "cabin version" Sleep 500ms Enter Wait Sleep 1s
Type "cd workspace-basic" Sleep 500ms Enter Wait Sleep 1s

Type "bat --paging=never packages/cli/cabin.toml" Sleep 500ms Enter Wait Sleep 3s

# Clearing between screenfuls keeps the terminal from scrolling: VHS's
# text capture, which `cargo demo-record` checks, reads the terminal
# from its first line, not the visible screen.
Ctrl+L Sleep 500ms
Type "bat --paging=never packages/cli/src/main.cc" Sleep 500ms Enter Wait Sleep 3s

Ctrl+L Sleep 500ms
Type "cabin run -p cli" Sleep 500ms Enter Wait Sleep 2s
Type "cabin tree -p cli" Sleep 500ms Enter Wait Sleep 3s
