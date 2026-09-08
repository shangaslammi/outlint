# Changelog

All notable changes to this project are documented in this file.

## [2.1.0] - 2026-09-07

### Added

- Direct item validation for release notes.

### Changed

- Diagnostic targets now identify their owning section.

### Fixed

- Content assignments remain stable during recovery.

### Security

- Schema input limits now cover every YAML source.

## [2.0.1] - 2026-08-30

### Added

- Markdown preambles are available through the core API.

### Deprecated

- The legacy release-note layout.

### Removed

- Ambiguous flat diagnostic paths.

### Fixed

- Repeated categories are scoped to their own releases.
