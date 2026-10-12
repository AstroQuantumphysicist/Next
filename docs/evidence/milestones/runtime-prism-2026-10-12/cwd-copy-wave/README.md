# Linux cwd and Prism cross-pane copy evidence

The qemu64 q35/TCG/OVMF smoke passed **134 checks** on signed development image SHA-256 `63374568a17fcd592fdabe24008abba36975ab48732a2b044b0f3efa1a6c36a6`. The full report is in [`boot-smoke.json`](boot-smoke.json); [`serial.log.gz`](serial.log.gz) preserves the guest trace, and [`capture-manifest.json`](capture-manifest.json) binds each documented 1280×800 PNG to its original guest PPM hash. The signed image is not checked into this bundle; [`build-manifest.json`](build-manifest.json) records its signed boot artifacts.

The original static Linux `cwd.elf` printed `LINUX CWD ELF: getcwd, chdir, and relative openat PASS` and exited with status 56. It exercises NUL-inclusive `getcwd`, ERANGE/EFAULT, `chdir`, failure preservation, path canonicalization/confinement, `AT_FDCWD`, and directory-fd-relative `openat`.

Prism Files copied a guest-created regular file from `/native/alpha` into `/native` using the existing Inspect/Commit authorization flow. The report records the exact successful source and destination, followed by successful refreshes of both pane listings. The screenshot is [`images/prism-files-copy-to-other-pane.png`](images/prism-files-copy-to-other-pane.png); all 17 current screenshots are retained under [`images/`](images/).

The [source manifest](source-manifest.json) binds the tested image to the implementation inputs. Validation commands and results are in [`validation.md`](validation.md). This run used one configured vCPU, left APs stopped, and ran scheduler work on the BSP. The requirement ledger still reports 74 pending, 121 partial, and 0 verified; no product-readiness or hardware-certification claim follows.
