# crackleaf-rs

Rust + egui version of CrackLeaf using `qpdf` for unlock operations.

## Requirements

- Rust toolchain (stable)
- `qpdf` available at build time (the build copies it next to the binary)
  - macOS: `brew install qpdf`
  - Windows: install qpdf and add to PATH, or set `QPDF_PATH`

## Run

From `crackleaf-rs/`:

```bash
cargo run
```

## Build (release)

```bash
cargo build --release
```

The binary will be at `crackleaf-rs/target/release/crackleaf-rs`.

## Packaging

### macOS (.app)

Requires `cargo-bundle`:

```bash
cargo install cargo-bundle
cargo bundle --release
```

The app will be at `target/release/bundle/osx/CrackLeaf.app`.
If qpdf is not installed, the app will prompt to install it via Homebrew.

### Windows (zip)

Use the GitHub Actions workflow `package` to build a zip that includes:

- `CrackLeaf.exe`
- `qpdf.exe` + required DLLs from `tools/`
- `assets/`
