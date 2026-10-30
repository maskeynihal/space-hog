#!/usr/bin/env bash
# Decides whether a push should produce a release, and which version.
#
# Looks at the commit messages in the push for one of:
#   upgrade(major)  upgrade(minor)  upgrade(patch)  upgrade(vX.Y.Z) / upgrade(X.Y.Z)
# An explicit version wins; otherwise the largest bump wins.
# The new version is computed from the latest vX.Y.Z tag (v0.0.0 if none).
#
# Usage: next-version.sh <before-sha> <after-sha>
# Prints key=value lines (release, version, tag), suitable for $GITHUB_OUTPUT.
set -euo pipefail

before="${1:-}"
after="${2:-HEAD}"

if [[ -n "$before" && ! "$before" =~ ^0+$ ]] && git cat-file -e "$before^{commit}" 2>/dev/null; then
  messages="$(git log --format=%B "$before..$after")"
else
  # New branch or unknown base: only look at the pushed commit.
  messages="$(git log -1 --format=%B "$after")"
fi

latest_tag="$(git tag --list 'v[0-9]*.[0-9]*.[0-9]*' --sort=-v:refname | head -n1)"
current="${latest_tag#v}"
current="${current:-0.0.0}"
IFS=. read -r major minor patch <<<"$current"

# Newest commit first, so take the first explicit version found.
explicit="$(grep -oiE 'upgrade\(v?[0-9]+\.[0-9]+\.[0-9]+\)' <<<"$messages" | head -n1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' || true)"

if [[ -n "$explicit" ]]; then
  next="$explicit"
  highest="$(printf '%s\n%s\n' "$current" "$next" | sort -V | tail -n1)"
  if [[ "$next" == "$current" || "$highest" != "$next" ]]; then
    echo "::error::upgrade(v$next) is not newer than the latest release v$current" >&2
    exit 1
  fi
elif grep -qi 'upgrade(major)' <<<"$messages"; then
  next="$((major + 1)).0.0"
elif grep -qi 'upgrade(minor)' <<<"$messages"; then
  next="$major.$((minor + 1)).0"
elif grep -qi 'upgrade(patch)' <<<"$messages"; then
  next="$major.$minor.$((patch + 1))"
else
  echo "release=false"
  exit 0
fi

if git rev-parse -q --verify "refs/tags/v$next" >/dev/null; then
  echo "::error::Tag v$next already exists" >&2
  exit 1
fi

echo "release=true"
echo "version=$next"
echo "tag=v$next"
