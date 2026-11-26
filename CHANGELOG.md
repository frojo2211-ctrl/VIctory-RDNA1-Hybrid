# Changelog

All notable changes to this public snapshot should be documented here.

The format is based on Keep a Changelog principles, adapted for an experimental research fork.

## [Unreleased]

### Added
- Public repository preparation: cleaned documentation, contribution guide, roadmap, issue templates, and audit notes.
- Owned RX 5700 XT fixture documentation: 200-frame basic sync repro, 120-frame mesh fallback ExecuteIndirect repro (248160 lit pixels), 40-frame carded shadow refresh + Root SRV repro, perspective-correct interpolation validation.
- Explicit limitation documentation for custom barycentrics/`attributeAtVertex`, DEVICE_HUNG/ID 538, carded send integration, streaming/complex scenes, and lack of complete commercial validation.

### Known Issues
- Custom `attributeAtVertex`/barycentric-dependent cases are not fully supported on RDNA1 hosts lacking native barycentrics.
- DEVICE_HUNG/ID 538 scenarios remain unresolved in general.
- Full carded send/memory integration and complex scene streaming require further validation.
