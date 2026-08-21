# Design QA — email/password authentication controls

**Source visual truth**

- `/Users/fanxiao/.codex/generated_images/01a018fd-ec5a-7123-923f-26112f90b1a1/exec-049b258b-4912-40ba-8c01-c19b8ed7f7a5.png`
- Selected direction: option 2, compact segmented control with an icon-only password visibility action.

**Implementation evidence**

- Full login state on the normal service configuration: `/Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio/design-qa-artifacts/login-email-password-production-final.png`
- Masked password state: `/Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio/design-qa-artifacts/login-password-masked-final.png`
- Revealed/eye-off state: `/Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio/design-qa-artifacts/login-password-revealed-final.png`
- Password-management state: `/Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio/design-qa-artifacts/password-management-prod-open.png`
- Focused source region: `/Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio/design-qa-artifacts/source-login-controls.png`
- Focused normalized implementation region: `/Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio/design-qa-artifacts/actual-login-controls-normalized.png`

**Viewport and normalization**

- Native desktop window: `1440 × 928` logical pixels, light theme, Chinese locale.
- Source raster: `1221 × 1289` pixels at generated-image density; focused source crop: `1180 × 720` pixels.
- Implementation capture: `3104 × 2080` pixels, containing a `1440 × 928` logical-pixel Retina window plus the macOS window shadow (`2×` app-content density).
- Focused implementation evidence was cropped to `1420 × 867`, then resampled to `1180 × 720` for equal-pixel comparison with the source crop. App/window chrome and surrounding canvas were excluded from focused fidelity judgments.
- The source contains populated credentials and an error state; the final implementation capture uses the same required agreements but a clean sign-in state. Those dynamic field/error values were excluded from visual-fidelity findings. The compared state is email login with **Password login** selected and the password field masked.

**Full-view comparison evidence**

- The selected email tab, compact two-option switch, explanatory copy, form order, password affordance, forgot-password link, and primary action retain the source hierarchy.
- The implementation fits the established native dialog and theme rather than enlarging the dialog to the concept-board crop. No controls overlap, clip, or lose hierarchy at the tested viewport.
- Password management reuses the same segmented-control and secret-field components, preserving visual and behavioral consistency.

**Focused region comparison evidence**

- The equal-size comparison confirms the same compact two-segment silhouette, muted inactive surface, white active surface, purple active text/outline, centered labels, rounded corners, and eye icon placement.
- Field labels, required markers, focus border, password masking, and forgot-password alignment remain legible and balanced.
- The official Lucide `eye` / `eye-off` assets are sharp vector icons with consistent stroke weight; no text glyph, handcrafted SVG, CSS drawing, or placeholder is used.

**Required fidelity surfaces**

- Fonts and typography: existing system CJK font stack, weights, hierarchy, line height, and truncation behavior are preserved. The segmented active label uses the intended stronger weight.
- Spacing and layout rhythm: shared `320 × 44` segmented control, centered placement, internal two-pixel padding/gap, field spacing, radii, and button alignment are consistent in both dialogs.
- Colors and tokens: implementation uses the existing panel, panel-soft, border, muted, weak, danger, and accent tokens. Active, hover, disabled, and keyboard-focus states remain distinguishable.
- Image quality and asset fidelity: Lucide SVG assets render cleanly at `20 × 20` inside a `52 × 46` interaction target and share one source/license across every password field.
- Copy and content: login-mode labels match the selected direction. New-password guidance states `8–20` characters with `0–9`, `A–Z`, and `a–z`, with symbols allowed, in both Chinese and English.
- Accessibility and interaction: both segments are independent checkable buttons with checked state, default action, keyboard activation, disabled semantics, and a visible two-pixel focus ring. Visibility buttons have dynamic accessible labels, default action, Space/Enter support, and non-duplicated decorative icon semantics.

**Findings**

- No actionable P0, P1, or P2 differences remain.
- [P3] The native dialog renders the segmented control and surrounding form at a slightly denser scale than the standalone concept board. This is an intentional fit to the existing desktop dialog and does not change hierarchy, readability, or target size.

**Primary interactions tested**

- Switched email login from verification code to password and back.
- Opened Settings → Account Center → Login Methods → Change Password.
- Confirmed the change-password verification switch uses the same component.
- Entered a local example password, then toggled masked and revealed states; the icon changes from `eye` to `eye-off`.
- Confirmed no production mutation or deployment was performed during QA.

**Open Questions**

- None.

**Comparison history**

- Pass 1: compared the selected source and final normal-service implementation in one full-view input, then compared equal-pixel focused control regions in a second combined input. No P0/P1/P2 visual issue was found, so no visual-fix iteration was required.

**Implementation checklist**

- [x] Shared segmented control in login and password management.
- [x] Unified eye/eye-off icons in every secret field.
- [x] Keyboard and accessibility semantics for both control families.
- [x] Local native interaction capture for login and password management.
- [x] Equal-pixel focused comparison against the selected visual target.

final result: passed
