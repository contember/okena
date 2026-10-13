"""Reject APK/AAB native libraries that cannot load with 16 KB memory pages."""

import struct
import sys
import zipfile


def check_library(data):
    if data[:6] != b"\x7fELF\x02\x01":
        raise ValueError("expected a little-endian 64-bit ELF library")
    offset = struct.unpack_from("<Q", data, 32)[0]
    entry_size, count = struct.unpack_from("<HH", data, 54)
    if entry_size < 56 or count == 0:
        raise ValueError("invalid ELF program header table")
    loads = 0
    for index in range(count):
        header = offset + index * entry_size
        kind, _, file_offset, address, _, _, _, alignment = struct.unpack_from(
            "<IIQQQQQQ", data, header
        )
        if kind == 1:
            loads += 1
            if alignment < 16384 or (address - file_offset) % 16384:
                raise ValueError(f"LOAD segment is not 16 KB aligned ({alignment})")
    if loads == 0:
        raise ValueError("no LOAD segments")


def main():
    if len(sys.argv) != 2:
        raise SystemExit("Usage: python3 scripts/check-native-alignment.py APP.aab|APP.apk")
    failures = []
    checked = 0
    with zipfile.ZipFile(sys.argv[1]) as archive:
        for name in archive.namelist():
            if not name.endswith(".so"):
                continue
            if "/arm64-v8a/" not in name and "/x86_64/" not in name:
                continue
            checked += 1
            try:
                check_library(archive.read(name))
            except (ValueError, struct.error) as error:
                failures.append(f"{name}: {error}")
    if not checked:
        raise SystemExit("No 64-bit native libraries found; check the artifact path.")
    if failures:
        raise SystemExit("\n".join(failures))
    print(f"Verified 16 KB ELF alignment for {checked} native libraries.")


if __name__ == "__main__":
    main()
