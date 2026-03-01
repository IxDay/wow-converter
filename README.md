# wow-gltf

Convert World of Warcraft M2 and WMO models from MPQ archives to **GLB** (binary glTF) — a single-file 3D format viewable in any browser, Blender, Windows 3D Viewer, macOS Quick Look, and more.

## Server

The built-in web server is the easiest way to browse and preview models. It indexes all M2/WMO files at startup, converts on demand, and caches the resulting `.glb` files to disk.

```
cargo run --bin server -- --data /path/to/Data
```

Open `http://localhost:8080`, search for a model, and click to view it in 3D. Converted files are cached in a `glb/` directory so subsequent loads are instant.

A pool of pre-opened MPQ archives is kept warm for the lifetime of the server, sized to the number of CPU cores, so concurrent requests never block on archive I/O.

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `-d, --data` | *(required)* | MPQ directory or single `.mpq` file |
| `-c, --cache` | `glb` | Cache directory for converted files |
| `-p, --port` | `8080` | HTTP port |

## CLI

Convert a single model directly:

```
cargo run --bin convert -- --data /path/to/Data barn.wmo
cargo run --bin convert -- --data /path/to/Data Lich_King.m2
```

This produces a `.glb` file in the current directory. The format embeds geometry, normals, UVs, and textures (as PNG) in a single binary — no loose files to manage.

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `-d, --data` | current directory | MPQ directory or single `.mpq` file |
| `-o, --output` | `<model>.glb` | Output path |

## List

List all M2 and WMO model paths found in the archives:

```
cargo run --bin list -- --data /path/to/Data
```

## Building

```
cargo build --release
```

Requires the `warcraft-rs` workspace at `../warcraft-rs/` for the `wow-mpq`, `wow-m2`, `wow-wmo`, and `wow-blp` crates.
