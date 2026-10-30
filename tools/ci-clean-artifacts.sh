#!/bin/sh
# ci-clean-artifacts.sh <name> [--branch] - in a GitHub Actions run, deletes the artifacts with
# this name left by previous runs, so only the newest copy counts against the account's Actions
# storage quota. With --branch, only those of runs on the same branch (pull requests: the PR
# branch), so a PR build never deletes main's latest build.
# Artifacts of runs still in progress are kept (a later job of that run may still download them).
# Needs GH_TOKEN with "actions: write" (read-only on fork PRs: then nothing is deleted).
# Never fails the job: a cleanup error only prints a warning.
N=$1
[ -n "$N" ] && [ -n "$GITHUB_REPOSITORY" ] && [ -n "$GITHUB_RUN_ID" ] || { echo "usage: $0 <name> [--branch] (in Actions)"; exit 0; }
B=
[ "$2" = --branch ] && B=${GITHUB_HEAD_REF:-$GITHUB_REF_NAME}
gh api --paginate "repos/$GITHUB_REPOSITORY/actions/artifacts?name=$N&per_page=100" \
  -q ".artifacts[] | select(.workflow_run.id != $GITHUB_RUN_ID) | \"\(.id) \(.workflow_run.id) \(.size_in_bytes) \(.workflow_run.head_branch)\"" |
while read -r id run size branch; do
  [ -n "$B" ] && [ "$branch" != "$B" ] && continue
  st=$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$run" -q .status 2>/dev/null)
  if [ -n "$st" ] && [ "$st" != completed ]; then
    echo "kept $N ($id): run $run is $st"
    continue
  fi
  if gh api -X DELETE "repos/$GITHUB_REPOSITORY/actions/artifacts/$id" >/dev/null 2>&1; then
    echo "deleted $N ($id, $((size / 1048576)) MB)"
  else
    echo "::warning::artifact $N ($id) not deleted (token without actions: write?)"
  fi
done
exit 0
