#!/usr/bin/env bash
# Original Win64 fixture; no Windows SDK, import libraries, or DLL dependencies.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p build/compat/nt
as --64 -o build/compat/nt/hello.o crates/nt-fixtures/hello.S
ld -static --build-id=none -T crates/nt-fixtures/nt.ld -o build/compat/nt/hello.elf build/compat/nt/hello.o
objcopy -O binary --only-section=.text build/compat/nt/hello.elf build/compat/nt/text.bin
objcopy -O binary --only-section=.rdata build/compat/nt/hello.elf build/compat/nt/rdata.bin
objcopy -O binary --only-section=.data build/compat/nt/hello.elf build/compat/nt/data.bin
python3 tools/pack-nt.py
objdump -p build/compat/nt/hello.exe > build/compat/nt/hello-headers.txt
as --64 --defsym NEXT_CROSS=1 -o build/compat/nt/hello-gate.o crates/nt-fixtures/hello.S
ld -static --build-id=none -T crates/nt-fixtures/nt.ld -o build/compat/nt/hello-gate.elf build/compat/nt/hello-gate.o
objcopy -O binary --only-section=.text build/compat/nt/hello-gate.elf build/compat/nt/text.bin
objcopy -O binary --only-section=.rdata build/compat/nt/hello-gate.elf build/compat/nt/rdata.bin
objcopy -O binary --only-section=.data build/compat/nt/hello-gate.elf build/compat/nt/data.bin
python3 tools/pack-nt.py hello-gate
objdump -p build/compat/nt/hello-gate.exe > build/compat/nt/hello-gate-headers.txt
