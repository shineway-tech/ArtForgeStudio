//! Native drag payloads retain their original private binding until consumption.
use super::*;

/// Prepared only by an owned, registered worker. No counted permit is retained
/// while AppKit is waiting for the next mouse event.
pub(crate) struct PreparedNativeFileDragSource {
    persistence: PrivatePersistence,
    authority: Arc<NamespaceStorageAuthority>,
    file: NamespaceManagedFile,
    metadata: ManagedFileMetadata,
    path: PathBuf,
}

/// UI-thread-owned (AppContext contains Rc). No unsafe Send and no TLS in Drop.
pub(crate) struct CapturedNativeFileDrag {
    context: AppContext,
    source: PreparedNativeFileDragSource,
    presentation: Option<Box<dyn Fn() -> bool>>,
}

fn owned_drag_key(lease: &NamespaceLease, path: &Path) -> Result<ManagedFileKey> {
    anyhow::ensure!(path.is_absolute(), "native drag requires an owned absolute path");
    let _relative = path.strip_prefix(lease.namespace.root())?;
    // Recovery and staging are not user-export surfaces. Nested managed areas
    // must win over their parents, and ManagedFileKey rejects path traversal.
    let mut areas = vec![ManagedUserArea::Input, ManagedUserArea::Output,
        ManagedUserArea::Prompt, ManagedUserArea::Canvas, ManagedUserArea::CanvasUploads,
        ManagedUserArea::CanvasExports, ManagedUserArea::References,
        ManagedUserArea::ReferencesLibrary, ManagedUserArea::ReferencesImports,
        ManagedUserArea::Previews, ManagedUserArea::Videos,
        ManagedUserArea::ToolboxCompressionInputs, ManagedUserArea::ToolboxCompressionResults,
        ManagedUserArea::ToolboxConversionInputs, ManagedUserArea::ToolboxConversionResults,
        ManagedUserArea::ToolboxCropInputs];
    areas.sort_by_key(|area| std::cmp::Reverse(lease.namespace.path(*area).components().count()));
    areas.into_iter().find_map(|area| path.strip_prefix(lease.namespace.path(area)).ok()
        .and_then(|name| name.to_str()).and_then(|name| ManagedFileKey::new(area, name).ok()))
        .ok_or_else(|| anyhow!("native drag source is outside owned export areas"))
}

pub(super) fn prepare_native_file_drag_source(
    persistence: &PrivatePersistence, path: &Path,
) -> Result<PreparedNativeFileDragSource> {
    let (_activity, _effect) = persistence.begin_effect()?;
    let authority = persistence.storage_authority()?;
    let key = owned_drag_key(persistence.lease(), path)?;
    let file = authority.open_existing_regular(&key)?;
    let metadata = authority.inspect_regular(&file)?;
    anyhow::ensure!(metadata.link_count == 1, "native drag source is aliased");
    Ok(PreparedNativeFileDragSource { persistence: persistence.clone(), authority,
        file, metadata, path: path.to_owned() })
}

fn require_drag_binding(context: &AppContext, persistence: &PrivatePersistence) -> Result<()> {
    anyhow::ensure!(context.store.borrow().private_persistence.as_ref()
        .map(|current| current.same_binding(persistence)).unwrap_or(false), "native drag Store changed");
    anyhow::ensure!(context.active_namespace.lock().map_err(|_| anyhow!("native drag namespace unavailable"))?
        .as_ref() == Some(persistence.lease()), "native drag namespace changed");
    let scope = SessionScope { owner_user_id: persistence.lease().namespace.user_public_id().into(),
        auth_epoch: persistence.lease().auth_epoch };
    anyhow::ensure!(context.backend.as_ref().map(|backend| backend.api.session().is_scope_current(&scope))
        .unwrap_or(false), "native drag session changed");
    Ok(())
}

/// Pure binding capture on the UI thread, outside any short completion guard.
pub(super) fn bind_native_file_drag(
    context: &AppContext, source: PreparedNativeFileDragSource,
) -> Result<CapturedNativeFileDrag> {
    require_drag_binding(context, &source.persistence)?;
    Ok(CapturedNativeFileDrag { context: context.clone(), source, presentation: None })
}

impl CapturedNativeFileDrag {
    /// Required on every UI caller. An omitted predicate never admits native
    /// consumption; dropping this owned closure does not evaluate it or use TLS.
    pub(crate) fn with_presentation_check(mut self, check: impl Fn() -> bool + 'static) -> Self {
        self.presentation = Some(Box::new(check)); self
    }
    pub(crate) fn belongs_to(&self, lease: &NamespaceLease) -> bool { self.source.persistence.lease() == lease }

    /// Called at the actual OS consumption point, not at queue insertion. All
    /// filesystem/Store validation finishes before the native loop; no latch,
    /// namespace mutex, or RefCell borrow is held across OS reentrancy. Counted
    /// effect/activity permits keep retirement waiting until that loop returns.
    pub(crate) fn consume<R>(self, native: impl FnOnce(&Path) -> R) -> Result<R> {
        anyhow::ensure!(self.presentation.as_ref().is_some_and(|check|check()), "native drag presentation expired");
        let (_activity, _effect) = self.source.persistence.begin_effect()?;
        require_drag_binding(&self.context, &self.source.persistence)?;
        anyhow::ensure!(self.source.authority.inspect_regular(&self.source.file)? == self.source.metadata,
            "native drag source changed");
        require_drag_binding(&self.context, &self.source.persistence)?;
        anyhow::ensure!(self.presentation.as_ref().is_some_and(|check|check()), "native drag presentation changed");
        Ok(native(&self.source.path))
    }
}

pub(super) fn cancel_native_file_drag_for_retirement(lease: &NamespaceLease) {
    #[cfg(target_os = "macos")]
    crate::platform::discard_macos_file_drag_if(|drag| drag.belongs_to(lease));
    #[cfg(not(target_os = "macos"))]
    let _ = lease;
}
pub(super) fn dispose_pending_native_file_drag_for_shutdown() {
    #[cfg(target_os = "macos")]
    crate::platform::discard_macos_file_drag_if(|_| true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::video_image_callbacks::tests::scoped_inputs::Fixture;

    fn source(f: &Fixture) -> PathBuf {
        let key = ManagedFileKey::new(ManagedUserArea::Output, "native-drag.png").unwrap();
        let mut file = f.authority.create_new_regular(&key).unwrap();
        f.authority.write_new_regular_from(&mut file, &mut Cursor::new(b"fixture-native-source")).unwrap();
        f.authority.sync_regular(&mut file).unwrap();
        drop(file);
        f.persistence.lease().namespace.path(ManagedUserArea::Output).join("native-drag.png")
    }
    fn prepared(f: &Fixture, path: &Path) -> PreparedNativeFileDragSource {
        let persistence = f.persistence.clone();
        std::thread::scope(|workers| workers.spawn(move || prepare_native_file_drag_source(&persistence, path))
            .join().unwrap().unwrap())
    }
    #[test]
    fn core_native_drag_omitted_presentation_predicate_denies_consumption() {
        let f = Fixture::new(); let path = source(&f);
        let captured = bind_native_file_drag(&f.context, prepared(&f, &path)).unwrap();
        assert!(captured.consume(|_| panic!("unbound presentation exported")).is_err());
        f.drain();
    }
    #[test]
    fn core_native_drag_exact_store_and_held_source_are_rechecked_at_consumption() {
        let f = Fixture::new();
        let path = source(&f);
        let captured = bind_native_file_drag(&f.context, prepared(&f, &path)).unwrap().with_presentation_check(||true);
        let moved = path.with_extension("original");
        fs::rename(&path, &moved).unwrap();
        fs::write(&path, b"replacement").unwrap();
        assert!(captured.consume(|_| panic!("replacement was exported")).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        f.drain();
    }
    #[test]
    fn core_native_drag_queue_holds_no_admission_and_exact_upgrade_denies_actual_effect() {
        let f = Fixture::new();
        let path = source(&f);
        let captured = bind_native_file_drag(&f.context, prepared(&f, &path)).unwrap().with_presentation_check(||true);
        f.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version: None });
        assert!(captured.consume(|_| panic!("ordinary native effect after upgrade")).is_err());
        f.drain();
    }
    #[test]
    fn core_native_drag_old_payload_cannot_export_after_store_replacement() {
        let f = Fixture::new();
        let path = source(&f);
        let captured = bind_native_file_drag(&f.context, prepared(&f, &path)).unwrap().with_presentation_check(||true);
        let replacement = Fixture::new();
        f.context.store.borrow_mut().private_persistence = Some(replacement.persistence.clone());
        assert!(captured.consume(|_| panic!("old source exported through replacement Store")).is_err());
        f.drain(); replacement.drain();
    }
    #[test]
    fn core_native_drag_actual_effect_runs_outside_short_lock_and_retains_counted_permits() {
        let f = Fixture::new();
        let path = source(&f);
        let captured = bind_native_file_drag(&f.context, prepared(&f, &path)).unwrap().with_presentation_check(||true);
        captured.consume(|actual| {
            assert_eq!(actual, path.as_path());
            // Reenter the same short lock and Store: an accidental outer guard
            // would deadlock or RefCell-panic at this real native-effect seam.
            f.context.apply_user_completion(f.persistence.lease(), || {
                f.context.store.borrow_mut().custom_prompts.push("native-reentry".into());
            }).unwrap();
        }).unwrap();
        assert!(f.context.store.borrow().custom_prompts.contains(&"native-reentry".into()));
        f.drain();
    }
}
