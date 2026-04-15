# `rotom init` and `rotom.toml`

## Scope

The first real `rotom init` should do four things:

- create `rotom.toml` if it does not exist yet
- create `.rotom/`
- seed `.rotom/command_database/`
- detect enough workspace info to write sensible defaults

Everything else belongs to later cache/dependency work.

## Project layout

```text
project/
├── rotom.toml
└── .rotom/
    ├── command_database/
    ├── cache/
    └── status/
```

`command_database` is the project-local DB copy.

`cache` is for persistent Rotom/Uxie cache data later.

`status` is for later compile-state tracking.

## Database bootstrap

Init should bootstrap `.rotom/command_database/` like this:

1. If the directory already exists and is not empty, leave it alone.
2. Otherwise download the latest rolling DB release:
   `https://github.com/DS-Pokemon-Rom-Editor/scrcmd-database/releases/latest/download/db-latest.zip`
3. If that fails, fall back to the DB snapshot baked into the Rotom build.

## Config shape

Current target:

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

For now init only needs simple detection:

- `decomp`
  - usual markers like `include/constants`, `scripts.order`, `src/script_manager.c`, `src/fieldmap.c`, `files/fielddata/script/scr_seq`
- `dspre`
  - usual markers like `header.bin`, `config.yaml`, `unpacked/`

Family detection can stay simple too:

- decomp: infer from known project markers
- DSPRE: use Uxie ROM header detection

## Later work

The next layer after this is:

- have compile/decompile use `rotom.toml` by default
- use `.rotom/cache/` for persistent Uxie header/include caches
- track include dependencies and stale state in `.rotom/status/`
- add proper native C-header dependency handling in the compile pipeline
