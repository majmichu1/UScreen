#!/usr/bin/env bash
# Set — or, with --check, verify — the release version and date everywhere the
# website and the citation file repeat them:
#
#   docs/index.html   JSON-LD softwareVersion / datePublished / dateModified,
#                     the download button, the package names in the install
#                     table, the release and "last updated" dates
#   docs/llms.txt     "Current version: X (date)" and "Last verified: date"
#   docs/sitemap.xml  every <lastmod>
#   CITATION.cff      version and date-released
#
# Every pattern must match at least once, so a rewrite of one of those files
# that drops a marker makes this script fail instead of silently leaving an
# old version on the public page. publish-release.sh runs the --check form
# before it publishes anything.
#
#   scripts/update-release-metadata.sh 1.2.0 2026-09-15
#   scripts/update-release-metadata.sh --check 1.2.0 2026-09-15
set -euo pipefail
cd "$(dirname "$0")/.."

usage() { echo "usage: $0 [--check] <version> <YYYY-MM-DD>" >&2; exit 2; }
CHECK=0
if [ "${1:-}" = "--check" ]; then CHECK=1; shift; fi
[ $# -eq 2 ] || usage
VERSION="$1"; DATE="$2"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "!! version must look like 1.2.3, got '$VERSION'" >&2; exit 2; }
[[ "$DATE" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] && date -d "$DATE" >/dev/null 2>&1 \
  || { echo "!! date must be a real YYYY-MM-DD, got '$DATE'" >&2; exit 2; }

python3 - "$CHECK" "$VERSION" "$DATE" <<'PY'
import re, sys

check, version, date = sys.argv[1] == "1", sys.argv[2], sys.argv[3]
NUM = r"[0-9]+\.[0-9]+\.[0-9]+"
DAY = r"[0-9]{4}-[0-9]{2}-[0-9]{2}"

# file -> [(pattern, replacement)]; every pattern must hit at least once
RULES = {
    "docs/index.html": [
        (rf'"softwareVersion": "{NUM}"', f'"softwareVersion": "{version}"'),
        (rf'"datePublished": "{DAY}"', f'"datePublished": "{date}"'),
        (rf'"dateModified": "{DAY}"', f'"dateModified": "{date}"'),
        (rf"Download {NUM}</a>", f"Download {version}</a>"),
        (rf'releases/latest">{NUM}</a>', f'releases/latest">{version}</a>'),
        (rf'<time datetime="{DAY}">{DAY}</time>', f'<time datetime="{date}">{date}</time>'),
        (rf"uscreen_{NUM}_amd64\.deb", f"uscreen_{version}_amd64.deb"),
        (rf"uscreen-{NUM}-1\.x86_64\.rpm", f"uscreen-{version}-1.x86_64.rpm"),
        (rf"uscreen-{NUM}-PKGBUILD\.tar\.gz", f"uscreen-{version}-PKGBUILD.tar.gz"),
        (rf"uscreen-{NUM}-linux-x86_64\.tar\.gz", f"uscreen-{version}-linux-x86_64.tar.gz"),
    ],
    "docs/llms.txt": [
        (rf"Current version: {NUM} \({DAY}\)", f"Current version: {version} ({date})"),
        (rf"Last verified: {DAY}", f"Last verified: {date}"),
    ],
    "docs/sitemap.xml": [
        (rf"<lastmod>{DAY}</lastmod>", f"<lastmod>{date}</lastmod>"),
    ],
    "CITATION.cff": [
        (rf'^version: "{NUM}"', f'version: "{version}"'),
        (rf'^date-released: "{DAY}"', f'date-released: "{date}"'),
    ],
}

stale, broken = [], []
for path, rules in RULES.items():
    with open(path, encoding="utf-8") as f:
        original = f.read()
    text = original
    for pattern, repl in rules:
        text, n = re.subn(pattern, repl, text, flags=re.M)
        if n == 0:
            broken.append(f"{path}: no match for /{pattern}/")
    if text != original:
        stale.append(path)
        if not check:
            with open(path, "w", encoding="utf-8") as f:
                f.write(text)
            print(f"updated {path}")

if broken:
    print("!! metadata markers missing — the files were rewritten without them:", file=sys.stderr)
    for b in broken:
        print("   " + b, file=sys.stderr)
    sys.exit(1)
if check:
    if stale:
        print(f"!! release metadata is not {version} / {date} in: " + ", ".join(stale), file=sys.stderr)
        print(f"   run: scripts/update-release-metadata.sh {version} {date}", file=sys.stderr)
        sys.exit(1)
    print(f"release metadata is {version} / {date} everywhere.")
elif not stale:
    print(f"already {version} / {date} everywhere; nothing to do.")
PY
