# The demo shell's setup, sourced by demo.tape and `cargo demo-check`.
PROMPT='%F{cyan}%~%f %(?.%F{green}.%F{red})❯%f '

# Each command the shell reads appends `<exit status> <command>` to
# $DEMO_LOG, which those checks compare with the expected session.
demo_preexec() { demo_command=$1 }
demo_precmd() {
  local st=$?
  if [[ -n $demo_command ]]; then
    print -r -- "$st $demo_command" >> "$DEMO_LOG"
  fi
  demo_command=
}
preexec_functions+=(demo_preexec)
precmd_functions+=(demo_precmd)
