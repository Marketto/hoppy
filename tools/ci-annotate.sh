#!/usr/bin/env bash
# Turns a failed cargo log into GitHub annotations: agents can read annotations but not CI
# logs. Used by the Windows job in .github/workflows/ci.yml (Git Bash on windows-latest).
#
#   bash tools/ci-annotate.sh LABEL LOGFILE
#
# GitHub shows at most 10 error annotations per step, so this emits:
#   - up to 9 diagnostics ("path:line:col: error…" from --message-format=short, panics,
#     "error:" lines), with file/line when known;
#   - 1 annotation with the last 40 log lines (newlines encoded as %0A).
set -u
label=$1
log=$2

escape() { # workflow-command data escaping
  local s=${1//'%'/'%25'}
  s=${s//$'\r'/}
  printf '%s' "$s"
}

grep -E '^[^ ]+:[0-9]+:[0-9]+: (error|warning)|panicked at|^error(\[|:)' "$log" | head -n 9 |
  while IFS= read -r line; do
    line=${line%$'\r'}
    if [[ $line =~ ^([^:\ ]+):([0-9]+):([0-9]+):\ (.*)$ ]]; then
      file=${BASH_REMATCH[1]//\\//}
      printf '::error file=%s,line=%s,col=%s,title=%s::%s\n' "$file" "${BASH_REMATCH[2]}" \
        "${BASH_REMATCH[3]}" "$label" "$(escape "${BASH_REMATCH[4]}")"
    else
      printf '::error title=%s::%s\n' "$label" "$(escape "$line")"
    fi
  done

tail_text=""
while IFS= read -r line; do
  tail_text+="$(escape "$line")%0A"
done < <(tail -n 40 "$log")
printf '::error title=%s (last 40 log lines)::%s\n' "$label" "$tail_text"
