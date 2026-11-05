"""Regenerate icon.ico and icon.png next to this script (stdlib only): python assets/make_icon.py

The >_ mark shared with dprep: Catppuccin Base ground, green chevron, blue cursor. Green and blue
are the colours of the classic shell prompt. Then rebuild the exe's resource from the project root
(rc.exe ships with the Windows SDK): rc.exe /fo dterm.res dterm.rc
"""
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]

GROUND = (0x1E, 0x1E, 0x2E)
ACCENT = (0xA6, 0xE3, 0xA1)
TEXT = (0x89, 0xB4, 0xFA)

# All in units of the icon's width. Small sizes get heavier strokes to stay readable.
CORNER = 0.22
CHEVRON = [(0.27, 0.27), (0.50, 0.50), (0.27, 0.73)]
CURSOR = [(0.56, 0.73), (0.76, 0.73)]


def segment_distance(px, py, a, b):
    ax, ay = a
    bx, by = b
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return ((px - ax - t * dx) ** 2 + (py - ay - t * dy) ** 2) ** 0.5


def tile_distance(px, py):
    qx = abs(px - 0.5) - (0.5 - CORNER)
    qy = abs(py - 0.5) - (0.5 - CORNER)
    outside = (max(qx, 0.0) ** 2 + max(qy, 0.0) ** 2) ** 0.5
    return outside + min(max(qx, qy), 0.0) - CORNER


def coverage(distance, size):
    """Anti-aliased coverage of a shape whose edge is `distance` away, one pixel wide."""
    return max(0.0, min(1.0, 0.5 - distance * size))


def mix(under, over, amount):
    return tuple(u + (o - u) * amount for u, o in zip(under, over))


def render(size):
    half_stroke = 0.055 if size >= 48 else 0.075
    rows = []
    for y in range(size):
        row = bytearray([0])  # PNG filter type: none
        for x in range(size):
            px, py = (x + 0.5) / size, (y + 0.5) / size
            colour = GROUND
            chevron = min(segment_distance(px, py, CHEVRON[i], CHEVRON[i + 1]) for i in range(2))
            colour = mix(colour, ACCENT, coverage(chevron - half_stroke, size))
            cursor = segment_distance(px, py, CURSOR[0], CURSOR[1])
            colour = mix(colour, TEXT, coverage(cursor - half_stroke, size))
            alpha = coverage(tile_distance(px, py), size)
            row += bytes(round(c) for c in colour) + bytes([round(alpha * 255)])
        rows.append(bytes(row))
    return png(size, b"".join(rows))


def png(size, raw):
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # 8-bit RGBA
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def main():
    images = [(size, render(size)) for size in SIZES]
    offset = 6 + 16 * len(images)
    ico = struct.pack("<HHH", 0, 1, len(images))
    for size, data in images:
        ico += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    with open(os.path.join(HERE, "icon.ico"), "wb") as f:
        f.write(ico + b"".join(data for _, data in images))
    with open(os.path.join(HERE, "icon.png"), "wb") as f:
        f.write(render(512))


if __name__ == "__main__":
    main()
