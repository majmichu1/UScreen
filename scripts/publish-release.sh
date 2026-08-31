#!/usr/bin/env bash
# Build and publish a GitHub release with the complete set of files.
#
# The set is checked before anything touches the network: a release with a
# file missing is exactly how the PKGBUILD got left out of 1.1.0, so this
# script would rather fail than publish half a release.
#
# Needs GH_TOKEN in the environment (never passed on a command line) and the
# uscreen-build container for portable binaries.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION="$(sed -n 's/^VERSION = //p' Makefile)"
REPO="majmichu1/UScreen"
: "${GH_TOKEN:?set GH_TOKEN first}"

[ -z "$(git status --porcelain)" ] || { echo "!! uncommitted changes — commit first"; exit 1; }

./scripts/build-release.sh
./packaging/build-packages.sh

# The complete set. Add here when a release gains a file; the check below
# keeps every future release honest about it.
ASSETS=(
  "dist/uscreen-$VERSION-linux-x86_64.tar.gz:application/gzip"
  "dist/uscreen_${VERSION}_amd64.deb:application/vnd.debian.binary-package"
  "dist/uscreen-$VERSION-1.x86_64.rpm:application/x-rpm"
  "dist/uscreen-$VERSION-PKGBUILD.tar.gz:application/gzip"
  "dist/uscreen-$VERSION/uscreen.apk:application/vnd.android.package-archive"
)
for a in "${ASSETS[@]}"; do
  f="${a%%:*}"
  [ -f "$f" ] || { echo "!! missing: $f — not publishing an incomplete release"; exit 1; }
done
echo "All $(( ${#ASSETS[@]} )) files present."

NOTES="${1:-}"
[ -n "$NOTES" ] && [ -f "$NOTES" ] || { echo "usage: $0 <release-notes.md>  (tag v$VERSION must exist on origin)"; exit 1; }

git rev-parse "v$VERSION" >/dev/null 2>&1 || { echo "!! tag v$VERSION does not exist — create and push it first"; exit 1; }

RID=$(python3 - "$NOTES" "$VERSION" <<'PY'
import json, sys, urllib.request
body = open(sys.argv[1]).read()
v = sys.argv[2]
req = urllib.request.Request(
    "https://api.github.com/repos/majmichu1/UScreen/releases",
    data=json.dumps({"tag_name": f"v{v}", "name": f"v{v}", "body": body}).encode(),
    headers={"Authorization": "Bearer " + __import__("os").environ["GH_TOKEN"],
             "Content-Type": "application/json"})
print(json.load(urllib.request.urlopen(req))["id"])
PY
)
echo "release id: $RID"

for a in "${ASSETS[@]}"; do
  f="${a%%:*}"; t="${a##*:}"; n="$(basename "$f")"
  curl -sS -X POST -H "Authorization: Bearer $GH_TOKEN" -H "Content-Type: $t" \
    --data-binary @"$f" \
    "https://uploads.github.com/repos/$REPO/releases/$RID/assets?name=$n" \
    | python3 -c "import json,sys; d=json.load(sys.stdin); print(' ', d.get('name'), d.get('state','?'))"
done
echo "✓ https://github.com/$REPO/releases/tag/v$VERSION"
