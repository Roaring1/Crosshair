#!/usr/bin/env python3
"""Compare the Rust software rasterizer with the frozen Python/Cairo drawing math.

The reference below is intentionally kept in sync with crosshair.py::_draw_crosshair.
Cairo and the Rust 2x2 sampler use different antialiasing implementations, so the
comparison uses geometry, alpha coverage and premultiplied-color tolerances rather
than requiring byte-identical output.
"""
from __future__ import annotations

import json
import math
import struct
import subprocess
import sys
from pathlib import Path
from typing import Any

import cairo

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
PROBE = ROOT / "target" / "debug" / "examples" / "render_probe"
CANVAS = 512
CENTER = CANVAS // 2


def draw_python_reference(cr: Any, cx: float, cy: float, config: dict[str, Any]) -> None:
    """Frozen copy of the Python GTK overlay's Cairo drawing procedure."""
    shape = config.get("shape", "dot")
    opacity = float(config.get("opacity", 0.95))
    size = float(config.get("size", 5))
    thick = float(config.get("thickness", 2))
    gap = float(config.get("gap", 3))

    def rgba(value: str, alpha: float) -> tuple[float, float, float, float]:
        clean = value.strip().removeprefix("#")
        return (
            int(clean[0:2], 16) / 255.0,
            int(clean[2:4], 16) / 255.0,
            int(clean[4:6], 16) / 255.0,
            alpha,
        )

    main_rgba = rgba(config.get("color", "#a000ff"), opacity)
    outline = config.get("outline", {})
    outline_on = bool(outline.get("enabled", True)) and int(outline.get("thickness", 1)) > 0
    outline_thick = float(outline.get("thickness", 1))
    outline_rgba = rgba(
        outline.get("color", "#000000"),
        float(outline.get("opacity", 0.8)) * opacity,
    )
    shadow = config.get("shadow", {})
    shadow_on = bool(shadow.get("enabled", False))
    shadow_rgba = rgba(
        shadow.get("color", "#000000"),
        float(shadow.get("opacity", 0.35)) * opacity,
    )
    shadow_dx = float(shadow.get("offset_x", 1))
    shadow_dy = float(shadow.get("offset_y", 1))

    cr.set_line_cap(cairo.LINE_CAP_ROUND)
    cr.set_line_join(cairo.LINE_JOIN_ROUND)

    def stroke_lines(color: tuple[float, float, float, float], width: float,
                     dx: float = 0.0, dy: float = 0.0) -> None:
        cr.set_source_rgba(*color)
        cr.set_line_width(max(0.5, width))
        if shape in ("cross", "cross_dot", "plus"):
            g = 0.0 if shape == "plus" else gap
            cr.move_to(cx + dx, cy + dy - g)
            cr.line_to(cx + dx, cy + dy - (g + size))
            cr.move_to(cx + dx, cy + dy + g)
            cr.line_to(cx + dx, cy + dy + (g + size))
            cr.move_to(cx + dx - g, cy + dy)
            cr.line_to(cx + dx - (g + size), cy + dy)
            cr.move_to(cx + dx + g, cy + dy)
            cr.line_to(cx + dx + (g + size), cy + dy)
            cr.stroke()
        if shape in ("x", "x_dot"):
            cr.move_to(cx + dx - gap, cy + dy - gap)
            cr.line_to(cx + dx - (gap + size), cy + dy - (gap + size))
            cr.move_to(cx + dx + gap, cy + dy + gap)
            cr.line_to(cx + dx + (gap + size), cy + dy + (gap + size))
            cr.move_to(cx + dx - gap, cy + dy + gap)
            cr.line_to(cx + dx - (gap + size), cy + dy + (gap + size))
            cr.move_to(cx + dx + gap, cy + dy - gap)
            cr.line_to(cx + dx + (gap + size), cy + dy - (gap + size))
            cr.stroke()
        if shape in ("ring", "circle", "dot_ring"):
            cr.arc(cx + dx, cy + dy, max(1.0, size), 0.0, math.tau)
            cr.stroke()

    def fill_dot(color: tuple[float, float, float, float], radius: float,
                 dx: float = 0.0, dy: float = 0.0) -> None:
        cr.set_source_rgba(*color)
        cr.arc(cx + dx, cy + dy, max(0.5, radius), 0.0, math.tau)
        cr.fill()

    # Match Python pass order exactly: shadow, outline, then main shape.
    if shadow_on:
        if shape == "dot":
            fill_dot(shadow_rgba, max(1.0, size), shadow_dx, shadow_dy)
        elif shape in ("ring", "circle"):
            stroke_lines(shadow_rgba, thick, shadow_dx, shadow_dy)
        elif shape == "dot_ring":
            stroke_lines(shadow_rgba, thick, shadow_dx, shadow_dy)
            fill_dot(shadow_rgba, max(1.0, thick * 0.9), shadow_dx, shadow_dy)
        else:
            stroke_lines(shadow_rgba, thick, shadow_dx, shadow_dy)
            if shape in ("cross_dot", "x_dot"):
                fill_dot(shadow_rgba, max(1.0, thick * 0.9), shadow_dx, shadow_dy)

    if outline_on:
        outline_width = thick + outline_thick * 2.0
        if shape == "dot":
            fill_dot(outline_rgba, max(1.0, size + outline_thick))
        elif shape in ("ring", "circle"):
            stroke_lines(outline_rgba, outline_width)
        elif shape == "dot_ring":
            stroke_lines(outline_rgba, outline_width)
            fill_dot(outline_rgba, max(1.0, thick * 0.9 + outline_thick))
        else:
            stroke_lines(outline_rgba, outline_width)
            if shape in ("cross_dot", "x_dot"):
                fill_dot(outline_rgba, max(1.0, thick * 0.9 + outline_thick))

    if shape == "dot":
        fill_dot(main_rgba, max(1.0, size))
    elif shape in ("ring", "circle"):
        stroke_lines(main_rgba, thick)
    elif shape == "dot_ring":
        stroke_lines(main_rgba, thick)
        fill_dot(main_rgba, max(1.0, thick * 0.9))
    else:
        stroke_lines(main_rgba, thick)
        if shape in ("cross_dot", "x_dot"):
            fill_dot(main_rgba, max(1.0, thick * 0.9))


def cases() -> list[tuple[str, dict[str, Any]]]:
    result: list[tuple[str, dict[str, Any]]] = []
    shapes = ("dot", "cross", "x", "plus", "ring", "circle", "cross_dot", "x_dot", "dot_ring")
    for shape in shapes:
        result.append((f"{shape}-base", {
            "shape": shape, "size": 9, "thickness": 3, "gap": 4,
            "opacity": 0.83, "color": "#26a0dd",
            "outline": {"enabled": False, "color": "#102030", "opacity": 0.7, "thickness": 2},
            "shadow": {"enabled": False, "color": "#334455", "opacity": 0.4,
                       "offset_x": 3, "offset_y": -2},
        }))

    for shape in ("dot", "cross", "x", "plus", "ring", "dot_ring", "cross_dot", "x_dot"):
        result.append((f"{shape}-layers", {
            "shape": shape, "size": 7, "thickness": 4, "gap": 5,
            "opacity": 0.76, "color": "#e05a2b",
            "outline": {"enabled": True, "color": "#183047", "opacity": 0.62, "thickness": 2},
            "shadow": {"enabled": True, "color": "#111827", "opacity": 0.48,
                       "offset_x": 3, "offset_y": -2},
        }))
    result.extend([
        ("cross-thick-edge", {
            "shape": "cross", "size": 1, "thickness": 25, "gap": 0,
            "opacity": 1.0, "color": "#f0a020",
            "outline": {"enabled": True, "color": "#101010", "opacity": 0.5, "thickness": 10},
            "shadow": {"enabled": True, "color": "#202830", "opacity": 0.55,
                       "offset_x": -6, "offset_y": 6},
        }),
        ("x-long-arms", {
            "shape": "x", "size": 30, "thickness": 1, "gap": 15,
            "opacity": 0.5, "color": "#16c0a0",
            "outline": {"enabled": False, "color": "#000000", "opacity": 0.8, "thickness": 1},
            "shadow": {"enabled": False, "color": "#000000", "opacity": 0.35,
                       "offset_x": 1, "offset_y": 1},
        }),
        ("dot-shadow-offset", {
            "shape": "dot", "size": 3, "thickness": 1, "gap": 0,
            "opacity": 0.64, "color": "#d04090",
            "outline": {"enabled": False, "color": "#000000", "opacity": 0.8, "thickness": 1},
            "shadow": {"enabled": True, "color": "#182838", "opacity": 0.72,
                       "offset_x": -12, "offset_y": 9},
        }),
    ])
    return result


def alpha_bounds(pixels: bytes, width: int, height: int) -> tuple[int, int, int, int] | None:
    xs: list[int] = []
    ys: list[int] = []
    for y in range(height):
        for x in range(width):
            if pixels[(y * width + x) * 4 + 3] > 0:
                xs.append(x)
                ys.append(y)
    if not xs:
        return None
    return min(xs), min(ys), max(xs), max(ys)


def alpha_mass(pixels: bytes) -> float:
    return sum(pixels[3::4]) / 255.0


def run_rust_probe(raw_config: dict[str, Any]) -> tuple[int, int, int, int, bytes]:
    result = subprocess.run(
        [str(PROBE)], input=json.dumps(raw_config).encode("utf-8"),
        capture_output=True, check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"Rust render probe failed: {result.stderr.decode(errors='replace')}")
    if len(result.stdout) < 16:
        raise RuntimeError("Rust render probe returned a truncated header")
    width, height, origin_x, origin_y = struct.unpack_from("<IIii", result.stdout)
    pixels = result.stdout[16:]
    if len(pixels) != width * height * 4:
        raise RuntimeError("Rust render probe returned an invalid pixel buffer")
    return width, height, origin_x, origin_y, pixels


def embed_rust(
    width: int, height: int, origin_x: int, origin_y: int, pixels: bytes
) -> bytes:
    left = CENTER + origin_x
    top = CENTER + origin_y
    if left < 0 or top < 0 or left + width > CANVAS or top + height > CANVAS:
        raise AssertionError(f"Rust raster outside comparison canvas: {(left, top, width, height)}")
    output = bytearray(CANVAS * CANVAS * 4)
    for y in range(height):
        source = y * width * 4
        target = ((top + y) * CANVAS + left) * 4
        output[target:target + width * 4] = pixels[source:source + width * 4]
    return bytes(output)


def render_cairo(config: dict[str, Any]) -> bytes:
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, CANVAS, CANVAS)
    context = cairo.Context(surface)
    draw_python_reference(context, CENTER, CENTER, config)
    surface.flush()
    return bytes(surface.get_data())


def main() -> int:
    if sys.byteorder != "little":
        raise SystemExit(
            "Renderer parity probe expects little-endian ARGB32 memory layout"
        )
    if not PROBE.is_file():
        raise SystemExit(
            f"Missing Rust helper {PROBE}; build it with cargo build --example render_probe"
        )

    failures: list[str] = []
    for name, raw in cases():
        # Keep aliases normalized the same way as the Python config loader.
        from crosshair_core import sanitize_config

        config = sanitize_config(raw)
        rust_width, rust_height, origin_x, origin_y, rust_local = run_rust_probe(config)
        rust = embed_rust(rust_width, rust_height, origin_x, origin_y, rust_local)
        reference = render_cairo(config)
        rust_bounds = alpha_bounds(rust, CANVAS, CANVAS)
        reference_bounds = alpha_bounds(reference, CANVAS, CANVAS)
        if rust_bounds is None or reference_bounds is None:
            failures.append(f"{name}: one renderer produced an empty image")
            continue

        bbox_delta = max(
            abs(a - b) for a, b in zip(rust_bounds, reference_bounds, strict=True)
        )
        rust_mass = alpha_mass(rust)
        reference_mass = alpha_mass(reference)
        mass_delta = abs(rust_mass - reference_mass) / max(reference_mass, 1.0)

        # Compare premultiplied ARGB channels in the shared 512px canvas. The
        # sparse canvas is useful for diagnostics; normalize MAE to the union
        # bounding box so a mostly-transparent background cannot hide errors.
        x0 = min(rust_bounds[0], reference_bounds[0])
        y0 = min(rust_bounds[1], reference_bounds[1])
        x1 = max(rust_bounds[2], reference_bounds[2])
        y1 = max(rust_bounds[3], reference_bounds[3])
        channel_error = 0
        channel_count = 0
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                index = (y * CANVAS + x) * 4
                channel_error += sum(abs(rust[index + c] - reference[index + c]) for c in range(4))
                channel_count += 4
        mean_error = channel_error / max(channel_count, 1)

        print(
            f"{name}: bbox_delta={bbox_delta}px alpha_mass_delta={mass_delta:.1%} "
            f"mean_channel_error={mean_error:.2f}/255 "
            f"rust={rust_bounds} cairo={reference_bounds}"
        )
        if bbox_delta > 1:
            failures.append(f"{name}: geometry bounds differ by {bbox_delta}px")
        if mass_delta > 0.12:
            failures.append(f"{name}: alpha coverage differs by {mass_delta:.1%}")
        if mean_error > 8.0:
            failures.append(f"{name}: mean premultiplied channel error is {mean_error:.2f}/255")

    if failures:
        print("\nRenderer parity failures:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print(
        f"PASS: {len(cases())} shared Python/Cairo cases met geometry and coverage tolerances"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
