#!/usr/bin/env bash
# ci-baseline: the scope job. Decides the tier of this run and which jobs may
# report `skipped` on purpose. Anything unknown, any event but a pull request
# and any failure to list the changed files means tier `full` with no verification
# job skippable (only the push-only `macos`, which never runs on another event), so
# a filter can never turn the gate green over unverified work.
#
# Inputs, all through the environment (untrusted values never reach this shell
# inline), one entry per line where a list is expected:
#   EVENT           github.event_name
#   BASE_SHA        github.event.pull_request.base.sha (empty outside pull requests)
#   HEAD_SHA        github.event.pull_request.head.sha
#   REPOSITORY_OWNER github.repository_owner; an action of this owner is never
#                   a routine pin bump, and none is without it
#   INERT_PATHS     prefix globs (`docs/**`, `**/*.md`); a change made only of
#                   these is tier `docs`; `.github/workflows/**` is never inert;
#                   a negation is never evaluated and keeps `full`
#   DOCS_SKIPPABLE  job ids that may skip under tier `docs` (checks, lint, shards,
#                   contributed jobs that opted in)
#   APPS            `<id>=<path>` per app in monorepo mode; a changed path outside
#                   every app runs every app
#   PROTECTED_PATHS prefix globs whose change in a pull request needs the
#                   `human-approved` label; a protected path is never inert; a
#                   negation is never evaluated and protects nothing less; a
#                   change made only of action pins moved to the commits of
#                   their tags is routine and not listed (decision 2026-11-03)
#   GITHUB_OUTPUT   the step output file
# Outputs, written to $GITHUB_OUTPUT and printed on stdout as the same lines:
#   tier=full|docs
#   skippable=<JSON array of job ids>
#   protected=<JSON array of changed protected paths but the pin bumps>;
#             `["<unknown>"]` when a
#             pull request's files cannot be listed; `[]` outside pull requests
#   changed-<id>=true|false   one per app, in APPS order
set -euo pipefail

: "${GITHUB_OUTPUT:?GITHUB_OUTPUT is not set}"
event=${EVENT:-}
inert_paths=${INERT_PATHS:-}
docs_skippable=${DOCS_SKIPPABLE:-}
apps=${APPS:-}
protected_paths=${PROTECTED_PATHS:-}

emit() {
  printf '%s=%s\n' "$1" "$2" | tee -a "$GITHUB_OUTPUT"
}

# 1. List the changed files; only a pull request with both SHAs can. `diffed`
#    says the list was computed (possibly empty); `listed` that it is not empty.
#    The three-dot range compares the head with its merge base with the base, so
#    what the base branch changed after the branch point is never this pull
#    request's change (#1280); no merge base in the checkout fails the diff.
tier=full
diffed=false
listed=false
files=""
if [ "$event" = pull_request ] && [ -n "${BASE_SHA:-}" ] && [ -n "${HEAD_SHA:-}" ] \
  && files="$(git -c diff.renames=false -c core.quotePath=false diff --name-only "$BASE_SHA...$HEAD_SHA")"; then
  diffed=true
  [ -z "$files" ] || listed=true
fi

matches() {
  local glob
  while IFS= read -r glob; do
    case "$glob" in '' | '!'*) continue ;; esac
    # shellcheck disable=SC2254 # the glob must stay unquoted to act as a pattern
    case "$1" in $glob | ${glob#"**/"}) return 0 ;; esac
  done <<< "$2"
  return 1
}

# 2. The tier: docs only when every listed file matches an inert prefix glob.
negated=""
while IFS= read -r glob; do
  case "$glob" in '!'*) negated="$negated $glob" ;; esac
done <<< "$inert_paths"
if [ -n "$negated" ]; then
  echo "::warning::inert_paths carries a negation (${negated# }); negations are never evaluated, tier stays full"
fi

is_inert() {
  case "$1" in .github/workflows/*) return 1 ;; esac
  if matches "$1" "$protected_paths"; then
    return 1
  fi
  matches "$1" "$inert_paths"
}

if $listed && [ -z "$negated" ]; then
  tier=docs
  while IFS= read -r file; do
    if ! is_inert "$file"; then
      tier=full
      break
    fi
  done <<< "$files"
fi

# 3. The skippable list.
ids=""
# Each id once: an app's checks can be skippable both under docs and unchanged.
add() {
  case $'\n'"$ids" in *$'\n'"$1"$'\n'*) return 0 ;; esac
  ids="$ids$1"$'\n'
}

if [ "$event" = pull_request ]; then
  if $listed; then
    if [ "$tier" = docs ]; then
      while IFS= read -r id; do
        [ -n "$id" ] && add "$id"
      done <<< "$docs_skippable"
    fi
    workflow=false
    while IFS= read -r file; do
      case "$file" in .github/*) workflow=true ;; esac
    done <<< "$files"
    if ! $workflow; then
      add workflow-lint
    fi
  fi
fi
# The macOS job runs on pushes only (`if: github.event_name == 'push'`), so on any
# other event its skip is by design, whether or not the files could be listed
# (#784); it never stands in for a verification job.
if [ "$event" != push ]; then
  add macos
fi

# 4. Per-app change flags; an unlisted change set or a path outside every app
#    means every app changed.
changed_apps=""
outside=false
if $listed; then
  while IFS= read -r file; do
    owner=""
    while IFS= read -r app; do
      [ -n "$app" ] || continue
      id=${app%%=*}
      path=${app#*=}
      path=${path%/}
      case "$file" in "$path"/*) owner=$id; changed_apps="$changed_apps $id " ;; esac
    done <<< "$apps"
    if [ -z "$owner" ]; then
      outside=true
    fi
  done <<< "$files"
fi

flags=""
while IFS= read -r app; do
  [ -n "$app" ] || continue
  id=${app%%=*}
  changed=true
  if $listed && ! $outside; then
    case "$changed_apps" in *" $id "*) ;; *) changed=false; add "$id-checks" ;; esac
  fi
  flags="$flags$id=$changed"$'\n'
done <<< "$apps"

skippable="$(printf '%s' "$ids" | jq -R . | jq -cs .)"

# 5. The protected paths this pull request changed; unknown when it could not list.
negated=""
while IFS= read -r glob; do
  case "$glob" in '!'*) negated="$negated $glob" ;; esac
done <<< "$protected_paths"
if [ -n "$negated" ]; then
  echo "::warning::protected_paths carries a negation (${negated# }); negations are never evaluated and protect nothing less"
fi
# A pin line: `uses: <owner>/<repo>[/<path>]@<40-hex commit> # <release tag>`,
# blanks being spaces only: YAML starts a comment at ` #` and nowhere else.
pin_re='^( *(- )?use[s]: *)([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)(/[A-Za-z0-9_./-]+)?@([0-9a-f]{40}) +# *(v?[0-9]{1,9}(\.[0-9]{1,9})*) *$'

# True when the code host's tag `$2` of `$1` names the commit `$3`; an annotated
# tag counts by the commit it peels to. No answer is no.
is_tag_commit() {
  local refs sha ref direct="" peeled=""
  refs="$(GIT_TERMINAL_PROMPT=0 git ls-remote --tags "https://github.com/$1" "refs/tags/$2" "refs/tags/$2^{}" 2>/dev/null)" || return 1
  while IFS=$'\t' read -r sha ref; do
    [ "$ref" = "refs/tags/$2" ] && direct=$sha
    [ "$ref" = "refs/tags/$2^{}" ] && peeled=$sha
  done <<< "$refs"
  [ -z "$peeled" ] || direct=$peeled
  [ -n "$direct" ] && [ "$direct" = "$3" ]
}

# True when the release `$1` is strictly above the release `$2`, number by
# number (`v1.10` above `v1.9`; a missing number counts as 0).
is_newer_release() {
  local new=${1#v} old=${2#v} a b
  while [ -n "$new" ] || [ -n "$old" ]; do
    a=${new%%.*}
    b=${old%%.*}
    a=$((10#${a:-0}))
    b=$((10#${b:-0}))
    [ "$a" -gt "$b" ] && return 0
    [ "$a" -lt "$b" ] && return 1
    case "$new" in *.*) new=${new#*.} ;; *) new="" ;; esac
    case "$old" in *.*) old=${old#*.} ;; *) old="" ;; esac
  done
  return 1
}

# True when `$2` replaces the pin `$1` of an action with a pin of the same
# action, at the same place, on a newer release whose tag names that commit in
# the action's own repository. An action of this repository's owner never is:
# the owner can make its tags name anything.
is_pin_pair() {
  local LC_ALL=C prefix action old_tag owner self
  [ -n "${REPOSITORY_OWNER:-}" ] || return 1
  [[ $1 =~ $pin_re ]] || return 1
  prefix=${BASH_REMATCH[1]}
  action=${BASH_REMATCH[3]}${BASH_REMATCH[4]}
  old_tag=${BASH_REMATCH[6]}
  [[ $2 =~ $pin_re ]] || return 1
  [ "${BASH_REMATCH[1]}" = "$prefix" ] || return 1
  [ "${BASH_REMATCH[3]}${BASH_REMATCH[4]}" = "$action" ] || return 1
  owner="$(printf '%s' "${action%%/*}" | tr '[:upper:]' '[:lower:]')"
  self="$(printf '%s' "$REPOSITORY_OWNER" | tr '[:upper:]' '[:lower:]')"
  [ "$owner" != "$self" ] || return 1
  is_newer_release "${BASH_REMATCH[6]}" "$old_tag" || return 1
  is_tag_commit "${BASH_REMATCH[3]}" "${BASH_REMATCH[6]}" "${BASH_REMATCH[5]}"
}

# True when this pull request's change to the existing file `$1` is made only of
# pin pairs, each removed line followed in its hunk by the line replacing it. A
# new, deleted, renamed or mode-changed file, or any other line, is not a bump.
is_pin_bump() {
  local diff line pending="" old hunk=false
  diff="$(git -c diff.renames=false -c core.quotePath=false diff -U0 --no-color --no-ext-diff "$BASE_SHA...$HEAD_SHA" -- "$1")" || return 1
  [ -n "$diff" ] || return 1
  while IFS= read -r line; do
    if ! $hunk; then
      case "$line" in
        'diff --git '* | 'index '* | '--- a/'* | '+++ b/'*) continue ;;
        '@@ '*) hunk=true; continue ;;
        *) return 1 ;;
      esac
    fi
    case "$line" in
      '@@ '*) [ -z "$pending" ] || return 1 ;;
      -*) pending="$pending${line#-}"$'\n' ;;
      +*)
        [ -n "$pending" ] || return 1
        old=${pending%%$'\n'*}
        pending=${pending#*$'\n'}
        is_pin_pair "$old" "${line#+}" || return 1
        ;;
      *) return 1 ;;
    esac
  done <<< "$diff"
  $hunk && [ -z "$pending" ]
}

changed_protected=""
if [ "$event" = pull_request ] && ! $diffed; then
  changed_protected='<unknown>'$'\n'
elif $listed; then
  while IFS= read -r file; do
    if matches "$file" "$protected_paths" && ! is_pin_bump "$file"; then
      changed_protected="$changed_protected$file"$'\n'
    fi
  done <<< "$files"
fi
protected="$(printf '%s' "$changed_protected" | jq -R . | jq -cs .)"

# 6. The claim: the same lines the workflow reads and the doctor parses.
emit tier "$tier"
emit skippable "$skippable"
emit protected "$protected"
while IFS= read -r flag; do
  [ -n "$flag" ] || continue
  emit "changed-${flag%%=*}" "${flag#*=}"
done <<< "$flags"
