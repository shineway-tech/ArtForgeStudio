use super::*;

#[derive(Default)]
struct StorageUsage {
    total: u64,
    outputs: u64,
    references: u64,
    cache: u64,
    transient: u64,
}

pub(super) fn wire_storage_callbacks(app: &AppWindow) {
    let state = app.global::<AppState>();
    {
        let weak = app.as_weak();
        state.on_refresh_storage_usage(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            refresh_storage_usage_async(&app);
        });
    }
    {
        let weak = app.as_weak();
        state.on_clear_preview_cache(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_storage_busy() {
                return;
            }
            state.set_storage_busy(true);
            state.set_storage_message(
                if state.get_language().as_str() == "en" {
                    "Clearing thumbnail cache..."
                } else {
                    "正在清理缩略图缓存..."
                }
                .into(),
            );
            clear_preview_memory_cache();
            let weak = app.as_weak();
            std::thread::spawn(move || {
                let removed = clear_preview_disk_cache_files();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let state = app.global::<AppState>();
                    state.set_storage_busy(false);
                    state.set_storage_message(
                        if state.get_language().as_str() == "en" {
                            format!("Freed {} of thumbnail cache.", format_storage_bytes(removed))
                        } else {
                            format!("已清理 {} 缩略图缓存。", format_storage_bytes(removed))
                        }
                        .into(),
                    );
                    refresh_storage_usage_async(&app);
                });
            });
        });
    }
}

pub(super) fn refresh_storage_usage_async(app: &AppWindow) {
    let state = app.global::<AppState>();
    if state.get_storage_busy() {
        return;
    }
    state.set_storage_busy(true);
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let usage = collect_storage_usage();
        let _ = weak.upgrade_in_event_loop(move |app| {
            let state = app.global::<AppState>();
            state.set_storage_busy(false);
            state.set_storage_total_size(format_storage_bytes(usage.total).into());
            state.set_storage_output_size(format_storage_bytes(usage.outputs).into());
            state.set_storage_reference_size(format_storage_bytes(usage.references).into());
            state.set_storage_cache_size(format_storage_bytes(usage.cache).into());
            state.set_storage_transient_size(format_storage_bytes(usage.transient).into());
        });
    });
}

fn collect_storage_usage() -> StorageUsage {
    let data = app_data_dir();
    let Ok(root_metadata) = fs::symlink_metadata(&data) else {
        return StorageUsage::default();
    };
    if !root_metadata.file_type().is_dir() || root_metadata.file_type().is_symlink() {
        return StorageUsage::default();
    }
    let output_root = data.join("out");
    let image_edit_input_root = output_root.join("image-edit-inputs");
    let upscale_input_root = output_root.join("upscale-references");
    let reference_root = data.join("references");
    let canvas_root = data.join("canvas");
    let cache_root = data.join("cache");
    let toolbox_root = data.join("toolbox");
    let delivery_root = data.join("delivery-staging");
    let mut usage = StorageUsage::default();
    let mut pending = vec![data];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                let size = metadata.len();
                usage.total = usage.total.saturating_add(size);
                if path.starts_with(&image_edit_input_root)
                    || path.starts_with(&upscale_input_root)
                {
                    usage.transient = usage.transient.saturating_add(size);
                } else if path.starts_with(&output_root) {
                    usage.outputs = usage.outputs.saturating_add(size);
                } else if path.starts_with(&reference_root) || path.starts_with(&canvas_root) {
                    usage.references = usage.references.saturating_add(size);
                } else if path.starts_with(&cache_root) {
                    usage.cache = usage.cache.saturating_add(size);
                } else if path.starts_with(&toolbox_root) || path.starts_with(&delivery_root) {
                    usage.transient = usage.transient.saturating_add(size);
                }
            }
        }
    }
    usage
}

fn format_storage_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_size_format_is_compact() {
        assert_eq!(format_storage_bytes(0), "0 B");
        assert_eq!(format_storage_bytes(1024), "1.0 KB");
        assert_eq!(format_storage_bytes(1024 * 1024), "1.0 MB");
    }
}
