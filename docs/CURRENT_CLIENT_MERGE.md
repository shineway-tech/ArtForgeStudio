# Current Client Layout

The source tree retains two generations of client code, but only one is active:

- `native-client`: the ArtForgeStudio product source and the only workspace member.
- `crates/`: archived modular migration sources, excluded from the workspace.

Normal and workspace-wide Cargo commands therefore build only `native-client`:

```powershell
cargo check -p artforge-studio-native
cargo test -p artforge-studio-native
cargo build --release -p artforge-studio-native --bin ArtForgeStudio
cargo build --release --workspace
```

The only application output is `ArtForgeStudio` (`ArtForgeStudio.exe` on Windows).
The archived `crates/artait-app/src/main.rs` is retained for reference but is not
declared as a Cargo binary target.

The `assets/sucai` folder remains available to the active client for packaged
inspiration assets.
