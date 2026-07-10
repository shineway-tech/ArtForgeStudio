# ArtAIT Project Structure And Function Map

## Overview

The current ArtAIT project is a Python/PySide6 desktop application for AI image,
character sprite and animation/video workflows. It has two user-facing entry
points:

- `python main.py generate` or `artait_generate.py`: image and character tool.
- `python main.py animation` or `artait_animation.py`: animation short-film tool.

Both entry points share a large common GUI and workflow layer in
`gui_common.py`, use `config.json` for providers and behavior, and call provider
plugins under `providers/`.

## Top-Level Files

- `main.py`: command router for `generate` and `animation`.
- `artait_generate.py`: Generate-mode window classes and startup handling.
- `artait_animation.py`: Animation-mode UI, script tools, storyboard helpers,
  style dialogs and video workflow.
- `gui_common.py`: shared PySide6 controls, settings dialog, provider instance
  UI, background worker threads, gallery, prompt tools, themes and main-window
  base classes.
- `generate_actions.py`: core image/action generation orchestration. It resolves
  actions, builds prompts, invokes providers, polls tasks, supports retries and
  cancellation.
- `config.json`: active runtime configuration. The checked-in file currently
  includes real-looking API keys and must be treated as local secret material.
- `requirements.txt`: Python dependencies: `requests`, `Pillow`, `PySide6`.
- `ArtAIT-Generate.bat`, `ArtAIT-Animation.bat`, `ArtAIT_min.bat`: Windows
  PyInstaller build scripts.
- `merge_to_spritesheet.py`: merges frame folders into sprite sheets.
- `unmult.py`: removes black premultiplication from transparent image pixels.
- `refactor_svg.py`, `check_script.py`: local maintenance helpers.

## Core Package

- `core/config.py`: loads and migrates config, resolves active provider
  endpoints, tests API connectivity and parses model responses.
- `core/uploads.py`: upload cache, image URL validation and upload helpers.
- `core/video.py`: active video endpoint accessors, MPV playback and provider
  video task wrappers.
- `core/task_tracker.py`: saves and reloads the most recent async generation
  task.

## Provider Package

`providers/base.py` defines the plugin protocol:

- `ProviderMeta`: provider id, display name, capabilities and default models.
- `GenerationCtx`: injected helpers used by generation providers.
- Capability ids: `generate`, `generate_character`, `generate_video`,
  `analyze`, `test_connection`, `quota`, `upload_binary`.
- Active instance lookup helpers for generation, analysis and video.

`providers/__init__.py` discovers provider modules that expose `PROVIDER_META`.
Important provider modules:

- `toapis.py`: ToAPI image generation, character generation, analysis and quota.
- `ikuncode.py`: Gemini-style and image-generation endpoints, task polling.
- `wavespeed.py`: WaveSpeed image generation, edit and upload support.
- `openai_generic.py`: OpenAI-compatible image generation/edit/analyze adapter.
- `gemini_generic.py`: Gemini-compatible image generation/analyze adapter.
- `deepseek_generic.py`, `anthropic_generic.py`: analysis adapters.
- `volcengine_seedance.py`: video task provider.
- `video_generic.py`: generic video settings/test adapter.
- `photoroom.py`: background removal helper.
- `prompt_optimizer.py`: local prompt optimizer service client.
- `_settings.py`: reusable PySide6 settings widget builder.
- `common.py`: shared request logging, result extraction, image download and
  base64 handling.

## Prompting Package

- `prompting/action_prompt.py`: action-specific prompt construction, grid
  enforcement, reference/shared prompt expansion and generation size resolution.
- `prompting/appearance.py`: appearance profile extraction and prompt building.
- `prompting/constants.py`: shared prompt constants.

## Prompt And Reference Directories

- `prompt/reference_prompt/`: action templates such as `idle`, `run`, `attack`.
- `prompt/scene_prompt/`: scene style templates.
- `prompt/ui_prompt/`: UI prompt examples.
- `prompt/create_character_prompt/`: character creation templates.
- `reference_prompt/`: older or alternate reference templates.
- `reference_action/`: expected reference images for actions.
- `out/`: generated output and local runtime artifacts.

## Runtime Data Flow

1. GUI loads `config.json` through `core.config.load_config`.
2. Provider instances define active generation, analysis and video endpoints.
3. The user selects or drops reference images and chooses actions.
4. `generate_actions.py` discovers configured actions and builds prompts through
   `prompting.action_prompt`.
5. Provider plugin functions submit image or edit requests.
6. Long-running providers are polled through provider-specific task functions.
7. Results are downloaded or decoded and saved under `out/`.
8. GUI worker threads report logs and refresh gallery thumbnails.

## Configuration Shape

The modern config model uses:

- `provider_instances`: map of named instances.
- `current_generation_instance`
- `current_analysis_instance`
- `current_video_instance`

Each instance has:

- `provider`: provider type id.
- `name`: user-facing name.
- `scope`: `generation`, `analysis`, `video` or `both`.
- `show_in_main_ui`: visibility flag.
- `config`: provider-specific API URL, key, model and options.

Legacy keys such as `current_provider` and `providers` are still migrated and
used as fallback.

## Rust Reconstruction Boundaries

The Rust project mirrors the Python architecture with stricter separation:

- `artait-core`: config, providers, prompts, actions, files, video and future
  generation orchestration.
- `artait-gui`: `eframe/egui` desktop shell and user interaction.

The first Rust stage intentionally provides a verified skeleton and migration
map rather than claiming full parity with every PySide6 widget and provider
request. Full parity requires porting provider HTTP payloads, background
workers, gallery behavior, upload cache and video polling in later milestones.
