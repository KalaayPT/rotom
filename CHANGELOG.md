# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/KalaayPT/rotom/compare/v0.4.0...v0.5.0) - 2026-07-31

### Fixed

- *(globalscripts)* fix globalscript heuristic, now emits raw slot ids

## [0.4.0](https://github.com/KalaayPT/rotom/compare/v0.3.0...v0.4.0) - 2026-07-27

### Added

- *(globalscripts)* add project-wide globalscript references and
- *(levelscripts)* tighten levelscript validation
- *(language)* add menu builder syntax for easy menu definition

### Fixed

- *(lowering)* fix not-conditions being ignored

### Other

- merge release worflows

## [0.3.0](https://github.com/KalaayPT/rotom/compare/v0.2.0...v0.3.0) - 2026-07-12

### Fixed

- *(lowering)* fix AND/OR lowering

### Other

- fix tags on release

## [0.2.0](https://github.com/KalaayPT/rotom/compare/v0.1.4...v0.2.0) - 2026-07-12

### Added

- *(language)* add truthiness/automatic resolution to flag checks

### Fixed

- *(db)* fix broken db resolution in uxie

### Other

- improve test coverage
- improve test coverage, especially in decompiler and lsp
- add code coverade reporting
- use snafu for error handling
- fix canary release replacement
