# Development VM responsiveness reference

This is a host-observed development benchmark, not a kernel IRQ-latency measurement, physical hardware certification or display/vblank guarantee. It records actual PS/2 input and completed software frames on image `60ab816c97cc2c510ac5eed76a3db2d039b924a72fe6904bc6eae63f36cbfc27`.

Run from the repository root after building, with no other positive VM validator using the local peers:

```sh
source tools/env.sh
python3 tools/latency_smoke.py --timeout 240 --output-dir build/latency
```

The profile is QEMU q35/TCG, qemu64, one CPU, 256 MiB RAM, software framebuffer and polled PS/2 input. The idle phase retains PID1 and its supervised worker. The loaded phase adds two noncooperative children, exhausting the four process slots; further spawn must reject. The children gained 85 and 77 timer preemptions during the measured phase, baseline tasks remained live, and both children were killed after measurement.

Each phase performs 20 cycles containing focus Files, focus Terminal and a verified typed `status` command. Each of the 120 samples starts at host QMP keyboard dispatch and ends when the serial observer sees the completed frame marker strictly after the matching full response. Typing setup is recorded separately; a command sample measures its Enter action. Raw timestamps, response offsets, frame generations, PIDs, process states and tool/image hashes are retained in the [report](evidence/milestones/60ab816c97cc/latency/latency-smoke.json).

| Phase / action | Samples | Median ms | p95 ms | Maximum ms |
| --- | ---: | ---: | ---: | ---: |
| Idle, all actions | 60 | 37.962 | 89.296 | 122.906 |
| Idle, focus | 40 | 36.548 | 43.296 | 44.932 |
| Idle, typed command | 20 | 78.102 | 116.109 | 122.906 |
| Loaded, all actions | 60 | 46.464 | 149.091 | 160.824 |
| Loaded, focus | 40 | 45.080 | 52.641 | 66.009 |
| Loaded, typed command | 20 | 142.004 | 157.163 | 160.824 |

p95 uses the nearest-rank ceiling of 0.95 × sample count. Loaded overall median/p95 were 1.224/1.670 times idle. No default numeric pass threshold is applied; an explicit `--max-p95-ms` can enforce a locally chosen development budget without establishing a product guarantee.

Results include host scheduling, TCG execution, QMP transport, serial observation and instrumentation delays. Idle precedes loaded in one run, so warmup, cache and drift effects are not isolated. This tenth run was scheduled independently of other positive VM validators. Host load and instrumentation still affect the observations; it is not a controlled physical-machine benchmark. The earlier fourth run overlapped persistence startup and invalidated the other validator's DNS request-count attribution; its failed persistence logs and scoped timing limitations remain retained in the fourth milestone. Further runs need controlled host load and sequential peer ownership; hardware, IPC payload and kernel interrupt benchmarks remain separate acceptance gates.
