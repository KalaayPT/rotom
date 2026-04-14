# `rotom init`

- creates a .rotom folder in project root
- recognizes project type (`uxie`), creates `rotom.toml`
- checks for existing global db cache
  - if exists -> checks for updates
  - if not
    - tries downloading
    - if missing internet connection, uses last known (embedded in binary) dbs
- `rotom.toml` records:
  - project name
  - project type (dspre/decomp)
  - script directories:
    - source
    - binary
  - db revision
  - rotom version
- creates a project-local 

# database handling

- one global cache in `%LOCALAPPDATA%/rotom/cache` (win) or `~/.cache/rotom` (unix)
- one project-local, locked copy of database, saved in `project_root/.rotom/`
