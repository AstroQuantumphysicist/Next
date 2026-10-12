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


def decoded_instructions(text):
    """Return decoded instructions with their actual compiled addresses/bytes."""
    result = []
    pattern = re.compile(
        r"^\s*([0-9a-f]+):\s+((?:[0-9a-f]{2}\s+)+)([a-z][a-z0-9]*)\b\s*(.*)$",
        re.I,
    )
    for line in text.splitlines():
        found = pattern.match(line)
        if found:
            raw = bytes.fromhex(found.group(2))
            result.append(
                {
                    "address": int(found.group(1), 16),
                    "size": len(raw),
                    "mnemonic": found.group(3).lower(),
                    "operands": found.group(4).strip(),
                    "instruction": line.strip(),
                }
            )
    return result


def _one_instruction(instructions, predicate, description):
    matches = [item for item in instructions if predicate(item)]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one compiled {description}, found {len(matches)}")
    return matches[0]


def analyze_transition_stages(disassembly, hardware_source, bootstrap_source, startup_source):
    """Check and report the currently compiled mixed-mode transition windows."""
    decoded = {name: decoded_instructions(text) for name, text in disassembly.items()}

    def mnemonics(instructions, names):
        return [item for item in instructions if item["mnemonic"] in names]

    real = decoded["real16"]
    real_idts = mnemonics(real, {"lidt", "lidtw"})
    if len(real_idts) != 2 or any(item["mnemonic"] != "lidtw" for item in real_idts):
        raise ValueError("real-mode entry must install one full IVT then one NMI bridge")
    if "0xd40" not in real_idts[0]["operands"] or "0xd58" not in real_idts[1]["operands"]:
        raise ValueError("real-mode IVT/bridge loads must use their serialized private descriptors")
    real_stack = _one_instruction(real, lambda i: i["mnemonic"] == "mov" and i["operands"].startswith("ss,"), "real SS load")
    real_sp = _one_instruction(real, lambda i: i["mnemonic"] == "mov" and i["operands"].startswith("sp,"), "real SP load")
    if real_sp["address"] != real_stack["address"] + real_stack["size"] or real_idts[0]["address"] <= real_sp["address"]:
        raise ValueError("real private SS:SP must be adjacent and precede private IVT")
    real_pe = _one_instruction(real, lambda i: i["mnemonic"] == "mov" and i["operands"].startswith("cr0,"), "real PE control-register write")
    if not real_idts[0]["address"] < real_idts[1]["address"] < real_pe["address"]:
        raise ValueError("real IVT/bridge order must precede protected-mode transition")
    real_far = _one_instruction(real, lambda i: i["mnemonic"] == "jmp" and re.match(r"(?:0x)?[0-9a-f]+:\s*(?:0x)?[0-9a-f]+", i["operands"], re.I), "real-to-protected far jump")
    if real_far["address"] <= real_pe["address"]:
        raise ValueError("protected-mode far jump must follow PE enable")

    protected = decoded["protected32"]
    protected_idts = mnemonics(protected, {"lidt", "lidtd"})
    if len(protected_idts) != 2 or any(item["mnemonic"] != "lidtd" for item in protected_idts):
        raise ValueError("protected mode must load its IDT then restore the NMI bridge")
    if "0x82d46" not in protected_idts[0]["operands"] or "0x82d58" not in protected_idts[1]["operands"]:
        raise ValueError("protected IDT/bridge loads must use their serialized private descriptors")
    protected_stack = _one_instruction(protected, lambda i: i["mnemonic"] == "mov" and i["operands"].startswith("ss,"), "protected SS load")
    protected_sp = next(
        (
            item
            for item in protected
            if item["address"] == protected_stack["address"] + protected_stack["size"]
        ),
        None,
    )
    if protected_sp is None or protected_sp["mnemonic"] != "mov" or not protected_sp["operands"].startswith("esp,") or protected_idts[0]["address"] <= protected_sp["address"]:
        raise ValueError("protected private SS:ESP must be adjacent and precede private IDT")
    protected_pg = _one_instruction(protected, lambda i: i["mnemonic"] == "mov" and i["operands"].startswith("cr0,"), "protected paging control-register write")
    if not protected_idts[0]["address"] < protected_idts[1]["address"] < protected_pg["address"]:
        raise ValueError("protected IDT/bridge order must precede paging transition")
    protected_far = _one_instruction(protected, lambda i: i["mnemonic"] == "jmp" and re.match(r"(?:0x)?[0-9a-f]+:\s*(?:0x)?[0-9a-f]+", i["operands"], re.I), "protected-to-long far jump")
    if protected_far["address"] <= protected_pg["address"]:
        raise ValueError("long-mode far jump must follow paging enable")

    long_mode = decoded["long64"]
    long_idts = mnemonics(long_mode, {"lidt", "lidtd", "lidtw"})
    if len(long_idts) != 1 or long_idts[0]["mnemonic"] != "lidt":
        raise ValueError("long-mode entry must install one private IDT")
    if "[r12+0x30]" not in long_idts[0]["operands"]:
        raise ValueError("long-mode IDT must load the current AP mailbox's private descriptor")
    long_sp = _one_instruction(long_mode, lambda i: i["mnemonic"] == "mov" and i["operands"].startswith("rsp,"), "long-mode RSP load")
    if long_idts[0]["address"] <= long_sp["address"]:
        raise ValueError("long-mode private IDT must follow the private stack install")

    protected_idt_contract = [
        "for vector in 0..256",
        "let pm = 1280 + vector * 8;",
        "let address = 0x80000 + protected_offset as u32;",
        "bytes[pm + 2..pm + 4].copy_from_slice(&8u16.to_le_bytes());",
        "bytes[pm + 5] = 0x8e;",
        "protected_offset = (addr_of!(next_ap_emergency32) as usize)",
    ]
    if any(fragment not in bootstrap_source + hardware_source for fragment in protected_idt_contract):
        raise ValueError("cannot verify the full protected IDT's private 32-bit selector/handler construction")
    if "lidt [0x82d46]" not in startup_source or "expected[3398..3400].copy_from_slice(&2047u16.to_le_bytes());" not in hardware_source or "expected[3400..3404].copy_from_slice(&(bootstrap::PROTECTED_IDT as u32).to_le_bytes());" not in hardware_source:
        raise ValueError("protected mode must load the serialized 256-entry IDT descriptor")

    def boundary(item):
        return {"address": item["address"], "instruction": item["instruction"]}

    return {
        "activation_supported": False,
        "address_convention": "instruction addresses are physical addresses in the copied SIPI image; stage changes take effect after the named instruction retires",
        "compiled_landmarks": {
            "real_full_ivt_load": boundary(real_idts[0]),
            "real_nmi_bridge_load": boundary(real_idts[1]),
            "real_enable_protected_mode": boundary(real_pe),
            "real_to_protected_far_jump": boundary(real_far),
            "protected_private_stack_load": boundary(protected_sp),
            "protected_private_idt_load": boundary(protected_idts[0]),
            "protected_nmi_bridge_load": boundary(protected_idts[1]),
            "protected_enable_paging": boundary(protected_pg),
            "protected_to_long_far_jump": boundary(protected_far),
            "long_private_stack_load": boundary(long_sp),
            "long_private_idt_load": boundary(long_idts[0]),
        },
        "protected_private_idt_source": {
            "entries": 256,
            "code_selector": 8,
            "handler": "next_ap_emergency32 in the copied startup image",
            "idtr_limit": 2047,
            "basis": "source-verified serializer and metadata IDTR construction; AP RAM contents were not read during execution",
        },
        "stages": [
            {"stage": "before_first_instruction", "start": None, "end": "SIPI entry at 0x80000", "vector_coverage": "unproven", "private_stack": "unproven", "blocker": "NMI can arrive before AP executes CLI; BSP observation cannot constrain reset-time AP LINT or VMM injection"},
            {"stage": "real_bootstrap_before_private_idt", "start": "0x80000", "end_at": boundary(real_idts[0]), "vector_coverage": "unproven", "private_stack": False, "blocker": "firmware/reset IVT active while AP identity and private SS:SP are established"},
            {"stage": "real_private_idt", "start_after": boundary(real_idts[0]), "end_at": boundary(real_idts[1]), "vector_coverage": "all 256 IVT vectors route to private emergency park", "private_stack": True},
            {"stage": "real_nmi_bridge", "start_after": boundary(real_idts[1]), "end_at": boundary(real_pe), "vector_coverage": "vector 2 only", "private_stack": True, "blocker": "synchronous exceptions are not bridged"},
            {"stage": "protected_entry_before_private_stack", "start": "target of real-to-protected far jump", "end_at": boundary(protected_sp), "vector_coverage": "vector 2 only", "private_stack": False, "blocker": "shared NMI bridge is active while protected SS:ESP is installed"},
            {"stage": "protected_bootstrap_with_nmi_bridge", "start_after": boundary(protected_sp), "end_at": boundary(protected_idts[0]), "vector_coverage": "vector 2 only", "private_stack": True, "blocker": "pre-private-IDT protected exceptions are uncovered"},
            {"stage": "protected_private_idt", "start_after": boundary(protected_idts[0]), "end_at": boundary(protected_idts[1]), "vector_coverage": "all 256 protected-mode IDT entries target the private 32-bit emergency park handler", "private_stack": True},
            {"stage": "paging_nmi_bridge", "start_after": boundary(protected_idts[1]), "end_at": boundary(protected_pg), "vector_coverage": "vector 2 only", "private_stack": True, "blocker": "synchronous exceptions are not bridged across PG/LMA switch"},
            {"stage": "long_entry_before_private_stack", "start": "target of protected-to-long far jump", "end_at": boundary(long_sp), "vector_coverage": "vector 2 only", "private_stack": False, "blocker": "shared NMI bridge remains active while RSP is installed"},
            {"stage": "long_nmi_bridge", "start_after": boundary(long_sp), "end_at": boundary(long_idts[0]), "vector_coverage": "vector 2 only", "private_stack": True, "blocker": "synchronous exceptions are not bridged before the private long-mode IDT"},
            {"stage": "long_private_idt", "start_after": boundary(long_idts[0]), "end": "rest of long-mode entry", "vector_coverage": "all 256 IDT entries target the private park/failure handler", "private_stack": True},
        ],
        "non_idt_events": {"smi": "not contained by an IDT", "init": "not contained by an IDT", "vmm_injection": "cannot be excluded by BSP IOAPIC/LINT observation"},
        "interpretation": "Compiled order and byte decoding only; no AP startup, platform guarantee, or safety authority is established.",
    }


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
    disassembly_by_mode = {}
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
        disassembly_by_mode[name] = decoded
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
    manifest["audit_source_context_sha256"] = {name: digest((ROOT / name).read_bytes()) for name in ["crates/helix-kernel/src/ap_startup.S", "crates/helix-kernel/src/ap_startup.rs", "crates/helix-kernel/src/ap_startup_hardware.rs", "crates/helix-kernel/src/ap_bootstrap.rs", "crates/helix-kernel/src/ap_transition.rs", "crates/helix-kernel/src/ap_nmi_sources.rs", "crates/helix-kernel/src/arch.rs"]}
    manifest["transition_stage_analysis"] = analyze_transition_stages(
        disassembly_by_mode,
        (ROOT / "crates/helix-kernel/src/ap_startup_hardware.rs").read_text(),
        (ROOT / "crates/helix-kernel/src/ap_bootstrap.rs").read_text(),
        (ROOT / "crates/helix-kernel/src/ap_startup.S").read_text(),
    )
    (out / "audit.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"result": "PASS", "blob_bytes": len(blob), "blob_sha256": digest(blob), "manifest": str(out / "audit.json"), "scope": manifest["scope"]}))


if __name__ == "__main__":
    main()
