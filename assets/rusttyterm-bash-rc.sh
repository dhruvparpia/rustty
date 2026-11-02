# rusttyterm bash integration (SHELLINT-013, issue #38).
# Loaded with `bash --rcfile <this file>`: it loads the user's own .bashrc
# first, then adds the OSC 133 markers and OSC 7 cwd reports.
[ -f ~/.bashrc ] && . ~/.bashrc

__rt_emitting=0

# Before each command: 133;B (accepted) + 133;C (first output expected).
__rt_preexec() {
  [ "$__rt_emitting" = 1 ] && return 0
  printf '\033]133;B\007\033]133;C\007'
}

# Before each prompt: 133;D:<exit> for the finished command, OSC 7 cwd,
# then 133;A (at prompt).
__rt_precmd() {
  local code=$?
  [ "$__rt_emitting" = 1 ] && return 0
  __rt_emitting=1
  printf '\033]133;D:%s\007' "$code"
  printf '\033]7;file://%s%s\007' "${COMPUTERNAME:-localhost}" "$(pwd -W 2>/dev/null || pwd)"
  printf '\033]133;A\007'
  __rt_emitting=0
}

trap '__rt_preexec' DEBUG
PROMPT_COMMAND="__rt_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
