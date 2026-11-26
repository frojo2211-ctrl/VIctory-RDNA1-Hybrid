# Roadmap

This roadmap is intentionally conservative. Items move only after reproducible validation on owned fixtures or clearly documented evidence.

## Near-term

- [ ] **P0 - Hybrid V3 complex-load blockers (highest priority)**
  - Investigate and fix `E_OUTOFMEMORY` from `MakeResident` under elevated RAM/VRAM pressure.
  - Capture and resolve D3D12 debug IDs **527** and **538** with Debug Layer + DRED evidence on owned fixtures.
  - Add stress repros for residency/memory pressure and complex render workloads; do not claim stability until proven.
- [ ] **P1 - Carded/send integration repro coverage**
  - Cover `send::take/record/give_back`, `Host::card_before/prepare_card_before`, dirty-range concurrency, copy-back/queue ordering, Root SRV after upload through recorded send path, and backing reuse under memory pressure.
  - Capture D3D12 Debug Layer + DRED evidence on RX 5700 XT.
- [ ] **P2 - RDNA1 barycentric alternative**
  - Investigate mathematically correct alternative for per-vertex/weighted/custom `attributeAtVertex` cases on hosts without native barycentrics.
  - Preserve native barycentric path where supported.
  - Validate with owned readback fixtures; keep unsupported cases explicit until proven.
- [ ] **P3 - HUNITY-inspired profile comparison**
  - Compare default vs conservative profile only on equivalent fixtures that actually exercise toggles.
  - Record RAM, VRAM, frame time, and visual integrity.

## Medium-term

- [ ] Expand owned synchronization/lifetime fixtures for queue dependencies, multi-list, scratch reuse, and memory pressure.
- [ ] Improve device-removed/ID 538 diagnostics and minimal reproducible fixtures.
- [ ] Document GPU/driver compatibility matrix with evidence.

## Long-term / research

- [ ] Streaming/residency robustness for complex scenes.
- [ ] Broader shader translation compatibility with strict correctness checks.
- [ ] Performance profiling on RDNA1 with reproducible benchmarks.

## Non-goals

- No DRM/authentication/protection circumvention.
- No distribution of proprietary game assets, extracted shaders, or copyrighted data.
- No claim of complete commercial game compatibility without evidence.
