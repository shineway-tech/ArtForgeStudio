# Native Client Architecture

`native-client` is the product client and builds the `ArtForgeStudio` executable.

## Rust layers

- `src/main.rs`: platform-specific executable entry point.
- `src/lib.rs`: library entry point used by the executable and tests.
- `src/runtime/app.rs`: application startup and top-level callback wiring.
- `src/runtime/callbacks/`: Slint callback adapters grouped by feature.
- `src/runtime/generation/`: generation state, orchestration, and polling.
- `src/runtime/api/`: platform API client, secure session, account, order, membership, notification, upload and generation contracts.
- `src/runtime/services/`: local image processing only; model and payment traffic never bypasses the platform API.
- `src/runtime/payment_window.rs`: HTTPS-only checkout abstraction; Windows uses WebView2 and other development platforms use the system browser.
- `src/runtime/storage/`: application paths and local persistence.
- `src/runtime/features/`: viewer, account, and inspiration workflows.
- `src/runtime/presentation/`: Slint model synchronization and theme application.

`AppContext` owns the mutable store and generation registry. Background work returns
results through channels; Slint state is updated on the UI event loop.

## Slint layers

- `ui/app.slint`: `AppWindow` composition only.
- `ui/types.slint`: Rust-visible UI data structures.
- `ui/app-state.slint`: compatibility state and callback surface.
- `ui/theme.slint`: runtime color palette.
- `ui/components/`: reusable controls and feature components.
- `ui/pages/`: full application pages.
- `ui/dialogs/`: modal workflows and overlays.

## Verification

```sh
cargo check -p artforge-studio-native
cargo test -p artforge-studio-native
cargo check -p artforge-studio-native --target x86_64-pc-windows-msvc
```

The Windows check/release build must run on a Windows runner with the MSVC toolchain and
Windows SDK. See `MIGRATION.md` for migration behavior and retained local data.
