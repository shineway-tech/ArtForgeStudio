# Character Viewer Workflow Shortcuts Design

## Goal

When a character image is open in the image detail viewer, its right-click menu offers shortcuts to Character Age Progression, Character Outfit Swap, and Character Body Variations. Choosing a shortcut opens that preset's existing independent infinite-canvas workspace and adds the current image to the workspace reference strip.

## Interaction

- The three shortcuts appear only when the viewed asset's stored category is `character`.
- The existing viewer actions remain available below the shortcuts.
- A shortcut closes the viewer, selects the matching preset workflow, opens its existing canvas workspace, and adds the viewed image as a canvas reference rather than as a canvas node.
- Each preset keeps its own notes, links, prompt, and references through the existing workspace persistence model.
- If the current image cannot be read or the target workspace already has the maximum number of references, the viewer stays open and shows an error.

## Compatibility

- Reuse the current `canvas_workspaces`, `canvas_references`, and reference presentation paths.
- Do not change generation requests or server-facing data structures.
- Store the viewed item's category as transient viewer state so the menu is based on the image being viewed, not the currently selected generation category.

## Acceptance

- A character image exposes exactly the three requested workflow shortcuts in its right-click menu.
- A non-character image does not expose them.
- Each shortcut opens `character-age`, `character-outfit`, or `character-body` respectively.
- The target workspace reference list contains the viewed image, and other workspaces remain independent.
