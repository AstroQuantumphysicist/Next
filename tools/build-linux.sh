#!/usr/bin/env bash
# Original static Linux fixtures; the assembler and linker are host tools.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p build/compat/linux
as --64 -o build/compat/linux/hello.o crates/linux-fixtures/hello.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/hello.elf build/compat/linux/hello.o
as --64 --defsym NEXT_CROSS=1 -o build/compat/linux/hello-gate.o crates/linux-fixtures/hello.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/hello-gate.elf build/compat/linux/hello-gate.o
as --64 -o build/compat/linux/clock.o crates/linux-fixtures/clock.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/clock.elf build/compat/linux/clock.o
as --64 --defsym NEXT_CROSS=1 -o build/compat/linux/clock-gate.o crates/linux-fixtures/clock.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/clock-gate.elf build/compat/linux/clock-gate.o
as --64 -o build/compat/linux/memory.o crates/linux-fixtures/memory.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/memory.elf build/compat/linux/memory.o
as --64 --defsym NEXT_CROSS=1 -o build/compat/linux/memory-gate.o crates/linux-fixtures/memory.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/memory-gate.elf build/compat/linux/memory-gate.o
as --64 -o build/compat/linux/file.o crates/linux-fixtures/file.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/file.elf build/compat/linux/file.o
as --64 --defsym NEXT_CROSS=1 -o build/compat/linux/file-gate.o crates/linux-fixtures/file.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/file-gate.elf build/compat/linux/file-gate.o
as --64 -o build/compat/linux/tls.o crates/linux-fixtures/tls.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/tls.elf build/compat/linux/tls.o
as --64 --defsym NEXT_CROSS=1 -o build/compat/linux/tls-gate.o crates/linux-fixtures/tls.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/tls-gate.elf build/compat/linux/tls-gate.o
as --64 -o build/compat/linux/namespace.o crates/linux-fixtures/namespace.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/namespace.elf build/compat/linux/namespace.o
as --64 --defsym NEXT_CROSS=1 -o build/compat/linux/namespace-gate.o crates/linux-fixtures/namespace.S
ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld -o build/compat/linux/namespace-gate.elf build/compat/linux/namespace-gate.o
for fixture in getpid uname lseek fstat getdents cwd; do
    as --64 -o "build/compat/linux/$fixture.o" "crates/linux-fixtures/$fixture.S"
    ld -static --build-id=none -z max-page-size=4096 -T crates/linux-fixtures/linux.ld \
        -o "build/compat/linux/$fixture.elf" "build/compat/linux/$fixture.o"
done
python3 - <<'PY'
from pathlib import Path
import subprocess
binary = Path('build/compat/linux/hello.elf')
result = subprocess.run([str(binary.resolve())], capture_output=True, timeout=3)
expected = b'LINUX STATIC ELF: original syscall compatibility PASS\n'
if result.returncode != 37 or result.stdout != expected or result.stderr:
    raise SystemExit(f'Host Linux fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}')
print('Independent Linux static ELF host execution PASS (exit 37)')
binary = Path('build/compat/linux/clock.elf')
result = subprocess.run([str(binary.resolve())], capture_output=True, timeout=3)
expected = b'LINUX CLOCK ELF: monotonic/sleep/bounds compatibility PASS\n'
if result.returncode != 43 or result.stdout != expected or result.stderr:
    raise SystemExit(f'Host Linux clock fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}')
print('Independent Linux clock ELF host execution PASS (exit 43)')
binary = Path('build/compat/linux/memory.elf')
result = subprocess.run([str(binary.resolve())], capture_output=True, timeout=3)
expected = b'LINUX MEMORY ELF: brk/mmap/munmap/zeroing compatibility PASS\n'
if result.returncode != 44 or result.stdout != expected or result.stderr:
    raise SystemExit(f'Host Linux memory fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}')
print('Independent Linux memory ELF host execution PASS (exit 44)')
import tempfile
binary = Path('build/compat/linux/file.elf')
with tempfile.TemporaryDirectory(prefix='next-linux-file-') as directory:
    scratch = Path(directory) / 'fixture.txt'
    result = subprocess.run([str(binary.resolve()), str(scratch)], capture_output=True, timeout=3, stdin=subprocess.DEVNULL)
    expected = b'LINUX FILE ELF: open/read/write/seek/stat/dup/close compatibility PASS\n'
    if result.returncode != 46 or result.stdout != expected or result.stderr or scratch.read_bytes() != b'linux-static-file-data!':
        raise SystemExit(f'Host Linux file fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}')
print('Independent Linux file ELF host execution PASS (exit 46)')
binary = Path('build/compat/linux/tls.elf')
result = subprocess.run([str(binary.resolve())], capture_output=True, timeout=3)
expected = b'LINUX TLS ELF: FS/GS ownership/yield/bounds compatibility PASS\n'
if result.returncode != 47 or result.stdout != expected or result.stderr:
    raise SystemExit(f'Host Linux TLS fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}')
print('Independent Linux TLS ELF host execution PASS (exit 47)')
binary = Path('build/compat/linux/namespace.elf')
with tempfile.TemporaryDirectory(prefix='next-linux-namespace-') as directory:
    old = Path(directory) / 'old'
    new = Path(directory) / 'new'
    result = subprocess.run([str(binary.resolve()), str(old), str(new)], capture_output=True, timeout=3)
    expected = b'LINUX NAMESPACE ELF: rename/unlink/open-description/inode compatibility PASS\n'
    if result.returncode != 48 or result.stdout != expected or result.stderr or old.exists() or new.exists():
        raise SystemExit(f'Host Linux namespace fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}')
print('Independent Linux namespace ELF host execution PASS (exit 48)')
binary = Path('build/compat/linux/getpid.elf')
result = subprocess.run([str(binary.resolve())], capture_output=True, timeout=3)
expected = b'LINUX GETPID ELF: stable kernel process identity PASS\n'
if result.returncode != 51 or result.stdout != expected or result.stderr:
    raise SystemExit(f'Host Linux getpid fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}')
print('Independent Linux getpid ELF host execution PASS (exit 51)')
binary = Path('build/compat/linux/lseek.elf')
with tempfile.TemporaryDirectory(prefix='next-linux-lseek-') as directory:
    scratch = Path(directory) / 'fixture.txt'
    result = subprocess.run([str(binary.resolve()), str(scratch)], capture_output=True, timeout=3, stdin=subprocess.DEVNULL)
    expected = b'LINUX LSEEK ELF: position, signed errors, and cursor preservation PASS\n'
    if result.returncode != 53 or result.stdout != expected or result.stderr or scratch.read_bytes() != b'abcdef':
        raise SystemExit(f'Host Linux lseek fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}, file={scratch.read_bytes()!r}')
print('Independent Linux lseek ELF host execution PASS (exit 53)')
binary = Path('build/compat/linux/fstat.elf')
with tempfile.TemporaryDirectory(prefix='next-linux-fstat-') as directory:
    scratch = Path(directory) / 'fixture.txt'
    result = subprocess.run([str(binary.resolve()), str(scratch)], capture_output=True, timeout=3, stdin=subprocess.DEVNULL)
    expected = b'LINUX FSTAT ELF: identity, type, size, EBADF and EFAULT PASS\n'
    if result.returncode != 54 or result.stdout != expected or result.stderr or scratch.read_bytes() != b'fstat-linux':
        raise SystemExit(f'Host Linux fstat fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}, file={scratch.read_bytes()!r}')
print('Independent Linux fstat ELF host execution PASS (exit 54)')
PY

python3 - <<'PYGETDENTS'
from pathlib import Path
import subprocess,tempfile
binary=Path('build/compat/linux/getdents.elf').resolve()
with tempfile.TemporaryDirectory(prefix='next-linux-getdents-') as directory:
    (Path(directory)/'alpha').write_bytes(b'directory fixture')
    (Path(directory)/'child').mkdir()
    result=subprocess.run([str(binary),directory],capture_output=True,timeout=3,stdin=subprocess.DEVNULL)
    expected=b'LINUX GETDENTS ELF: records, cursor, dup, EOF and faults PASS\n'
    if result.returncode != 55 or result.stdout != expected or result.stderr:
        raise SystemExit(f'Host Linux getdents fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}')
print('Independent Linux getdents64 ELF host execution PASS (exit 55)')
PYGETDENTS
python3 - <<'PYCWD'
from pathlib import Path
import subprocess
import tempfile

binary = Path('build/compat/linux/cwd.elf').resolve()
with tempfile.TemporaryDirectory(prefix='next-linux-cwd-') as directory:
    root = Path(directory)
    (root / 'alpha').mkdir()
    (root / 'sample').write_bytes(b'cwd-sample')
    result = subprocess.run([str(binary)], cwd=root, capture_output=True, timeout=3, stdin=subprocess.DEVNULL)
    expected = b'LINUX CWD ELF: getcwd, chdir, and relative openat PASS\n'
    if result.returncode != 56 or result.stdout != expected or result.stderr:
        raise SystemExit(f'Host Linux cwd fixture failed: status={result.returncode}, stdout={result.stdout!r}, stderr={result.stderr!r}')
print('Independent Linux cwd/openat ELF host execution PASS (exit 56)')
PYCWD
