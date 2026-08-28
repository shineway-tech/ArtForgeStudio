use super::*;
use crate::directory_migration::remap_path;
use std::sync::OnceLock;

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct DirectoryLocations {
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub prompt: String,
    // Persisted with the locations in one SQLite transaction. This also repairs
    // older metadata, queued snapshots and recovery records after a crash.
    #[serde(default)]
    pub relocations: Vec<DirectoryRelocation>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct DirectoryRelocation {
    pub source: PathBuf,
    pub destination: PathBuf,
}

fn locations_slot() -> &'static Mutex<DirectoryLocations> {
    static LOCATIONS: OnceLock<Mutex<DirectoryLocations>> = OnceLock::new();
    LOCATIONS.get_or_init(|| Mutex::new(DirectoryLocations::default()))
}

pub(super) fn directory_locations() -> DirectoryLocations {
    locations_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub(super) fn set_directory_locations(locations: DirectoryLocations) {
    *locations_slot().lock().unwrap_or_else(|e| e.into_inner()) = locations;
}

impl DirectoryLocations {
    pub fn directory(&self, kind: &str) -> Option<PathBuf> {
        let (configured, default) = match kind {
            "input" => (&self.input, "input"),
            "output" => (&self.output, "out"),
            "prompt" => (&self.prompt, "prompt"),
            _ => return None,
        };
        Some(if configured.is_empty() {
            app_data_dir().join(default)
        } else {
            PathBuf::from(configured)
        })
    }

    pub fn migrated(&self, kind: &str, source: PathBuf, destination: PathBuf) -> Result<Self> {
        let mut next = self.clone();
        let value = display_directory_path(&destination);
        match kind {
            "input" => next.input = value,
            "output" => next.output = value,
            "prompt" => next.prompt = value,
            _ => anyhow::bail!("未知目录类型"),
        }
        next.relocations.push(DirectoryRelocation {
            source,
            destination,
        });
        // Reusing an old root for a different directory could otherwise make a
        // newly saved path look like a historical path. Accept moves back only
        // when every active root (including historical boundaries inside it)
        // remains a fixed point of the relocation map.
        for kind in ["input", "output", "prompt"] {
            let root = next.directory(kind).unwrap();
            let mut boundaries = vec![root.clone()];
            boundaries.extend(
                next.relocations
                    .iter()
                    .filter(|entry| {
                        remap_path(&entry.source.display().to_string(), &root, &root).is_some()
                    })
                    .map(|entry| entry.source.clone()),
            );
            for boundary in boundaries {
                let mut mapped = display_directory_path(&boundary);
                next.remap(&mut mapped);
                if !crate::directory_migration::same_path(&boundary, Path::new(&mapped)) {
                    anyhow::bail!("该目标目录会与历史迁移路径混淆，请选择新的独立文件夹");
                }
            }
        }
        Ok(next)
    }

    pub fn remap(&self, value: &mut String) {
        for relocation in &self.relocations {
            if let Some(next) = remap_path(value, &relocation.source, &relocation.destination) {
                *value = display_directory_path(Path::new(&next));
            }
        }
    }

    pub fn remap_paths(&self, values: &mut [String]) {
        for value in values {
            self.remap(value);
        }
    }

    pub fn remap_profile(&self, profile: &mut CustomPromptProfile) {
        self.remap(&mut profile.reference_path);
        self.remap_paths(&mut profile.reference_paths);
    }

    pub fn remap_local_store(&self, data: &mut LocalStoreData) {
        for asset in data.assets.iter_mut().chain(&mut data.generations) {
            self.remap(&mut asset.source_path);
            self.remap_paths(&mut asset.reference_paths);
        }
        for note in &mut data.canvas_notes {
            self.remap(&mut note.image_path);
        }
        for profile in data.custom_prompt_profiles.values_mut() {
            self.remap_profile(profile);
        }
    }

    pub fn remap_store(&self, store: &mut Store) {
        for asset in store
            .assets
            .iter_mut()
            .chain(&mut store.generations)
            .chain(&mut store.inspiration)
        {
            self.remap(&mut asset.source_path);
            self.remap_paths(&mut asset.reference_paths);
        }
        for note in &mut store.canvas_notes {
            self.remap(&mut note.image_path);
        }
        for profile in store.custom_prompt_profiles.values_mut() {
            self.remap_profile(profile);
        }
        for category in ["character", "scene", "ui", "effect"] {
            for reference in references_for_category_mut(&mut store.references, category) {
                self.remap(&mut reference.source_path);
            }
        }
    }
}

pub(super) fn display_directory_path(path: &Path) -> String {
    let text = path.display().to_string();
    #[cfg(windows)]
    {
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        return text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
    }
    #[cfg(not(windows))]
    {
        text
    }
}

pub(super) fn configured_output_directory() -> PathBuf {
    directory_locations()
        .directory("output")
        .expect("output directory kind")
}

pub(super) fn sync_directory_locations(app: &AppWindow) {
    let config = directory_locations();
    let state = app.global::<AppState>();
    state.set_input_dir(display_directory_path(&config.directory("input").unwrap()).into());
    state.set_output_dir(display_directory_path(&config.directory("output").unwrap()).into());
    state.set_prompt_dir(display_directory_path(&config.directory("prompt").unwrap()).into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_rebases_only_file_fields_and_preserves_prompt_text() {
        let root = std::env::temp_dir().join("elunvi-location-test");
        let source = root.join("out");
        let target = root.join("new");
        let old = source.join("image.png").display().to_string();
        let expected = target.join("image.png").display().to_string();
        let unrelated = root.join("outside/image.png").display().to_string();
        let locations = DirectoryLocations::default()
            .migrated("output", source, target)
            .unwrap();
        let mut data = LocalStoreData::default();
        data.prompt_drafts.scene = old.clone();
        data.canvas_notes.push(CanvasNoteData {
            image_path: old.clone(),
            content: old.clone(),
            ..Default::default()
        });
        data.custom_prompt_profiles.insert(
            "preset".into(),
            CustomPromptProfile {
                reference_path: old.clone(),
                reference_paths: vec![old.clone(), unrelated.clone()],
                ..Default::default()
            },
        );
        locations.remap_local_store(&mut data);
        assert_eq!(data.canvas_notes[0].image_path, expected);
        assert_eq!(data.canvas_notes[0].content, old);
        assert_eq!(data.prompt_drafts.scene, old);
        assert_eq!(
            data.custom_prompt_profiles["preset"].reference_paths,
            vec![expected, unrelated]
        );
    }

    #[test]
    fn locations_round_trip_and_chain_across_restarts() {
        let root = std::env::temp_dir().join("elunvi-location-test");
        let config = DirectoryLocations::default()
            .migrated("input", root.join("a"), root.join("b"))
            .unwrap()
            .migrated("input", root.join("b"), root.join("c"))
            .unwrap();
        let restored: DirectoryLocations =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        let mut old = root.join("a/sub/file.png").display().to_string();
        restored.remap(&mut old);
        assert_eq!(PathBuf::from(old), root.join("c/sub/file.png"));
        assert_eq!(restored.directory("input"), Some(root.join("c")));
    }

    #[test]
    fn previous_location_can_be_restored_but_not_reused_by_another_kind() {
        let root = std::env::temp_dir().join("elunvi-location-test");
        let config = DirectoryLocations::default()
            .migrated("input", root.join("a"), root.join("b"))
            .unwrap();
        assert!(config
            .migrated("output", root.join("c"), root.join("a"))
            .is_err());
        let restored = config
            .migrated("input", root.join("b"), root.join("a"))
            .unwrap();
        let mut path = root.join("a/image.png").display().to_string();
        restored.remap(&mut path);
        assert_eq!(PathBuf::from(path), root.join("a/image.png"));
    }
}
