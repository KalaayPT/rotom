# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/KalaayPT/rotom/compare/v0.7.0...v0.8.0) - 2026-10-21

### Added

- *(lsp)* nicer mesasge preview formatting
- *(disassembler)* collect globalscript labels before writing sources
- *(language)* allow customizing autovar return variable using
- *(language)* allow using autovar for variable params

### Fixed

- *(disassembler)* fix misidentified endmovements
- *(text archives)* fixed mismatched constant lookups on matching

### Other

- *(tests)* add pokediamond roundtrip tests and known hashes
- *(tests)* add round-trips to decomp integration tests
- autovar resolution uses normal lowering path
- more agents.md streamlining

## [0.7.0](https://github.com/KalaayPT/rotom/compare/v0.6.0...v0.7.0) - 2026-10-09

### Added

- *(decompiler)* added literal param option for e.g. SetVar, closes #10
- *(cli)* decompile --file decompiles only the named binaries in project mode

### Fixed

- *(decomp)* various decomp fixes
- *(compiler)* unknown commands fail analysis with their position instead of panicking in lowering
- *(fixtures)* tie integration tests to srccmd db revision
- *(decompiler)* keep unreferenced code that sits before a movement list in a gap
- *(project)* decompile skips empty script binaries instead of failing on them
- *(config)* read backslash paths from a Windows rotom.toml on every platform, and write forward slashes
- *(cli)* print project errors on stderr with compile --json
- *(decompiler)* number actions in offset order so every run decompiles the same

### Other

- Fix syntax in Test #1 script by moving 'oops'
- shorten agents.md
- fix test broken by upstream rename
- *(project)* keep the first paragraph of decompile_project_files short
- small clippy cleanup + docs

## [0.6.0](https://github.com/KalaayPT/rotom/compare/v0.5.0...v0.6.0) - 2026-08-02

### Added

- *(language)* add inline actions through `action()` builtin

### Fixed

- add missing `CloseMessage` in MenuBuilder lowering

### Other

- update dependencies
- oops lmao
- bump uxie ver and uxie uxies improved global constant resolution

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
