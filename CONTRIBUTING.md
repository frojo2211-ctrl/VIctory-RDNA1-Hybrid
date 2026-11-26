# Contributing to VIctory RDNA1 Hybrid

Thank you for your interest. This project is experimental and focused on correctness, stability, and legal/clean-room-safe engineering.

## Ground rules

- **No proprietary content:** Do not submit game assets, extracted shaders, binaries, dumps, screenshots containing copyrighted game content, or derived proprietary data.
- **No DRM/protection bypass:** Do not add or request features that circumvent DRM, authentication, licensing, or platform protections.
- **No personal data:** Do not include local paths, credentials, tokens, private logs with personal identifiers, or machine-specific secrets.
- **Owned fixtures only:** Repro tests/fixtures must be synthetic/owned and redistributable under the repository license policy.
- **Be precise about validation:** Do not claim "fixed", "compatible", or "supported" without reproducible evidence.

## Development setup

1. Install Rust toolchain from `rust-toolchain.toml`.
2. Install Visual Studio/MSVC C++ Build Tools and Windows SDK.
3. Build:
   ```powershell
   cargo build --locked -p victory
   ```
4. For diagnostics/hardware repros:
   ```powershell
   cargo build -p victory-gpu --features diagnostics --locked
   ```

## Reporting bugs / GPU compatibility

Use the GitHub issue templates. Include:
- GPU model, driver version, Windows version.
- Exact command, commit hash, environment variables.
- Whether D3D12 Debug Layer / DRED was enabled.
- Minimal owned fixture or steps to reproduce.
- Full error/HRESULT/device-removed details.
- Clear statement if issue was observed on proprietary content (do not attach content).

## Pull requests

- Keep PRs focused and small.
- Describe root cause, evidence, and validation.
- Include tests/fixtures when possible.
- Do not regress CGBJ memory architecture, RDNA1 safety, or prior validated fixes.
- If a limitation remains, document it explicitly in PR/CHANGELOG/docs.

## License

By contributing, you agree that your contributions are licensed under the repository's GPL-3.0-or-later license, unless a file explicitly states otherwise.
