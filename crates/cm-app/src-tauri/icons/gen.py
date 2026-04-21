"""Generate placeholder icons for Tauri (no Pillow).

Writes 32x32, 128x128, 128x128@2x PNGs plus icon.ico and icon.icns.
All icons are a solid indigo square — good enough for dev builds until
a real icon is designed.
"""

import os
import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
COLOR = (79, 70, 229, 255)  # indigo-600 RGBA


def make_png(size: int) -> bytes:
    r, g, b, a = COLOR
    raw = bytearray()
    for _ in range(size):
        raw.append(0)  # filter: none
        for _ in range(size):
            raw.extend((r, g, b, a))

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    idat = zlib.compress(bytes(raw), 9)
    return sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def make_ico(png_bytes: bytes, size: int) -> bytes:
    # ICO header with a single embedded PNG image entry.
    header = struct.pack("<HHH", 0, 1, 1)
    # width/height 0 means 256+, otherwise the dimension.
    w = 0 if size >= 256 else size
    h = 0 if size >= 256 else size
    entry = struct.pack(
        "<BBBBHHII",
        w,
        h,
        0,    # color count
        0,    # reserved
        1,    # color planes
        32,   # bpp
        len(png_bytes),
        22,   # offset (6-byte header + 16-byte entry)
    )
    return header + entry + png_bytes


def make_icns(png_bytes_128: bytes, png_bytes_256: bytes) -> bytes:
    # Minimal ICNS with ic07 (128) and ic08 (256) PNG entries.
    def entry(tag: bytes, data: bytes) -> bytes:
        return tag + struct.pack(">I", len(data) + 8) + data

    body = entry(b"ic07", png_bytes_128) + entry(b"ic08", png_bytes_256)
    return b"icns" + struct.pack(">I", len(body) + 8) + body


def main() -> None:
    p32 = make_png(32)
    p128 = make_png(128)
    p256 = make_png(256)
    p512 = make_png(512)

    (HERE / "32x32.png").write_bytes(p32)
    (HERE / "128x128.png").write_bytes(p128)
    (HERE / "128x128@2x.png").write_bytes(p256)
    (HERE / "icon.png").write_bytes(p512)
    (HERE / "icon.ico").write_bytes(make_ico(p256, 256))
    (HERE / "icon.icns").write_bytes(make_icns(p128, p256))

    for name in ("32x32.png", "128x128.png", "128x128@2x.png", "icon.png", "icon.ico", "icon.icns"):
        f = HERE / name
        print(f"{name}: {f.stat().st_size} bytes")


if __name__ == "__main__":
    main()
