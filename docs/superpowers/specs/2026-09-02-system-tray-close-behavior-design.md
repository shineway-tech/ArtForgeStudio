# System Tray Close Behavior Design

## Goal

When the main-window close button is used, Elunvi Canvas must either exit or hide to the operating-system tray according to a persisted user preference. Users without a stored preference must choose on their first close, and can change the choice later under Settings > Basic Settings.

## Behavior

- The persisted values are `exit` and `tray`; an absent, empty, or unknown value is treated as `ask`.
- On `ask`, closing keeps the window visible and opens a blocking two-choice dialog: **Exit application** or **Minimize to system tray**.
- Choosing an option persists it before performing the action.
- `tray` hides the window so it no longer appears on the taskbar while the process and tray icon remain alive.
- Left-clicking the tray icon restores and activates the main window.
- The tray context menu contains **Open main window** and **Exit application**.
- Tray-menu exit always exits, regardless of the persisted close preference.
- The Basic Settings selector changes and immediately persists the preference without closing or hiding the window.

## Architecture

Slint's built-in `SystemTrayIcon` owns the tray icon and native menu. `AppWindow` close interception resolves the persisted string through a small normalized decision function, then either opens the dialog, hides the window, or exits the event loop. The close preference is an additive `#[serde(default)]` field in `UserProfileData`, preserving backward compatibility with existing profiles.

## UI

The close-choice dialog follows the existing centered modal styling, uses bilingual copy, and cannot be dismissed by clicking the backdrop because the user must make an explicit first-time choice. Basic Settings adds a two-option close behavior row using the same selected-option visual language as card style.

## Verification

- Unit tests cover normalization and old-profile deserialization.
- UI/source integration tests cover the modal, settings selector, tray component, and callback wiring.
- A release build and Windows portable package must succeed without deleting or replacing the existing portable `data` directory.
