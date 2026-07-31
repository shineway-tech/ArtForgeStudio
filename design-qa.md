# 图片裁剪 Design QA

- Result: `passed`
- Reference: `/Users/fanxiao/.codex/generated_images/019fb1ac-d6ee-7d12-92b0-3cef9678696f/call_8YThL9mLtp5xAtn1Yz3ECgLw.png`
- Final editor state: `/private/tmp/artforge-crop-final-3x4.png`
- Save confirmation: `/private/tmp/artforge-crop-final-saved.png`
- Asset confirmation: `/private/tmp/artforge-assets-final-crop.png`
- Side-by-side comparison: `/private/tmp/artforge-crop-final-comparison.png`
- Comparison viewport: reference cropped to 1487 × 958; implementation normalized to 1487 × 958.

## Verified states

- Empty state supports click upload, drag-and-drop, and paste guidance.
- The async macOS file picker opens without terminating the Slint event loop.
- A non-square 1448 × 1086 image is fitted without distortion.
- Original, free, 1:1, 4:3, 3:4, 16:9, and 9:16 ratios are available.
- 3:4 produces an 815 × 1086 crop and keeps the source pixels without upscaling.
- Crop movement, corner resizing, rotation, flips, reset, and image replacement are wired.
- Saving is local, displays `0积分`, and does not invoke a credit-consuming API.
- The saved PNG appears in `我的资产 > 其他` as an `图片裁剪` asset.
- Crop assets do not expose regenerate or re-edit actions.

## Visual review

- The loaded editor starts directly below the application header, matching the reference hierarchy.
- The editor and inspector are balanced for both landscape and portrait crop selections.
- Settings and save actions are separated into two cards, matching the selected direction.
- No P0, P1, or P2 visual, interaction, or accessibility blockers remain.

## Automated verification

- `cargo check -p artforge-studio-native`
- `cargo test -p artforge-studio-native local_crop_uses_normalized_bounds_and_applies_transforms -- --nocapture`
- `cargo test -p artforge-studio-native toolbox_crop_is_a_free_local_editor_that_saves_other_assets -- --nocapture`
- Full native-client suite previously completed with 222 passed, 0 failed, and 39 ignored mock-API tests.
