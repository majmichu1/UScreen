#!/usr/bin/env bash
# Source/tarball installs must also work with a read-only /usr (e.g. Bazzite).
# Package installs instead own /usr/share/libinput/99-uscreen-pen.quirks.
set -euo pipefail

usage() { echo "Usage: $0 {install|remove} [--root /staging/root]" >&2; exit 1; }
[[ $# == 1 || $# == 3 ]] || usage
action=$1
[[ $action == install || $action == remove ]] || usage
root=/
if [[ $# == 3 ]]; then
    [[ $2 == --root && $3 == /* ]] || usage
    root=$3
fi
[[ $root != / || $EUID == 0 ]] || { echo "Run with sudo to update libinput settings." >&2; exit 1; }

directory="${root%/}/etc/libinput"
destination="$directory/local-overrides.quirks"
source_file="$(cd -- "$(dirname -- "$0")/.." && pwd)/packaging/libinput/99-uscreen-pen.quirks"
begin='# BEGIN UScreen managed pen contact quirk'
end='# END UScreen managed pen contact quirk'
[[ $action != install || -r $source_file ]] || { echo "Missing $source_file" >&2; exit 1; }
[[ -e $destination || $action == install ]] || exit 0
mkdir -p "$directory"
# Do not replace a user's symlink or write through it.
[[ ! -L $destination ]] || { echo "Refusing to replace symlink: $destination" >&2; exit 1; }
exec 9<"$directory"
flock 9
temporary=$(mktemp "$directory/.uscreen-pen-quirk.XXXXXX")
trap 'rm -f "$temporary"' EXIT

if [[ $action == install ]]; then
    printf '%s\n' "$begin" > "$temporary"
    cat "$source_file" >> "$temporary"
    printf '%s\n' "$end" >> "$temporary"
fi
# Strip only our previous block. Put the replacement first so any explicit
# user overrides later in the file retain precedence. Reject damaged markers.
if [[ -e $destination ]]; then
    awk -v begin="$begin" -v end="$end" '
        $0 == begin { if (inside) exit 1; inside = 1; next }
        $0 == end { if (!inside) exit 1; inside = 0; next }
        !inside { print }
        END { if (inside) exit 1 }
    ' "$destination" >> "$temporary" || {
        echo "Damaged UScreen quirk markers; leaving $destination unchanged." >&2
        exit 1
    }
    chown --reference="$destination" "$temporary"
    chmod --reference="$destination" "$temporary"
else
    chmod 644 "$temporary"
fi
if [[ ! -s $temporary ]]; then
    rm -f "$destination"
else
    mv -f "$temporary" "$destination"
fi
echo "UScreen pen quirk updated. Log out and back in for libinput to reload it."
