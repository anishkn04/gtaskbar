# Icons

## Interface icons are upstream GNOME icons, not ours

Every icon in the interface is an official `adwaita-icon-theme` icon,
referenced by name and resolved by GTK at runtime. Nothing is bundled. The full
set lives in `src/ui/icons.rs`:

| Purpose | Icon |
| --- | --- |
| Task list | `view-list-symbolic` |
| All tasks | `view-continuous-symbolic` |
| Today | `x-office-calendar-symbolic` |
| Next 7 days | `alarm-symbolic` |
| Overdue | `dialog-warning-symbolic` |
| Completed | `checkbox-symbolic` |
| Completion tick | `object-select-symbolic` |
| Quick add | `list-add-symbolic` |
| Sync | `view-refresh-symbolic` |
| Account / connect | `avatar-default-symbolic` |
| Menu | `open-menu-symbolic` |
| Close | `window-close-symbolic` |
| Preferences | `preferences-system-symbolic` |

This is deliberate, and it was learned the hard way. Two earlier revisions of
this app generated a bespoke monochrome set. That was wrong twice over:

- **It ignored the platform's icon theme.** The entire premise of libadwaita is
  that an app looks like it belongs to the desktop it runs on. Bundling artwork
  works against that: the icons can never pick up the user's theme, a
  high-contrast variant, or a custom icon set, and the app looks identical
  everywhere including places where the system theme would have looked better.
- **Hand-authored symbolic SVGs are fragile in ways that only show up at
  runtime.** GTK's handling of a `-symbolic` icon neither preserves
  `fill="none"` nor reads `currentColor`. Stroked outlines rendered as solid
  blobs, and thin-line icons resolved to black and vanished against a dark
  background. Both looked correct in every SVG viewer and in a contact sheet.
  The icons are not just better sourced upstream, they are *less* fragile, which
  matters more than the drawing ever was.

The tests in `src/ui/icons.rs` assert that every name is symbolic and that no
name is `gtaskbar`-namespaced, since a namespaced name would resolve to nothing
now that nothing is bundled, and fail silently as a blank space.

## The application icon

An application icon has to exist independently of any icon theme, so this is the
one piece of artwork the project ships. It is **not drawn here**.

- **Glyph:** Material Symbols `checklist` (outlined, 24px), taken from Google's
  official [`material-design-icons`](https://github.com/google/material-design-icons)
  repository, at
  `symbols/web/checklist/materialsymbolsoutlined/checklist_24px.svg`.
- **Licence:** Apache License 2.0. The verbatim upstream file is kept at
  `data/icons/upstream/material-symbols-checklist-24px.svg` and the licence text
  at `data/icons/upstream/LICENSE-Apache-2.0.txt`, so the provenance is
  checkable without a network round trip.
- **Changes made to it:** none to the path data. The only edit is a transform
  that re-centres the upstream `viewBox="0 -960 960 960"` onto a 128px artboard
  so the glyph fills a launcher slot. Both files carry a comment saying so.

A symbolic variant, `gtaskbar-symbolic.svg`, carries the same glyph for the tray
and for notification icons. It is drawn in the canonical symbolic grey
`#bebebe` rather than the upstream colour, because GTK remaps the luminance of a
symbolic icon to the current foreground — the same convention
`adwaita-icon-theme` uses.

## Requirements

The interface icons come from `adwaita-icon-theme`, which is a hard runtime
dependency and is pulled in by GTK itself. It is not listed in the dependency
notes beyond that because there is nothing to install beyond the toolkit.
