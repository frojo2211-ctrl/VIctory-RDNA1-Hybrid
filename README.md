# VIctory RDNA1 Hybrid

An experimental, open-source fork focused on running Xbox Series X|S titles natively on Windows/D3D12 with a strong emphasis on **AMD RDNA1 (GFX1010, e.g. RX 5700 XT)** stability and compatibility research.

This repository is a **public, cleaned snapshot** prepared for review. It is **not** a complete, production-ready compatibility layer and it does **not** claim full commercial game compatibility.

## Scope and intent

- **Target hardware focus:** AMD RDNA1 / GFX1010 (RX 5700 XT validation target), Windows 11, D3D12.
- **Project status:** Experimental / research. Many rendering, synchronization, memory, and title-specific issues remain.
- **Legal/safety boundaries:** No DRM circumvention, no authentication bypass, no protection bypass, no key/license extraction, no proprietary game assets. Users must supply their own legally obtained content/dumps.

## Proven results (owned fixtures, RX 5700 XT)

The following results were obtained on **AMD Radeon RX 5700 XT (RDNA1/GFX1010)** using **owned diagnostic fixtures** in this repository, with D3D12 Debug Layer + DRED enabled where noted. They are **fixture results**, not proof of full game compatibility.

| Fixture / Test | Result | Notes |
|---|---:|---|
| Basic sync/lifetime repro (`host::sync_repro::rdna1_sync_lifetime_repro`) | **PASS, 200 frames** | D3D12 Debug Layer + DRED light. No D3D12 debug messages, no device removed in that specific test. |
| Mesh fallback compute + graphics + ExecuteIndirect (`host::sync_repro::rdna1_mesh_fallback_repro`) | **PASS, 120 frames** | D3D12 Debug Layer + DRED light. `avg_frame_us=6323` reported by fixture; `248160` lit pixels validated (`total_lit=248160`). No D3D12 debug messages, no device removed in that specific test. |
| Carded shadow refresh + Root SRV (`host::sync_repro::rdna1_carded_send_repro`) | **PASS, 40 frames** | `VICTORY_CARD_ALL=1 VICTORY_CARD_BUFFERS=1`, carded shadow path active, Root SRV read after refresh, per-frame readback validation. `avg_frame_us=1037`. No D3D12 debug messages, no device removed in that specific test. |
| RDNA1-safe perspective-correct interpolation readback | **PASS** | Hardware and WARP validation on owned readback fixtures. Conventional perspective interpolation validated; this does **not** validate custom `attributeAtVertex` barycentric cases. |

## Known limitations / open problems

These are active limitations. Do not interpret fixture passes as full compatibility.

- **Custom barycentrics / `attributeAtVertex`:** Per-vertex/weighted/custom `attributeAtVertex`-dependent barycentric cases remain **Unsupported** on hosts without native barycentrics. Conventional perspective-correct interpolation has been validated; custom barycentric-dependent cases have not been proven compatible on RDNA1.
- **Hybrid V3 complex-load stability (unresolved):** The latest Hybrid V3 integrated test reported `E_OUTOFMEMORY` during `MakeResident`, elevated RAM/VRAM pressure, and D3D12 debug IDs **527** and **538** still present. These are not resolved and must be treated as publication/runtime blockers for complex workloads.
- **DEVICE_HUNG / device removal risk:** Device-hung/device-removed scenarios are not fully root-caused or eliminated. Specific owned fixtures may pass, but this does not generalize to complex scenes/titles.
- **Carded buffers / send integration:** Carded shadow refresh/Root SRV fixture passes, but **full carded send integration** is not fully validated (complete `send::take/record/give_back` + `card_before/prepare_card_before` submission flow, dirty-range concurrent ownership, copy-back/queue ordering under pressure, backing reuse under memory pressure).
- **Streaming / complex scenes:** Streaming, residency, complex scene rendering, and real title workloads may expose synchronization, lifetime, memory pressure, or shader translation issues not covered by current fixtures. Stability and rendering of complex loads are **not proven**.
- **No complete commercial app validation:** No claim of complete compatibility for commercial games/applications. Any title behavior is experimental and may crash, hang, render incorrectly, or perform poorly.
- **BCPack optional in public snapshot:** `bcpack-tables.bin` is **not distributed** in this public snapshot due redistribution-rights review. BCPack-dependent DirectStorage compressed texture paths are unavailable and fail cleanly (`E_NOTIMPL`) unless an authorized private build supplies tables out-of-tree. Do not add or reconstruct proprietary table data.

## Hardware requirements

### Minimum / target development environment
- **OS:** Windows 11 (x64). D3D12 development/runtime components required.
- **GPU:** Direct3D 12-capable GPU. RDNA1/RX 5700 XT is the primary validation target for this fork; other GPUs may behave differently.
- **CPU:** AVX2-capable x64 CPU recommended (upstream project assumptions/tooling may require it).
- **RAM:** 16 GB+ recommended.
- **Disk:** Plenty of free space for build artifacts/caches (keep large free margins; debug builds can be large).
- **Toolchain:** Rust toolchain pinned by `rust-toolchain.toml`, Visual Studio/MSVC C++ Build Tools, Windows SDK.

### For running titles (user-supplied content)
- You must provide your own legally obtained Xbox Series X|S dump/content.
- Do **not** use this project to circumvent DRM, authentication, or protections.
- Game/asset files are **not** included and must remain outside the repository.

## Build instructions

```powershell
# From repository root
cargo build --locked -p victory

# Diagnostics-enabled build (used for owned repro fixtures)
cargo build -p victory-gpu --features diagnostics --locked
```

Notes:
- The workspace is a Rust workspace (`Cargo.toml`) with crates under `crates/` and app under `apps/victory`.
- Some tests are hardware/diagnostics-gated and marked `#[ignore]`; run them explicitly only on appropriate hardware.
- Use `--locked` where possible for reproducibility.

## Owned fixture reproduction (RX 5700 XT)

Run only on compatible hardware. These tests are diagnostics/hardware oriented and may stress GPU drivers.

```powershell
# Basic sync/lifetime repro (200 frames by default)
$env:VICTORY_D3D12_DEBUG='1'
$env:VICTORY_D3D12_DRED='light'
$env:VICTORY_SYNC_REPRO_FRAMES='200'
cargo test -p victory-gpu --lib --features diagnostics --locked host::sync_repro::rdna1_sync_lifetime_repro -- --ignored --nocapture --test-threads=1

# Mesh fallback compute + graphics + ExecuteIndirect (120 frames)
$env:VICTORY_D3D12_DEBUG='1'
$env:VICTORY_D3D12_DRED='light'
$env:VICTORY_SYNC_REPRO_FRAMES='120'
cargo test -p victory-gpu --lib --features diagnostics --locked host::sync_repro::rdna1_mesh_fallback_repro -- --ignored --nocapture --test-threads=1

# Carded shadow refresh + Root SRV (40 frames)
$env:VICTORY_D3D12_DEBUG='1'
$env:VICTORY_D3D12_DRED='light'
$env:VICTORY_CARD_ALL='1'
$env:VICTORY_CARD_BUFFERS='1'
$env:VICTORY_SYNC_REPRO_FRAMES='40'
cargo test -p victory-gpu --lib --features diagnostics --locked host::sync_repro::rdna1_carded_send_repro -- --ignored --nocapture --test-threads=1
```

Expected evidence to capture:
- Test PASS/FAIL.
- Frame count, fixture-reported counters (`total_lit`, `avg_frame_us`).
- Whether D3D12 Debug Layer emitted messages.
- Device-removed/HRESULT/DRED status (if any).

## Project layout

- `apps/victory/` - CLI/application entry points.
- `crates/victory-core/` - core config/policy/logging utilities.
- `crates/victory-gpu/` - D3D12 host, memory/shadow/carded buffers, draw/dispatch, diagnostics repros.
- `crates/victory-shader/` - shader translation/lowering/emission.
- `crates/victory-hle/`, `victory-loader/`, `victory-pe/`, `victory-formats/`, etc. - supporting subsystems.
- `docs/` - architecture and project notes.
- `tests/` - integration/fixture infrastructure (owned/synthetic fixtures only).

## License and credits

- **Repository license:** GPL-3.0-or-later (see `LICENSE`).
- **Upstream/original project:** VIctory by Marvisak and contributors. This fork preserves upstream license and attribution.
- **CGBJ/Cool Fox lineage:** This fork includes/integrates work derived from the Cool Fox / CGBJ line as part of its experimental history. Preserve original credits and license notices in redistributed source.
- **BCPack tables not distributed:** `bcpack-tables.bin` is intentionally absent from this public snapshot pending redistribution-rights confirmation. BCPack-dependent functionality is optional and disabled in this snapshot; authorized private builds may supply tables out-of-tree.

See `CREDITS.md` for attribution details.

## Safety / compliance

- No proprietary game assets, extracted shaders, or copyrighted game data are included.
- No DRM/authentication/protection bypass mechanisms are included or documented.
- Reports must use owned/synthetic fixtures or legally obtained user-local content that is not redistributed.

## Disclaimer

This software is provided for research/interoperability/experimental purposes. It may be unstable, incomplete, and incompatible with many titles and hardware/driver combinations. Use at your own risk.
