#!/usr/bin/env python3
"""Audit compiled, unactivated AP image bytes; this is not a guest AP test.

An isolated audit ELF may retain otherwise unused code with -C link-dead-code=yes.
This tool neither builds/signs images, modifies source, maps code nor starts APs.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[1]
MAX_ELF = 32 * 1024 * 1024


def digest(data):
    return hashlib.sha256(data).hexdigest()


def audit(elf):
    data = elf.read_bytes()
    if len(data) < 64 or len(data) > MAX_ELF or data[:7] != b"\x7fELF\x02\x01\x01":
        raise ValueError("requires bounded little-endian ELF64")
    if struct.unpack_from("<HH", data, 16) != (2, 62):
        raise ValueError("requires x86-64 ET_EXEC")
    phoff = struct.unpack_from("<Q", data, 32)[0]
    phsize, phcount = struct.unpack_from("<HH", data, 54)
    if phsize != 56 or not 1 <= phcount <= 16 or phoff + phsize * phcount > len(data):
        raise ValueError("invalid bounded program table")
    loads = []
    for i in range(phcount):
        kind, flags, offset, virtual, physical, files, memory, align = struct.unpack_from("<IIQQQQQQ", data, phoff + i * phsize)
        if kind == 1:
            if virtual != physical or files > memory or offset + files > len(data):
                raise ValueError("invalid identity load")
            loads.append((flags, offset, virtual, files))
    symbols = {}
    raw = subprocess.run(["nm", "-P", str(elf)], check=True, capture_output=True, text=True).stdout
    for line in raw.splitlines():
        columns = line.split()
        if len(columns) >= 3 and (columns[0].startswith("next_ap_") or "MAILBOXES" in columns[0]):
            symbols[columns[0]] = int(columns[2], 16)
    wanted = ["next_ap_blob_start", "next_ap_pm32", "next_ap_lm64", "next_ap_gdt", "next_ap_gdt_ptr", "next_ap_null_idt", "next_ap_blob_end", "next_ap_private_fault"]
    if any(n not in symbols for n in wanted):
        raise ValueError("missing retained AP symbols; use explicit audit linkage or an immutable source-image consumer")
    start, pm, lm, gdt, pointer, null, end, fault = [symbols[n] for n in wanted]
    if not start < pm < lm < gdt < pointer < null < end or end - start > 4096:
        raise ValueError("invalid mixed-mode image layout")
    containing = [p for p in loads if p[2] <= start and end <= p[2] + p[3]]
    if len(containing) != 1 or containing[0][0] != 4:
        raise ValueError("AP source must be entirely in immutable RO/NX ELF load")
    fault_load = [p for p in loads if p[2] <= fault < p[2] + p[3]]
    if len(fault_load) != 1 or fault_load[0][0] != 5:
        raise ValueError("private fault handler must be kernel RX")
    flags, fileoff, virtual, files = containing[0]
    blob = data[fileoff + start - virtual:fileoff + end - virtual]
    p, l, g, gp, ni = [v - start for v in (pm, lm, gdt, pointer, null)]
    if blob[:2] != b"\xfa\xfc":
        raise ValueError("entry must CLI/CLD before state access")
    transitions = [b"\x66\xea" + struct.pack("<IH", 0x80000 + p, 8), b"\xea" + struct.pack("<IH", 0x80000 + l, 0x18)]
    if blob[:p].count(transitions[0]) != 1 or blob[p:l].count(transitions[1]) != 1:
        raise ValueError("far transitions must target copied SIPI image and private GDT selectors")
    expected_gdt = struct.pack("<QQQQ", 0, 0x00CF9B000000FFFF, 0x00CF93000000FFFF, 0x00AF9B000000FFFF)
    if blob[g:gp] != expected_gdt or blob[gp:ni] != struct.pack("<HI", 31, 0x80000 + g):
        raise ValueError("private GDT bytes/pointer differ from architectural descriptors")
    if blob[ni:] != b"\0" * 6:
        raise ValueError("transition IDT must not inherit BSP/firmware table")
    if blob.count(b"\xfa\xf4\xeb\xfc") != 2:
        raise ValueError("both failure and successful paths must park with IF clear")
    if "next_ap_emergency16" in symbols:
        if "next_ap_emergency64" not in symbols:
            raise ValueError("missing mode-compatible long emergency handler")
        real = symbols["next_ap_emergency16"] - start
        protected = symbols["next_ap_emergency32"] - start
        long = symbols["next_ap_emergency64"] - start
        if not 0 < real < p <= protected < l <= long < g:
            raise ValueError("emergency handler does not belong to its copied CPU mode")
        if blob[real:real+5] != bytes.fromhex("fa f4 e9 fb ff") or blob[protected:protected+7] != bytes.fromhex("fa f4 e9 f9 ff ff ff") or blob[long:long+7] != bytes.fromhex("fa f4 e9 f9 ff ff ff"):
            raise ValueError("emergency handlers must park without return or BSP accesses")
    # Real DS=8200 installs the bridge descriptor at physical82D58 without
    # an invalid >64KiB real-segment operand. Protected DS is flat.
    real_bridge = bytes.fromhex("b8 00 82 8e d8 0f 01 1e 58 0d 31 c0 8e d8")
    protected_bridge = bytes.fromhex("0f 01 1d 58 2d 08 00 0f 22 c0")
    if blob[:p].count(real_bridge) != 1 or blob[p:l].count(protected_bridge) != 1:
        raise ValueError("missing bounded real/protected NMI bridge installation before mode switch")
    parts = []
    for name, lower, upper, machine, mode in [
        ("real16", 0, p, "i386", "intel,addr16,data16"),
        ("protected32", p, l, "i386", "intel,addr32,data32"),
        ("long64", l, g, "i386:x86-64", "intel"),
    ]:
        parts.append((name, blob[lower:upper], machine, mode, 0x80000 + lower))
    return blob, parts, {"scope": "host byte/disassembly audit; no AP activation or guest execution", "elf_sha256": digest(data), "blob_sha256": digest(blob), "blob_bytes": len(blob), "source_load_flags": flags, "symbols": {n: symbols[n] for n in wanted + (["next_ap_emergency16", "next_ap_emergency32", "next_ap_emergency64"] if "next_ap_emergency16" in symbols else [])}, "early_nmi_blocker": "Activation requires reviewed NMI-source quiescence/transition handling before private long-mode IDT installation."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--elf", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    out = args.output_dir.resolve()
    if not out.is_relative_to((ROOT / "work").resolve()) or out.exists():
        parser.error("output must be a new directory under work/")
    elf = args.elf.resolve()
    if not elf.is_file() or elf.stat().st_size > MAX_ELF:
        parser.error("ELF must be an existing bounded file")
    blob, parts, manifest = audit(elf)
    out.mkdir(parents=True)
    (out / "startup.bin").write_bytes(blob)
    disassembly = ""
    for name, code, machine, mode, base in parts:
        raw = out / f"{name}.bin"
        raw.write_bytes(code)
        decoded = subprocess.run(["objdump", "-D", "-b", "binary", "-m", machine, "-M", mode, f"--adjust-vma={base}", str(raw)], check=True, capture_output=True, text=True).stdout
        # Examine decoded instruction mnemonics, never raw hexadecimal bytes.
        mnemonics = re.findall(r"^\s*[0-9a-f]+:\s+(?:[0-9a-f]{2}\s+)+([a-z][a-z0-9]*)\b", decoded, re.M)
        if any(m in {"sti", "call", "ret", "iret", "iretd", "iretq", "syscall", "sysenter", "int"} for m in mnemonics) or "(bad)" in decoded:
            raise ValueError("unexpected control transfer/interrupt enable or undecodable startup instruction")
        (out / f"{name}.txt").write_text(decoded)
        disassembly += decoded
    if disassembly.count("lock cmpxchg") != 2 or "lidt" not in disassembly or "lgdt" not in disassembly:
        raise ValueError("missing private table installation or bounded ownership CAS")
    if "next_ap_emergency16" in manifest["symbols"]:
        # Real and protected SS reloads must immediately install the matching SP.
        for name, successor in [("real16", "sp"), ("protected32", "esp")]:
            decoded = (out / f"{name}.txt").read_text()
            instructions = re.findall(r"^\s*[0-9a-f]+:\s+(?:[0-9a-f]{2}\s+)+(.+)$", decoded, re.M)
            ss = [i for i, instruction in enumerate(instructions) if re.search(r"\bmov\s+ss,", instruction)]
            if len(ss) != 1 or not re.search(rf"\bmov\s+{successor},", instructions[ss[0]+1]):
                raise ValueError("private stack must install SP immediately after MOV SS")
        manifest["stack_reload_shadow_audited"] = True
    manifest["audit_source_context_sha256"] = {name: digest((ROOT / name).read_bytes()) for name in ["crates/helix-kernel/src/ap_startup.S", "crates/helix-kernel/src/ap_startup.rs", "crates/helix-kernel/src/ap_startup_hardware.rs", "crates/helix-kernel/src/arch.rs"]}
    (out / "audit.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"result": "PASS", "blob_bytes": len(blob), "blob_sha256": digest(blob), "manifest": str(out / "audit.json"), "scope": manifest["scope"]}))


if __name__ == "__main__":
    main()
