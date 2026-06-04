# Installing Rotom

Rotom is most useful when it is wired into an existing DS decomp or DSPRE-style
project so your normal build can compile `.rotom` scripts automatically.

Before changing a project build, make a backup!

## Install Binaries

Download a release zip, extract it, and put the extracted folder on your
`PATH`. Keep these files together:

| File | Purpose |
| ---- | ------- |
| `rotom` / `rotom.exe` | Compiler CLI |
| `rotom-lsp` / `rotom-lsp.exe` | Language server |
| `libnitroarc_ffi.so` / `nitroarc_ffi.dll` | Native library used by uxie |

Verify the CLI is available:

```bash
rotom --version
```

## Project Setup

### DSPRE Projects

Run this in the project root:

```bash
rotom init
```

`rotom init` detects the project type, downloads the command database, and writes
`rotom.toml` plus the `.rotom/` project directory. It also asks whether to convert
the project to Rotom format. If conversion fails, backup and remove the generated `.script`
files and run:

```bash
rotom decompile
```

After setup, open any `.rotom` file in your editor. The language server will pick
up the project automatically if you have the rotom extension installed and `rotom-lsp` is on your `PATH`.

### pokeplatinum Decomp

Run this in the project root:

```bash
rotom init
rotom convert
```

`rotom init` detects the project type and downloads the command database.
`rotom convert` translates all `.s` scripts to `.rotom` and backs up the originals
under `.rotom/backups/`.

Then replace `res/field/scripts/meson.build` so Meson runs `rotom compile` before
packing `scr_seq.narc`:

```meson
rotom_exe = find_program('rotom', required: true)

rotom_compile = custom_target('rotom_compile',
    output: 'rotom_compile.stamp',
    command: [
        'sh', '-c',
        'cd "@0@" && "@1@" compile && touch "@2@/rotom_compile.stamp"'.format(
            meson.project_source_root(),
            rotom_exe.full_path(),
            meson.current_build_dir(),
        ),
    ],
    build_always_stale: true,
)

scr_seq_narc = custom_target('scr_seq.narc',
    output: ['scr_seq.narc', 'scr_seq.naix'],
    depends: rotom_compile,
    command: [
        nitroarc_exe,
        '--create',
        '--index',
        '--files-from', files('scripts.order'),
        '--file', '@OUTPUT0@',
        '@PRIVATE_DIR@',
    ],
)

nitrofs_files += scr_seq_narc
naix_headers += scr_seq_narc[1]
```

Reconfigure Meson once so it reads the replacement build file:

```bash
subprojects/meson-1.10.0/meson.py setup --reconfigure build
```

After that, run `make` as normal. Ninja will call `rotom compile` and pack the
script NARC.

### hg-engine Projects

Two build files need changes. After that, the first `make` initializes Rotom and
decompiles the scripts automatically.

Note: this also replaces .txt archives with pokeplatinum-style chatot json archives!

In `Makefile`, add this near the top, after the ROM validation block:

```diff
+# Rotom integration check
+ROTOM_STATE := $(wildcard .rotom/status/compile-state.json)
MAC = $(shell uname -s | grep -i -q 'darwin'; echo $$?)
```

Then add this at the end of the `all` target:

```diff
	@echo "Done.  See output $(BUILDROM)."
+ifeq ($(ROTOM_STATE),)
+	@echo "Initializing rotom and decompiling scripts..."
+	rotom init --non-interactive || true
+	rotom decompile || true
+	uxie text-decode $(BUILD)/text .rotom/text/ || true
+endif
```

In `narcs.mk`, replace the `$(SCR_SEQ_NARC)` rule:

```diff
-$(SCR_SEQ_NARC): $(SCR_SEQ_DEPENDENCIES)
-	$(NARCHIVE) extract $(SCR_SEQ_TARGET) -o $(SCR_SEQ_DIR) -nf
-	for file in $^; do $(ARMIPS) $$file; done
-	$(NARCHIVE) create $@ $(SCR_SEQ_DIR) -nf
-
-# for convenience, rebuild SCR_SEQ_NARC every build so that DSPRE changes are not overwritten
-.PHONY: $(SCR_SEQ_NARC)
+ifneq ($(ROTOM_STATE),)
+# Rotom script mode: compile from .rotom sources.
+.PHONY: $(SCR_SEQ_NARC)
+$(SCR_SEQ_NARC):
+	rotom compile
+	$(NARCHIVE) create $@ $(SCR_SEQ_DIR) -nf
+else
+# Armips script mode: assemble from armips/scr_seq.
+$(SCR_SEQ_NARC): $(SCR_SEQ_DEPENDENCIES)
+	$(NARCHIVE) extract $(SCR_SEQ_TARGET) -o $(SCR_SEQ_DIR) -nf
+	for file in $^; do $(ARMIPS) $$file; done
+	$(NARCHIVE) create $@ $(SCR_SEQ_DIR) -nf
+# for convenience, rebuild SCR_SEQ_NARC every build so that DSPRE changes are not overwritten
+.PHONY: $(SCR_SEQ_NARC)
+endif
```

Replace the `$(MSGDATA_NARC)` rule:

```diff
-$(MSGDATA_NARC): $(MSGDATA_DEPENDENCIES) $(MSGDATA_COMPILETIME_DEPENDENCIES)
-	$(NARCHIVE) extract $(MSGDATA_TARGET) -o $(MSGDATA_DIR) -nf
-	for file in $(MSGDATA_DEPENDENCIES); do $(PYTHON) tools/source/dumptools/validate_text_archive.py $(CHARMAP) $$file || exit 1; done
-	for file in $^; do $(MSGENC) -e -c $(CHARMAP) $$file $(MSGDATA_DIR)/7_$$(basename $$file .txt); done
-	$(NARCHIVE) create $@ $(MSGDATA_DIR) -nf
+ifneq ($(ROTOM_STATE),)
+# Rotom text mode: encode from .rotom/text/ .json sources.
+.PHONY: $(MSGDATA_NARC)
+$(MSGDATA_NARC):
+	$(NARCHIVE) extract $(MSGDATA_TARGET) -o $(MSGDATA_DIR) -nf
+	uxie text-encode .rotom/text/ $(MSGDATA_DIR)
+	$(NARCHIVE) create $@ $(MSGDATA_DIR) -nf
+else
+# Original hg-engine text mode.
+$(MSGDATA_NARC): $(MSGDATA_DEPENDENCIES) $(MSGDATA_COMPILETIME_DEPENDENCIES)
+	$(NARCHIVE) extract $(MSGDATA_TARGET) -o $(MSGDATA_DIR) -nf
+	for file in $(MSGDATA_DEPENDENCIES); do $(PYTHON) tools/source/dumptools/validate_text_archive.py $(CHARMAP) $$file || exit 1; done
+	for file in $^; do $(MSGENC) -e -c $(CHARMAP) $$file $(MSGDATA_DIR)/7_$$(basename $$file .txt); done
+	$(NARCHIVE) create $@ $(MSGDATA_DIR) -nf
+endif
```

## Editor Support

`rotom-lsp` powers diagnostics, completion, hover, go-to-definition, inlay hints,
signature help, and code lenses. Editor extensions are published separately.

Tree-sitter grammar and highlighting live in
[`tree-sitter-rotom`](https://github.com/KalaayPT/tree-sitter-rotom).

## Building From Source

Install a recent stable Rust toolchain with [rustup](https://rustup.rs). Rotom uses
Rust 2024, so Rust 1.85 or newer is required.

Install the CLI:

```bash
cargo install --path .
```

Install the language server:

```bash
cargo install --path rotom-lsp
```

Or build everything without installing:

```bash
cargo build --release --workspace
```

The binaries are written to `target/release/`.
