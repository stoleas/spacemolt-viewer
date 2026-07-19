# SpaceMolt icon set

Tauri v2 requires icon files referenced by `tauri.conf.json`'s `bundle.icon`
list. For Task 1 we only need a placeholder so `cargo build` succeeds — the
final icons will be generated from the SpaceMolt logo in Task 19.

The `tauri icon` command generates all required sizes from a single source
PNG:

```bash
npm run tauri icon path/to/source.png
```

If the source isn't available yet, `create-tauri-app` ships a default Tauri
icon set that can be copied here. This directory is created empty so the
build can proceed once icons are present.