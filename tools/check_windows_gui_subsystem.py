from __future__ import annotations

import struct
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EXE = ROOT / "target" / "release" / "pi-wizard-desktop.exe"
IMAGE_SUBSYSTEM_WINDOWS_GUI = 2


def read_windows_subsystem(path: Path) -> int:
    with path.open("rb") as file:
        dos_header = file.read(0x40)
        if len(dos_header) < 0x40 or dos_header[:2] != b"MZ":
            raise SystemExit(f"Windows GUI subsystem check failed: {path} is not a PE executable")

        pe_offset = struct.unpack_from("<I", dos_header, 0x3C)[0]
        file.seek(pe_offset)
        pe_and_headers = file.read(4 + 20 + 0x46)

    if len(pe_and_headers) < 4 or pe_and_headers[:4] != b"PE\0\0":
        raise SystemExit(f"Windows GUI subsystem check failed: {path} has no valid PE signature")
    subsystem_offset = 4 + 20 + 0x44
    if subsystem_offset + 2 > len(pe_and_headers):
        raise SystemExit(f"Windows GUI subsystem check failed: {path} has a truncated optional header")
    return struct.unpack_from("<H", pe_and_headers, subsystem_offset)[0]


def main() -> None:
    if not EXE.is_file():
        raise SystemExit(f"Windows GUI subsystem check failed: release executable is missing: {EXE}")
    subsystem = read_windows_subsystem(EXE)
    if subsystem != IMAGE_SUBSYSTEM_WINDOWS_GUI:
        raise SystemExit(
            "Windows GUI subsystem check failed: "
            f"expected IMAGE_SUBSYSTEM_WINDOWS_GUI ({IMAGE_SUBSYSTEM_WINDOWS_GUI}), got {subsystem}"
        )
    print("Windows GUI subsystem check passed")


if __name__ == "__main__":
    main()
