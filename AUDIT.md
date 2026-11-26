# Public Release Audit

This document records the audit performed while preparing `D:\VIctory-Hybrid-Public` from the private workspace checkpoint `7cc6bc3`.

## Scope

- Source: private workspace (not published), checkpoint `7cc6bc3`.
- Destination: independent public-preparation copy.
- Method: export tracked files from checkpoint via `git archive`, remove private/sensitive/unwanted files, rewrite public docs, add community/governance files.

## Included

- Tracked source code from checkpoint `7cc6bc3` (Rust workspace crates/apps/xtask/tests/docs/scripts/config).
- `LICENSE` (GPL-3.0-or-later, preserved).
- `crates/victory-formats/src/bcpack-tables.bin` (required BCPack format constant table; documented exception in asset policy).
- New public docs: `README.md`, `CONTRIBUTING.md`, `CHANGELOG.md`, `ROADMAP.md`, `CREDITS.md`, `SECURITY.md`, `AUDIT.md`.
- GitHub issue templates under `.github/ISSUE_TEMPLATE/`.

## Excluded / removed from public copy

- `HANDOFF-CODEX.md` (private handoff; contained local paths/context).
- `PROGRESS.md` (private working log; contained local paths and internal workflow details).
- `Play-VIctory.cmd` (upstream-specific launcher with game-name/path assumptions; not appropriate for clean public snapshot).
- `docs/README-fxceless.md`, `docs/fork-ray-tracing.md` (upstream fork marketing/notes; replaced by neutral public README).
- Untracked private logs/files from workspace: `teste-hybrid-preview*.log`, `_tmp_patch.txt`.
- No `target/`, caches, dumps, binaries, or game assets copied.

## Personal/sensitive data checks

Searched tracked content for common personal path, username, local email, private release URL, token, secret, and private key patterns. 

- Removed files containing private local paths.
- Public docs rewritten to avoid private machine paths.
- Git history in public copy is **new** (fresh `git init`) to avoid leaking prior local commit metadata; original private history remains in private repo.

## License / redistribution review

- Workspace declares `GPL-3.0-or-later` (`Cargo.toml`, crate manifests, `LICENSE`).
- Upstream VIctory/Cool Fox lineage credited; GPL license preserved.
- `bcpack-tables.bin` retained because project asset policy marks it as a required format-constant exception; it is not game content. Redistribution should keep this notice visible.
- No proprietary game assets, extracted shaders, dumps, or commercial binaries included.

## Risks / follow-up before publishing

- **Resolved for public snapshot - BCPack tables:** `bcpack-tables.bin` removed from tracked files and purged from public git history. BCPack is optional in this snapshot and fails cleanly when tables are unavailable. Do not re-add proprietary/unauthorized table data.
- **History rewrite decision:** Public copy has clean history by design. If you want to publish original history, it must be scrubbed for personal/local metadata and sensitive references first.
- **Upstream attribution completeness:** Review `CREDITS.md` and add any missing contributors/copyright notices required by GPL/source provenance.
- **Docs accuracy:** README includes fixture results provided by project validation; ensure they map to the exact commit/tests you publish.
- **Pre-existing formatting debt:** `cargo fmt --check` reports pre-existing formatting differences in untouched code; resolve before CI/public release.
- **Hybrid V3 blockers:** `E_OUTOFMEMORY` in `MakeResident`, elevated RAM/VRAM pressure, D3D12 debug IDs 527/538, incomplete custom barycentrics, and unproven complex-load stability/rendering remain unresolved.
- **No push performed:** This audit does not publish or push anything.
