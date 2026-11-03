# rustty bash integration (SHELLINT-013, issue #38).
# Loaded with `bash --rcfile <this file>`: it loads the user's own .bashrc
# first, then adds the OSC 133 markers and OSC 7 cwd reports.
[ -f ~/.bashrc ] && . ~/.bashrc

__rt_emitting=0

__rt_urlencode() {
  local s=$1 out= i c
  for ((i=0; i<${#s}; i++)); do
    c=${s:i:1}
    case $c in
      [A-Za-z0-9./:_-]) out+=$c ;;
      *) printf -v c '%%%02X' "'$c"; out+=$c ;;
    esac
  done
  printf '%s' "$out"
}

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
  local cwd=$(pwd -W 2>/dev/null || pwd)
  local enc=$(__rt_urlencode "$cwd")
  local sep
  case $cwd in /*) sep='' ;; *) sep='/' ;; esac
  printf '\033]7;file://%s%s%s\007' "${COMPUTERNAME:-localhost}" "$sep" "$enc"
  printf '\033]133;A\007'
  __rt_emitting=0
}

trap '__rt_preexec' DEBUG
PROMPT_COMMAND="__rt_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"