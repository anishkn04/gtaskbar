#!/usr/bin/env python3
"""Generate the gtaskbar icon set.

Why a generator, and why filled geometry:

GTK's handling of a `-symbolic` icon does not preserve `fill="none"`. An icon
drawn as strokes renders as a solid blob, whatever the CSS cascade says — this
cost several iterations to diagnose, because the icons *looked* fine in
rsvg-convert and in any SVG viewer, and only broke inside GTK. Every icon in
adwaita-icon-theme is drawn as filled geometry for the same reason, so this set
is too: a stroked outline is not a thing GTK symbolic icons can express.

Each primitive below therefore returns a *filled* path that looks like the
stroked equivalent:

- `h_line` / `v_line`  a round-capped line, as a stadium
- `ring`              a stroked circle, as an annulus (even-odd fill)
- `frame`             a stroked rounded rectangle, as an even-odd frame
- `poly`              a stroked polygon outline, as an even-odd frame

Colour is the canonical symbolic grey `#bebebe`; GTK remaps luminance, so a
single grey yields correct icons in light and dark and in every interactive
state. `currentColor` does not work: it resolves to black, and a thin stroke at
that luminance disappears against a dark background.

Run `./tools/gen-icons.py` after editing. Check the result with
`./tools/icon-sheet.sh` and in the running app.
"""

from pathlib import Path
import sys

OUT = Path(__file__).resolve().parent.parent / "data/icons/symbolic/apps"

GREY = "#bebebe"
EVEN_ODD = 'fill-rule="evenodd"'


def _n(value: float) -> str:
    """Trim float noise so the generated files diff cleanly."""
    return f"{value:g}"


def h_line(x1: float, x2: float, y: float, w: float = 1.5) -> str:
    """A horizontal line with round caps, as a filled stadium."""
    r = w / 2
    left, right = (x1, x2) if x1 <= x2 else (x2, x1)
    return (
        f'<path fill="{GREY}" d="M{_n(left)},{_n(y - r)}'
        f'h{_n(right - left)}'
        f'a{_n(r)},{_n(r)} 0 0 1 0,{_n(w)}'
        f'h-{_n(right - left)}'
        f'a{_n(r)},{_n(r)} 0 0 1 0,-{_n(w)}Z"/>'
    )


def v_line(y1: float, y2: float, x: float, w: float = 1.5) -> str:
    """A vertical line with round caps, as a filled stadium."""
    r = w / 2
    top, bottom = (y1, y2) if y1 <= y2 else (y2, y1)
    return (
        f'<path fill="{GREY}" d="M{_n(x - r)},{_n(top)}'
        f'v{_n(bottom - top)}'
        f'a{_n(r)},{_n(r)} 0 0 0 {_n(w)},0'
        f'v-{_n(bottom - top)}'
        f'a{_n(r)},{_n(r)} 0 0 0 -{_n(w)},0Z"/>'
    )


def line(x1: float, y1: float, x2: float, y2: float, w: float = 1.5) -> str:
    """Any line, as a filled stadium. Horizontal and vertical take the fast path."""
    if abs(y1 - y2) < 1e-9:
        return h_line(x1, x2, y1, w)
    if abs(x1 - x2) < 1e-9:
        return v_line(y1, y2, x1, w)

    # Diagonal: a stadium is a rectangle plus two end caps, expressed as the
    # convex hull of the two circles and the swept quad.
    import math

    dx, dy = x2 - x1, y2 - y1
    length = math.hypot(dx, dy)
    ux, uy = dx / length, dy / length
    px, py = -uy * w / 2, ux * w / 2
    r = w / 2
    return (
        f'<path fill="{GREY}" d="M{_n(x1 + px)},{_n(y1 + py)}'
        f'L{_n(x2 + px)},{_n(y2 + py)}'
        f'A{_n(r)},{_n(r)} 0 0 0 {_n(x2 - px)},{_n(y2 - py)}'
        f'L{_n(x1 - px)},{_n(y1 - py)}'
        f'A{_n(r)},{_n(r)} 0 0 0 {_n(x1 + px)},{_n(y1 + py)}Z"/>'
    )


def ring(cx: float, cy: float, radius: float, w: float = 1.5) -> str:
    """A stroked circle, as a filled annulus."""
    outer, inner = radius, radius - w
    return (
        f'<path fill="{GREY}" {EVEN_ODD} d="'
        f"M{_n(cx - outer)},{_n(cy)}"
        f"a{_n(outer)},{_n(outer)} 0 1 0 {_n(outer * 2)},0"
        f"a{_n(outer)},{_n(outer)} 0 1 0 -{_n(outer * 2)},0Z"
        f"M{_n(cx - inner)},{_n(cy)}"
        f"a{_n(inner)},{_n(inner)} 0 1 1 {_n(inner * 2)},0"
        f"a{_n(inner)},{_n(inner)} 0 1 1 -{_n(inner * 2)},0Z\"/>"
    )


def disc(cx: float, cy: float, radius: float) -> str:
    return f'<circle cx="{_n(cx)}" cy="{_n(cy)}" r="{_n(radius)}" fill="{GREY}"/>'


def frame(
    x: float, y: float, w: float, h: float, radius: float = 2, thickness: float = 1.5
) -> str:
    """A stroked rounded rectangle, as an even-odd frame."""
    inner = max(0.1, radius - thickness)
    ix, iy = x + thickness, y + thickness
    iw, ih = w - thickness * 2, h - thickness * 2

    def rounded(px: float, py: float, pw: float, ph: float, pr: float) -> str:
        return (
            f"M{_n(px + pr)},{_n(py)}"
            f"h{_n(pw - pr * 2)}a{_n(pr)},{_n(pr)} 0 0 1 {_n(pr)},{_n(pr)}"
            f"v{_n(ph - pr * 2)}a{_n(pr)},{_n(pr)} 0 0 1 -{_n(pr)},{_n(pr)}"
            f"h-{_n(pw - pr * 2)}a{_n(pr)},{_n(pr)} 0 0 1 -{_n(pr)},-{_n(pr)}"
            f"v-{_n(ph - pr * 2)}a{_n(pr)},{_n(pr)} 0 0 1 {_n(pr)},-{_n(pr)}Z"
        )

    return (
        f'<path fill="{GREY}" {EVEN_ODD} d="'
        f"{rounded(x, y, w, h, radius)}"
        f"{rounded(ix, iy, iw, ih, inner)}Z\"/>"
    )


def poly(points: list[tuple[float, float]]) -> str:
    """A convex polygon, as a filled shape."""
    head = f"M{_n(points[0][0])},{_n(points[0][1])}"
    rest = "".join(f"L{_n(px)},{_n(py)}" for px, py in points[1:])
    return f'<path fill="{GREY}" d="{head}{rest}Z"/>'


def triangle_outline(points: list[tuple[float, float]], thickness: float = 1.5) -> str:
    """A stroked triangle. The inner edge is inset toward the centroid."""
    cx = sum(px for px, _ in points) / len(points)
    cy = sum(py for _, py in points) / len(points)
    scale = thickness / 2
    inner = [(cx + (px - cx) * scale, cy + (py - cy) * scale) for px, py in points]
    head = f"M{_n(points[0][0])},{_n(points[0][1])}"
    outer = head + "".join(f"L{_n(px)},{_n(py)}" for px, py in points[1:])
    ihead = f"M{_n(inner[0][0])},{_n(inner[0][1])}"
    ibody = ihead + "".join(f"L{_n(px)},{_n(py)}" for px, py in inner[1:])
    return f'<path fill="{GREY}" {EVEN_ODD} d="{outer}Z{ibody}Z"/>'


HEADER = (
    '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">\n'
    "  <!-- Generated by tools/gen-icons.py. Do not edit by hand. -->\n"
)


def svg(*parts: str) -> str:
    return HEADER + "".join(f"  {part}\n" for part in parts) + "</svg>\n"


TRIANGLE = [(8, 1.8), (14.4, 13.6), (1.6, 13.6)]

# The clipboard and its clip, as one even-odd path. Written as a raw string so
# the embedded double quotes need no escaping.

ICONS = {
    # Smart views.
    "gtaskbar-today-symbolic": svg(
        frame(2.4, 3.4, 11.2, 10.2, 2),
        h_line(2.4, 13.6, 6.4),
        v_line(2, 5.4, 5.4),
        v_line(2, 5.4, 10.6),
        disc(5.6, 8.4, 0.8), disc(8, 8.4, 0.8), disc(10.4, 8.4, 0.8),
        disc(5.6, 10.6, 0.8), disc(8, 10.6, 0.8), disc(10.4, 10.6, 0.8),
    ),
    "gtaskbar-upcoming-symbolic": svg(
        ring(8, 8, 5.7),
        line(8, 4.9, 8, 8),
        line(8, 8, 10.2, 9.4),
    ),
    "gtaskbar-overdue-symbolic": svg(
        triangle_outline(TRIANGLE),
        line(8, 6.4, 8, 9.2),
        disc(8, 11.4, 0.85),
    ),
    "gtaskbar-all-symbolic": svg(
        h_line(2.6, 13.4, 4.4),
        h_line(2.6, 13.4, 8),
        h_line(2.6, 13.4, 11.6),
    ),
    "gtaskbar-completed-symbolic": svg(
        ring(8, 8, 5.7),
        line(5.3, 8.2, 7.1, 10),
        line(7.1, 10, 10.7, 6.3),
    ),

    # Lists and actions.
    "gtaskbar-list-symbolic": svg(
        disc(3.2, 4.5, 0.95),
        disc(3.2, 8, 0.95),
        disc(3.2, 11.5, 0.95),
        h_line(6.6, 13.4, 4.5),
        h_line(6.6, 13.4, 8),
        h_line(6.6, 13.4, 11.5),
    ),
    "gtaskbar-check-symbolic": svg(
        line(2.9, 8.3, 6.2, 11.6, 1.7),
        line(6.2, 11.6, 13.1, 4.7, 1.7),
    ),
    "gtaskbar-add-symbolic": svg(
        v_line(3.3, 12.7, 8, 1.7),
        h_line(3.3, 12.7, 8, 1.7),
    ),
    "gtaskbar-sync-symbolic": svg(
        # Three-quarter arc, as a thick crescent.
        f'<path fill="{GREY}" d="M13.1 6.1a5.4 5.4 0 1 0 .3 4.05l-1.62-.72'
        'a3.8 3.8 0 1 1-.22-2.86Z"/>',
        poly([(13.9, 2.6), (13.9, 7.1), (9.4, 7.1)]),
    ),
    "gtaskbar-settings-symbolic": svg(
        # Sliders rather than a gear: a 16px gear cannot be drawn legibly as
        # filled geometry, and sliders read more clearly at this size.
        h_line(2.2, 13.8, 4.2),
        h_line(2.2, 13.8, 8),
        h_line(2.2, 13.8, 11.8),
        disc(5.4, 4.2, 1.7),
        disc(10.6, 8, 1.7),
        disc(6.2, 11.8, 1.7),
    ),

    # Window chrome.
    "gtaskbar-menu-symbolic": svg(
        h_line(2.7, 13.3, 4.5),
        h_line(2.7, 13.3, 8),
        h_line(2.7, 13.3, 11.5),
    ),
    "gtaskbar-close-symbolic": svg(
        line(4.2, 4.2, 11.8, 11.8),
        line(11.8, 4.2, 4.2, 11.8),
    ),
    "gtaskbar-search-symbolic": svg(
        ring(7.1, 7.1, 4.3),
        line(10.3, 10.3, 13.4, 13.4, 1.7),
    ),
    "gtaskbar-account-symbolic": svg(
        ring(8, 5.7, 2.6),
        f'<path fill="{GREY}" d="M2.9 13.6a5.1 5.1 0 0 1 10.2 0Z"/>',
    ),

    # The app's own mark, in the same language as the rest: a clipboard with
    # two completed rows.
    "gtaskbar-symbolic": svg(
        frame(1.4, 4.4, 11.2, 8.2, 1),
        frame(5.6, 2.6, 3.8, 3.0, 1),
        line(4.6, 8.5, 5.9, 9.8, 1.5),
        line(5.9, 9.8, 8.7, 7, 1.5),
        h_line(9.6, 12.4, 8.4, 1.4),
        line(4.6, 11.5, 5.9, 12.8, 1.5),
        line(5.9, 12.8, 8.7, 10, 1.5),
        h_line(9.6, 12.4, 11.4, 1.4),
    ),
}


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, content in ICONS.items():
        (OUT / f"{name}.svg").write_text(content)
    print(f"wrote {len(ICONS)} icons to {OUT}")

    problems = []
    for path_ in sorted(OUT.glob("*.svg")):
        text = path_.read_text()
        if "currentColor" in text:
            problems.append(f"{path_.name}: uses currentColor")
        # A stroked shape would render as a solid blob under GTK's symbolic
        # recolouring, so the set must be filled geometry throughout.
        if "stroke=" in text:
            problems.append(f"{path_.name}: uses stroke geometry")
        if GREY not in text:
            problems.append(f"{path_.name}: not drawn in the symbolic grey")
    if problems:
        print("\nproblems:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
