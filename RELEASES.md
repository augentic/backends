## 0.25.0

Unreleased

### Changed

- Upgraded Omnia SDK from 0.30.0 to 0.31.0
- Upgraded Azure SDK dependencies to align on `azure_core` 0.34:
  - `azure_core` 0.33.0 → 0.34.0
  - `azure_identity` 0.33.0 → 0.34.0
  - `azure_storage_blob` 0.10.1 → 0.11.0
  - `azure_security_keyvault_secrets` 0.12.0 → 0.13.0
- Upgraded crate-level dependencies:
  - `jsonschema` 0.45.0 → 0.46.2 (kafka)
  - `redis` 1.1.0 → 1.2.0 (redis)
- Adapted `azure-blob` to `azure_storage_blob` 0.11.0 API (download range now uses `Range<usize>`, body accessed via field); fixed potential overflow in range calculation on 32-bit target
- Replaced `Duration::from_secs(10 * 60)` with `Duration::from_mins(10)` in NATS key-value config

<!-- Release notes generated using configuration in .github/release.yaml at main -->

## What's Changed

- Bump to 0.23.0 by @github-actions[bot] in https://github.com/augentic/backends/pull/18
- Upgrade omnia by @andrew-goldie in https://github.com/augentic/backends/pull/19
- vet omnia 0.30.0 by @andrew-goldie in https://github.com/augentic/backends/pull/20
- Patch wasmtime by @karthik-phl in https://github.com/augentic/backends/pull/22
- Update to use Omnia 0.31.0 by @karthik-phl in https://github.com/augentic/backends/pull/23

**Full Changelog**: https://github.com/augentic/backends/compare/v0.23.0...v0.25.0

---

Release notes for previous releases can be found on the respective release
branches of the repository.

<!-- ARCHIVE_START -->

- [0.25.x](https://github.com/augentic/backends/blob/release-0.25.0/RELEASES.md)
- [0.24.x](https://github.com/augentic/backends/blob/release-0.24.0/RELEASES.md)
- [0.23.x](https://github.com/augentic/backends/blob/release-0.23.0/RELEASES.md)
