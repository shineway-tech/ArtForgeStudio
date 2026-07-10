# Current Client Merge

This source tree now contains two Rust client code paths:

- `crates/artait-app`: upstream workspace application from `ArtStudio-main`.
- `native-client`: the current ArtForgeStudio client that contains the recent UI and workflow changes.

The workspace root `Cargo.toml` includes `native-client` as a member, so the current client can be checked or built from this directory:

```powershell
cargo check -p artforge-studio-native
cargo build -p artforge-studio-native --release
```

The current executable package name remains `ArtForgeStudio`.

The `assets/sucai` folder was also synced into this source tree so the client can load the inspiration assets when packaged with the executable.
