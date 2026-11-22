#!/usr/bin/env bash
# Publish macOS artifacts (Relay-macOS.dmg, Relay-macOS.zip, and sha256 files)
# to a GitHub Release. Called by .github/workflows/macos.yml on main pushes
# (rolling prerelease `macos-latest`) and published GitHub releases.
set -euo pipefail

dir="${1:-dist}"
ref="${GITHUB_REF:-}"
sha="${GITHUB_SHA:-}"
repo="${GITHUB_REPOSITORY:-}"

files=(
  "$dir/Relay-macOS.dmg"
  "$dir/Relay-macOS.dmg.sha256"
  "$dir/Relay-macOS.zip"
  "$dir/Relay-macOS.zip.sha256"
)
for f in "${files[@]}"; do
  [[ -f "$f" ]] || { echo "publish-macos-release: missing $f" >&2; exit 1; }
done

if [[ "$ref" == refs/tags/* ]]; then
  # Existing published release: attach assets without clobbering author title/notes.
  tag="${ref#refs/tags/}"
  if gh release view "$tag" >/dev/null 2>&1; then
    gh release upload "$tag" "${files[@]}" --clobber
  else
    gh release create "$tag" "${files[@]}" \
      --title "Relay ${tag}" \
      --notes "macOS Apple Silicon build for ${tag} (commit ${sha:0:7}). Ad-hoc signed; right-click > Open on first launch." \
      --target "$sha"
  fi
else
  tag="macos-latest"
  title="Relay macOS (latest main)"
  notes="Rolling Apple Silicon build from main (commit ${sha:0:7}). Ad-hoc signed; right-click > Open on first launch."
  if gh release view "$tag" >/dev/null 2>&1; then
    if [[ -n "$repo" && -n "$sha" ]]; then
      gh api -X PATCH "repos/${repo}/git/refs/tags/${tag}" -f sha="$sha" -F force=true >/dev/null || true
    fi
    gh release edit "$tag" --title "$title" --notes "$notes" --target "$sha"
    gh release upload "$tag" "${files[@]}" --clobber
  else
    gh release create "$tag" "${files[@]}" --title "$title" --notes "$notes" --target "$sha" --prerelease
  fi
fi
