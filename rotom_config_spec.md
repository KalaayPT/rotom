# `rotom init` and `rotom.toml`

## Scope

`rotom init` prepares a project for Rotom by doing four things:

- create `rotom.toml` if it does not exist yet
- create `.rotom/`
- seed `.rotom/command_database/`
- detect enough workspace info to write sensible defaults

The generated config is intentionally small; compile state and dependency tracking live under `.rotom/status/` as they are needed.

## Project layout

```text
project/
├── rotom.toml
└── .rotom/
    ├── command_database/
    ├── cache/
    └── status/
```

`command_database` stores the project-local copy of the script command database.

`cache` stores cached Uxie header and include data.

`status` stores file hashes and dependency metadata for compile-state tracking.

## Database bootstrap

Init bootstraps `.rotom/command_database/` like this:

1. If the directory already exists and is not empty, leave it alone.
2. Otherwise download the latest rolling DB release:
   `https://github.com/DS-Pokemon-Rom-Editor/scrcmd-database/releases/latest/download/db-latest.zip`
3. If that fails, fall back to the DB snapshot baked into the Rotom build.

## Config shape

Typical generated config:

```toml
format_version = 1

[project]
name = "pokeplatinum"

[workspace]
project_type = "decomp"
game_family = "platinum"

[paths]
database_dir = ".rotom/command_database"
cache_dir = ".rotom/cache"
status_dir = ".rotom/status"
source_roots = ["res/field/scripts"]
include_roots = ["include", "generated", "res/field/scripts"]

[database]
default_file = ".rotom/command_database/platinum_v2.json"
```

## Detection

Init detects the project type by checking for known file and directory markers:

- `decomp` — detected by the presence of `include/constants`, `scripts.order`, `src/script_manager.c`, `src/fieldmap.c`, or `files/fielddata/script/scr_seq`
- `dspre` — detected by the presence of `header.bin`, `config.yaml`, or `unpacked/`

Within a decomp project, the game family is inferred from project-specific markers. Within a DSPRE project, it is read from the Uxie ROM header.

## Related Runtime State

Other project-local state is kept separate from the config:

- `.rotom/cache/` stores cached Uxie header and include data
- `.rotom/status/` tracks file hashes and dependency metadata for compile state
