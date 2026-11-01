# VHS tape (https://github.com/charmbracelet/vhs) for the README demo GIF.

Output demo.gif

Require zsh
Require starship
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

Hide
Type "source ~/.zshrc && clear" Enter
Show

Type "cabin version" Sleep 500ms  Enter Sleep 1s
Type "cd examples/xxhash-usage" Sleep 500ms  Enter Sleep 1s

Type "bat --paging=never cabin.toml" Sleep 500ms  Enter Sleep 2s
Type "bat --paging=never src/main.c" Sleep 500ms  Enter Sleep 2s

Type "cabin run" Sleep 500ms  Enter Sleep 8s  # 8s for cold cache
Type "cabin tree" Sleep 500ms  Enter Sleep 8s
