# Icons

All of gtaskbar's artwork is bundled in the binary as GResources. The app never
asks the system icon theme for an icon, so it looks the same on a bare Arch
install as on a fully themed desktop, and no icon can shift underneath us
because a distribution changed a theme.

## The visual language

Every symbolic icon follows the same rules. New icons must too, or the set
starts to look assembled rather than designed.

| Rule | Value |
| --- | --- |
| Grid | 16 × 16, `viewBox="0 0 16 16"` |
| Stroke | 1.5px, `fill="none"` |
| Caps and joins | `round` / `round` |
| Colour | `currentColor` only — never a hardcoded fill or stroke |
| Shapes | Outlines, no filled shapes or gradient plates |
| Fills | None, with the single exception of the app icon's background plate |

The shared attribute string is:

```xml
fill="none" stroke="currentColor" stroke-width="1.5"
stroke-linecap="round" stroke-linejoin="round"
```

## Why monochrome

Two reasons, both practical rather than aesthetic:

- **GTK recolours symbolic icons automatically.** Using `currentColor` means
  icons follow the foreground for hover, pressed, insensitive and
  destructive states, and follow the active theme in light and dark. A
  hardcoded colour breaks all of that.
- **A multi-colour emblem reads as an outsider.** The first version of this app
  used GNOME's `emblem-default-symbolic` for its connect state: a green badge
  with a white tick. Against a dark shell with a pink accent it looked like a
  sticker from another application. One icon in a monochrome set undoes the
  coherence of all the others.

## Naming

- Symbolic icons: `gtaskbar-<name>-symbolic.svg`
- The app icon: `gtaskbar.svg`

The names are namespaced on purpose. An unnamespaced name like `list-symbolic`
could collide with a system icon of the same name, and GTK would silently
substitute the theme's artwork for ours.

## Registering a new icon

Add the SVG, then add it to `data/ui/gtaskbar.gresource.xml`:

```xml
<file compressed="true" preprocess="xml-stripblanks"
      alias="gtaskbar-thing-symbolic.svg">../icons/symbolic/apps/gtaskbar-thing-symbolic.svg</file>
```

Then add a constant to `src/ui/icons.rs` and use it. Do not inline the string
literal at the call site; the module is the single place the set is defined, and
its tests assert that every name is namespaced, symbolic and unique.

`install.sh` also copies the symbolic set to
`~/.local/share/icons/hicolor/symbolic/apps/` so the desktop shell and other
applications can render the same icons. gtaskbar itself does not depend on those
copies.

## Reviewing the set

```sh
./tools/icon-sheet.sh
```

Writes `target/icon-sheet/sheet-{dark,light}.png`. Review new icons in the grid
rather than in isolation: mismatches in stroke weight and optical size that are
invisible in a single icon are obvious side by side.

Rendering twice is a real check, not decoration. A symbolic icon with a
hardcoded colour will look correct on one background and wrong on the other.
