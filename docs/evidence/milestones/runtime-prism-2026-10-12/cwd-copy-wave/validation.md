# Validation for the cwd and Prism copy wave

All commands ran from `/workspace/Next` on the `main` worktree.

- `source tools/env.sh && bash tools/test.sh` — passed, full host workspace suite.
- `source tools/env.sh && bash tools/build-linux.sh` — passed, including `cwd.elf` on host Linux and the Linux fixture set.
- `source tools/env.sh && cargo test -p prism` — passed, 234 unit tests plus doctests.
- `python3 -m unittest test_boot_smoke_smp` from `tools/` — passed, 11 tests.
- `python3 -m py_compile tools/boot_smoke.py tools/test_boot_smoke_smp.py` and `git diff --check` — passed after smoke cleanup changes.
- `source tools/env.sh && bash tools/build.sh` — passed; coverage audit reports 195 requirements, 74 pending, 121 partial, 0 verified (`product_ready=false`).
- `python3 tools/boot_smoke.py --cpu qemu64 --timeout 360 --capture-prism-gallery --image build/next-dev.img --output-dir build/cwd-copy-clean-gallery-qemu64` — passed, 134 checks. The Linux cwd fixture exited 56; the Files copy report records the exact successful source and destination and both pane refreshes. The screenshot pass cleared transient process toasts while retaining notification history.

This is scoped development evidence. QEMU used one configured vCPU, APs stayed stopped, and scheduler work ran on the BSP; it does not establish Next 1.0 readiness, general POSIX compatibility, SMP scheduling, or physical hardware support.
