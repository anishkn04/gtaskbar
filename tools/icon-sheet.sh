#!/usr/bin/env bash
#
# Render a contact sheet of the icon set so it can be reviewed for consistency
# at a glance. Reviewing a new icon in isolation hides mismatches in stroke
# weight, optical size and cap style that are obvious in a grid.
#
# Each icon is rendered twice, once on a dark background and once on a light
# one. That is a deliberate check, not decoration: a symbolic icon that carries
# a baked-in colour instead of relying on currentColor will pass on one theme
# and fail on the other.
#
# Requires: rsvg-convert and imagemagick's montage.
#
# Usage: ./tools/icon-sheet.sh [output-dir]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
OUT_DIR="${1:-$REPO_ROOT/target/icon-sheet}"

SYMBOLIC_DIR="$REPO_ROOT/data/icons/symbolic/apps"
SCALABLE_DIR="$REPO_ROOT/data/icons/scalable/apps"

for tool in rsvg-convert montage; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool is required (install librsvg and imagemagick)" >&2
        exit 1
    fi
done

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

# Close to the app's own palette, so the sheet reflects reality rather than an
# idealised white background.
render_theme() {
    local name="$1" bg="$2" fg="$3"
    local tiles=()
    local count=0

    local source
    for source in "$SYMBOLIC_DIR"/*.svg "$SCALABLE_DIR"/*.svg; do
        [ -e "$source" ] || continue
        local stem
        stem="$(basename "$source" .svg)"
        local staged="$OUT_DIR/${stem}-${name}.svg"
        # Substitute the canonical symbolic grey for the sheet's foreground, so
        # the contact sheet shows what GTK would recolour it to.
        sed "s/#bebebe/${fg}/g" "$source" > "$staged"
        rsvg-convert -w 64 -h 64 "$staged" -o "$OUT_DIR/${stem}-${name}.png"
        tiles+=("$OUT_DIR/${stem}-${name}.png")
        count=$((count + 1))
    done

    montage "${tiles[@]}" \
        -tile 6x \
        -geometry 96x96+8+8 \
        -background "$bg" \
        "$OUT_DIR/sheet-${name}.png"
    echo "  $OUT_DIR/sheet-${name}.png  ($count icons)"
}

echo "Rendering icon contact sheets:"
render_theme dark "#181215" "#ecdfe5"
render_theme light "#fafafa" "#241f33"

echo
echo "Check each icon for: a consistent 1.5px stroke, round caps, no fills,"
echo "and no hardcoded colours."
