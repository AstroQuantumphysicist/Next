#!/usr/bin/env python3
"""Read-only verifier for the bounded NXTCOW01 development store, not NextFS fsck.

Extent/protected ranges are supplied explicitly; nothing discovers or grants a
partition. No repair, salvage, format or write operation is implemented.
"""
import argparse
import hashlib
import json
import os
import re
import stat
import sys
import zlib

SECTOR = 512
REQUIRED = 132
MAX_U64 = (1 << 64) - 1


class Invalid(ValueError):
    pass


def number(b, offset, width=4):
    return int.from_bytes(b[offset:offset + width], "little")


def crc(b):
    return zlib.crc32(b) & 0xffffffff


def require(condition, reason):
    if not condition:
        raise Invalid(reason)


def extent(start, count):
    require(0 <= start <= MAX_U64 and 0 <= count <= MAX_U64
            and start + count <= MAX_U64, "extent arithmetic overflow")
    return start + count


def superblock(b, start, sectors):
    require(b[:8] == b"NXTCOW01", "unknown superblock magic")
    require(number(b, 8) == 1 and number(b, 12) == 512,
            "unsupported superblock version/sector size")
    generation, bank = number(b, 16, 8), number(b, 24)
    require(generation != 0 and bank <= 1 and number(b, 28) == 8,
            "invalid generation/bank/file count")
    require(number(b, 32, 8) == start and number(b, 40, 8) == sectors,
            "superblock extent mismatch")
    require(not any(b[52:508]) and number(b, 508) == crc(b[:508]),
            "superblock reserved bytes/CRC")
    return {"generation": generation, "bank": bank, "root_crc": number(b, 48)}


def root(b, commit):
    require(b[:8] == b"NXTRT001" and number(b, 8) == 1,
            "unknown root format")
    require(not any(b[12:16]) and not any(b[28:32]) and not any(b[480:508]),
            "root reserved bytes")
    require(number(b, 16, 8) == commit["generation"], "root generation mismatch")
    require(number(b, 508) == crc(b[:508]) == commit["root_crc"], "root CRC mismatch")
    files, names = [], set()
    for index in range(8):
        at = 32 + index * 56
        name = b[at:at + 32]
        size, checksum = number(b, at + 32), number(b, at + 36)
        require(not any(b[at + 40:at + 56]), "entry reserved bytes")
        if name[0] == 0:
            require(not any(name) and size == checksum == 0, "nonzero empty entry")
            continue
        zero = name.find(b"\0")
        require(zero >= 0 and not any(name[zero:]), "unterminated/noncanonical name")
        path = name[:zero]
        require(2 <= len(path) <= 31 and path.startswith(b"/") and
                re.fullmatch(rb"[A-Za-z0-9/._-]+", path) is not None and
                all(p not in (b"", b".", b"..") for p in path[1:].split(b"/")),
                "invalid canonical file path")
        require(path not in names, "duplicate file path")
        require(size <= 4096, "oversized file")
        names.add(path)
        files.append({"slot": index, "path": path.decode("ascii"),
                      "size": size, "crc32": checksum})
    require(number(b, 24) == len(files), "root file count mismatch")
    return files


def verify_bytes(data, start, sectors, protected):
    """Validate an extent captured from a read-only file; data is the 132 sectors."""
    end = extent(start, sectors)
    require(sectors >= REQUIRED and len(data) == REQUIRED * SECTOR, "truncated/short extent")
    for begin, count in protected:
        stop = extent(begin, count)
        require(count == 0 or not (start < stop and begin < end), "protected range overlap")
    raw = [data[:SECTOR], data[SECTOR:2 * SECTOR]]
    candidates = []
    for slot, b in enumerate(raw):
        candidate = {"slot": slot, "valid": False, "blank": not any(b)}
        try:
            candidate.update(superblock(b, start, sectors))
            candidate["valid"] = True
        except Invalid as error:
            candidate["error"] = str(error)
        candidates.append(candidate)
    commits = [c for c in candidates if c["valid"]]
    require(commits, "no valid committed superblock (never autoformat): " +
            "; ".join(f"slot{c['slot']}={c['error']}" for c in candidates if not c["valid"]))
    if len(commits) == 2 and commits[0]["generation"] == commits[1]["generation"]:
        require((commits[0]["bank"], commits[0]["root_crc"]) ==
                (commits[1]["bank"], commits[1]["root_crc"]),
                "conflicting same-generation commits")
    read_only = any(not c["valid"] and not c["blank"] for c in candidates)
    selected = None
    # Inspect every candidate, including an older inactive root. Selection follows
    # mount's newest-good rule; damage in an older unselected bank is reported but
    # does not retroactively make a healthy newer mount read-only.
    for c in sorted(commits, key=lambda c: -c["generation"]):
        bank_start = 2 + c["bank"] * 65
        try:
            at = (bank_start + 64) * SECTOR
            files = root(data[at:at + SECTOR], c)
            for file in files:
                at = (bank_start + file["slot"] * 8) * SECTOR
                payload = data[at:at + 4096]
                require(crc(payload[:file["size"]]) == file["crc32"], "file payload CRC mismatch")
                require(not any(payload[file["size"]:]), "nonzero file tail")
            c["snapshot_valid"] = True
            c["files"] = files
            if selected is None:
                selected = c
                selected["mount_read_only"] = read_only
        except Invalid as error:
            c["snapshot_valid"] = False
            c["snapshot_error"] = str(error)
            if selected is None:
                read_only = True
    require(selected is not None, "no valid committed snapshot: " +
            "; ".join(f"slot{c['slot']}={c.get('snapshot_error', 'invalid')}" for c in commits))
    return {"format": "NXTCOW01-development", "start_lba": start,
            "sectors": sectors, "selected_generation": selected["generation"],
            "selected_bank": selected["bank"], "mount_read_only": selected["mount_read_only"],
            "candidates": candidates, "files": selected["files"]}


def digest(stream):
    stream.seek(0)
    result = hashlib.sha256()
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        result.update(block)
    return result.hexdigest()


def verify(path, start, sectors, protected):
    end = extent(start, sectors)
    for begin, count in protected:
        stop = extent(begin, count)
        require(count == 0 or not (start < stop and begin < end), "protected range overlap")
    with open(path, "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode), "only regular scratch image files supported")
        require(info.st_size % SECTOR == 0 and end <= info.st_size // SECTOR,
                "truncated media or extent outside image")
        before = digest(stream)
        stream.seek(start * SECTOR)
        data = stream.read(REQUIRED * SECTOR)
        try:
            report = verify_bytes(data, start, sectors, protected)
        finally:
            require(digest(stream) == before, "image changed during verification")
        report["source_sha256"] = before
        return report


def protected_range(value):
    try:
        start, count = (int(v, 0) for v in value.split(":"))
        extent(start, count)
        return start, count
    except (ValueError, Invalid) as error:
        raise argparse.ArgumentTypeError("expected bounded START:COUNT") from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    parser.add_argument("--start-lba", type=lambda s: int(s, 0), required=True)
    parser.add_argument("--sectors", type=lambda s: int(s, 0), required=True)
    parser.add_argument("--protect", type=protected_range, action="append", required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(verify(args.image, args.start_lba, args.sectors, args.protect), indent=2))
    except (Invalid, OSError) as error:
        print(f"COW verification rejected: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
