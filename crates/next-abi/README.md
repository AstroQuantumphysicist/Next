# next-abi

Dependency-free `no_std` contracts for the current Next development milestone.
These definitions are **development ABI 0.2**, not the frozen Next 1.0 ABI.

## Boot handoff

`boot::BootInfo` is a 168-byte, eight-byte-aligned C record for x86-64.
Fields contain physical addresses. `BootInfo::validate` checks the record's
version, size, reserved flags, additive checksum, framebuffer extent and memory
map metadata and distinct nonempty loader/kernel ranges; it never dereferences addresses. Call `seal` after filling all
fields and immediately before handoff. The wrapping sum of the record's 21
little-endian `u64` words, including its checksum word, is zero.
This checksum detects accidental corruption and provides no authentication.

Framebuffer pixels are four bytes per pixel, RGB or BGR with an unused fourth
byte. Stride is measured in pixels. UEFI memory descriptors are parsed from an
already mapped byte slice with `MemoryRegions::parse`; unknown descriptor kinds
are preserved, and only kind 7 is reported as conventional memory. The caller
is limited to 4096 descriptors, bounding overlap validation on corrupt maps. It
must reserve the loader, kernel, boot information and other live allocations
before using a conventional region. Validation does not prove the map was
obtained from firmware or that address ranges are mapped or device-safe.

The loader passes this record to a separate fixed-address Helix ELF. Kernel,
loader (including the record), and memory-map allocations must be completely
covered by retained UEFI LoaderCode/LoaderData descriptors. Only conventional
memory is eligible for the development allocator. None of the occupied loader
allocations are reclaimed. `MemoryRegions::is_loader_owned` verifies contiguous
coverage even for unsorted and adjacent firmware descriptors.

The final specification's individually sized optional/mandatory section records,
entropy, deployment and TPM records remain future work. Version mismatches and
unknown flags currently fail closed.

## ELF inspection

`elf::Elf64::parse` supports ELF64 little-endian x86-64 ET_EXEC/ET_DYN inspection.
It bounds program headers, validates file and address arithmetic, canonical
four-level virtual addresses, alignment, executable entrypoints and W^X.
Load segments sharing a 4 KiB page are rejected, even if byte ranges are disjoint.
The entry must be inside file-backed executable bytes, not BSS. Extended header
numbering is unsupported. Inspection deliberately performs no allocations.

Parsing does **not** execute programs, apply relocations, resolve dynamic
dependencies, authorize images, implement ASLR or establish process isolation.
ET_DYN addresses and entrypoints are unrelocated; a future loader must apply a
validated load bias and recheck resulting addresses and page permissions.

## Development kernel load policy

`kernel::KernelImage` accepts an ET_EXEC with only readable PT_LOAD records,
matching physical/virtual addresses, distinct page-aligned segments and a
file-backed executable entry. The image starts at 64 MiB, spans at most 16 MiB,
and the file is at most 8 MiB. Unknown/dynamic records and relocatable kernels
are rejected. Copying zeroes BSS and gaps inside the exclusive reserved span.
UEFI allocations establish exclusive ownership before copying; signatures,
ASLR, initrd/services and the final extensible boot contract remain outstanding.

## Native definitions

`Rights`, `Status` and `Syscall` provide development wire values and an explicit
x86-64 register convention. They are interface definitions, not evidence of
implemented kernel syscalls. Unknown rights must be denied by callers.

Validation:

```sh
cargo test -p next-abi
cargo clippy -p next-abi --all-targets -- -D warnings
cargo check -p next-abi --target x86_64-unknown-uefi
```
