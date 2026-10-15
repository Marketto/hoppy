#!/usr/bin/env bash
# Turns a failed cargo log into GitHub annotations: agents can read annotations but not CI
# logs. Used by the Windows and macOS jobs in .github/workflows/ci.yml (Git Bash on
# windows-latest, bash on macos-latest) and by release.yml.
#
#   bash tools/ci-annotate.sh LABEL LOGFILE
#
# GitHub shows at most 10 error annotations per step, so this emits, in priority order:
#   - one annotation per failing test (up to 5): its whole "---- <test> stdout ----" block,
#     panic message included, anchored at the panic location when known;
#   - diagnostics ("path:line:col: error…" from --message-format=short, panics outside such
#     blocks, "error:" lines) until 9 annotations are used;
#   - 1 annotation with the last 40 log lines, passing "test … ok" lines left out
#     (newlines encoded as %0A).
set -u
label=$1
log=$2
max_blocks=5
max_block_lines=40
used=0

# Patterns and replacements live in variables: bash 3.2 (macOS /bin/bash) keeps the quotes
# of a quoted replacement literally.
pct='%' pct_esc='%25' colon=':' colon_esc='%3A' comma=',' comma_esc='%2C' cr=$'\r'

escape() { # workflow-command data escaping
  local s=${1//$pct/$pct_esc}
  s=${s//$cr/}
  printf '%s' "$s"
}

prop() { # workflow-command property escaping (title=, file=)
  local s
  s=$(escape "$1")
  s=${s//$colon/$colon_esc}
  printf '%s' "${s//$comma/$comma_esc}"
}

# "panicked at crates\x\y.rs:12:5:" or "path:12:5: error: …" → file line col (slashes fixed)
location() {
  if [[ $1 =~ ([A-Za-z0-9_./\\:-]+\.rs):([0-9]+):([0-9]+) ]]; then
    printf '%s %s %s' "${BASH_REMATCH[1]//\\//}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}"
  fi
}

emit() { # title body-already-escaped [location-source-line]
  local loc file line col
  loc=$(location "${3:-}")
  if [[ -n $loc ]]; then
    read -r file line col <<<"$loc"
    printf '::error file=%s,line=%s,col=%s,title=%s::%s\n' "$(prop "$file")" "$line" "$col" \
      "$(prop "$1")" "$2"
  else
    printf '::error title=%s::%s\n' "$(prop "$1")" "$2"
  fi
  used=$((used + 1))
}

# 1. Failing test blocks: "---- name stdout ----" up to the next "----", "failures:" or
#    "test result:" line.
blocks=0
name="" body="" panic="" n=0
flush() {
  if [[ -n $name && $blocks -lt $max_blocks ]]; then
    emit "$label: $name" "$body" "$panic"
    blocks=$((blocks + 1))
  fi
  name="" body="" panic="" n=0
}
while IFS= read -r line; do
  line=${line%$'\r'}
  if [[ $line =~ ^----\ (.+)\ stdout\ ----$ ]]; then
    flush
    name=${BASH_REMATCH[1]}
    continue
  fi
  [[ -z $name ]] && continue
  if [[ $line =~ ^----\  || $line == "failures:" || $line =~ ^test\ result: ]]; then
    flush
    continue
  fi
  [[ -z $panic && $line == *"panicked at"* ]] && panic=$line
  [[ $n -eq 0 && -z $line ]] && continue # leading blank line
  if ((n < max_block_lines)); then
    body+="$(escape "$line")%0A"
    n=$((n + 1))
  fi
done <"$log"
flush

# 2. Other diagnostics (panics already reported in a block are skipped).
grep -E '^[^ ]+:[0-9]+:[0-9]+: (error|warning)|panicked at|^error(\[|:)' "$log" |
  { if ((blocks > 0)); then grep -v 'panicked at'; else cat; fi; } |
  head -n $((9 - used)) |
  while IFS= read -r line; do
    line=${line%$'\r'}
    if [[ $line =~ ^([^:\ ]+):([0-9]+):([0-9]+):\ (.*)$ ]]; then
      file=${BASH_REMATCH[1]//\\//}
      printf '::error file=%s,line=%s,col=%s,title=%s::%s\n' "$(prop "$file")" "${BASH_REMATCH[2]}" \
        "${BASH_REMATCH[3]}" "$(prop "$label")" "$(escape "${BASH_REMATCH[4]}")"
    else
      printf '::error title=%s::%s\n' "$(prop "$label")" "$(escape "$line")"
    fi
  done

# 3. Log tail without the noise of passing tests.
tail_text=""
while IFS= read -r line; do
  tail_text+="$(escape "$line")%0A"
done < <(tr -d '\r' <"$log" | grep -vE '^test .* \.\.\. ok$' | tail -n 40)
printf '::error title=%s::%s\n' "$(prop "$label (last 40 log lines)")" "$tail_text"
