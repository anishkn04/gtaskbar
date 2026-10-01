# Icons

All of gtaskbar's artwork is bundled in the binary as GResources. The app never
asks the system icon theme for an icon, so it looks the same on a bare Arch
install as on a fully themed desktop, and no icon can shift underneath us
because a distribution changed a theme.

## The icon set is generated, not hand-drawn

`data/icons/symbolic/apps/*.svg` is produced by `tools/gen-icons.py`. Edit the
generator, not the SVGs:

```sh
./tools/gen-icons.py     # regenerate
./tools/icon-sheet.sh    # render a contact sheet to review
```

## The constraints, and why they exist

Both of these cost real debugging time, because the icons render *correctly* in
`rsvg-convert` and in any SVG viewer, and only break inside GTK.

### 1. Drawn as filled geometry, never as strokes

GTK's handling of a `-symbolic` icon does not preserve `fill="none"`. An icon
drawn with strokes renders as a **solid blob**, no matter what the CSS cascade
says — `fill: none !important` in an embedded `<style>` does not save it.

Every icon in adwaita-icon-theme is filled geometry for the same reason. So
this set is too, and `gen-icons.py` provides filled equivalents of the shapes
that read better as outlines:

| Helper | Stands in for |
| --- | --- |
| `h_line` / `v_line` / `line` | a round-capped line, as a stadium |
| `ring` | a stroked circle, as an even-odd annulus |
| `frame` | a stroked rounded rectangle, as an even-odd frame |
| `triangle_outline` | a stroked triangle, as an even-odd frame |

The generator refuses to emit an icon containing `stroke=`, so this cannot
regress by accident.

### 2. Drawn in the canonical symbolic grey `#bebebe`

GTK recolours symbolic icons by **remapping luminance**. It does not read
`currentColor`: an SVG written that way resolves to black, and a thin stroke at
that luminance disappeared entirely against a dark background. The three-line
`all` and `list` icons were invisible for exactly this reason.

A single `#bebebe` in the source yields correct icons in light and dark, and in
every interactive state, because the remap handles it.

### 3. Monochrome, no emblems

A multi-colour emblem reads as an outsider. The first version of this app used
GNOME's `emblem-default-symbolic` for its connect state: a green badge with a
white tick. Against a dark shell with a pink accent it looked like a sticker
from another application. One icon in a monochrome set undoes the coherence of
all the others.

## Naming

- Symbolic icons: `gtaskbar-<name>-symbolic.svg`
- The app icon: `gtaskbar.svg`

Names are namespaced on purpose. An unnamespaced name like `list-symbolic`
could collide with a system icon of the same name, and GTK would silently
substitute the theme's artwork for ours.

The app icon is registered as `gtaskbar.svg`, not under the application id,
because GTK resolves a bundled icon by its file name in the resource. Asking
for `dev.anishkn04.gtaskbar` misses the bundle and falls back to a stock
placeholder.

## Registering a new icon

1. Add it to `ICONS` in `tools/gen-icons.py` and regenerate.
2. Add it to `data/ui/gtaskbar.gresource.xml`.
3. Add a constant to `src/ui/icons.rs`.

Do not inline the string literal at the call site. That module is the single
place the set is defined, and its tests assert that every name is namespaced,
symbolic and unique.

`install.sh` also copies the symbolic set to
`~/.local/share/icons/hicolor/symbolic/apps/` so the desktop shell and other
applications render the same icons. gtaskbar itself does not depend on those
copies.

## Reviewing the set

`./tools/icon-sheet.sh` writes `target/icon-sheet/sheet-{dark,light}.png`.
Review new icons in the grid rather than in isolation: mismatches in weight and
optical size that are invisible in a single icon are obvious side by side.

Rendering twice is a real check, not decoration. An icon carrying a colour other
than the canonical grey looks correct on one background and wrong on the other.

**A contact sheet is not sufficient on its own.** These two constraints are
only observable in the running app, so a set change must be confirmed there
before it is committed.
