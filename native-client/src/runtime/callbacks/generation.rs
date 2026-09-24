use super::*;

#[cfg(test)]
mod core_generation_metadata_tests {
    use super::*;

    struct Fixture(video_image_callbacks::tests::scoped_inputs::Fixture);
    impl std::ops::Deref for Fixture {
        type Target = video_image_callbacks::tests::scoped_inputs::Fixture;
        fn deref(&self) -> &Self::Target { &self.0 }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let delivery = drain_delivery_commit_workers_for_lease_for_test(self.persistence.lease());
            let previews = drain_activation_preview_workers_for_lease_for_test(self.persistence.lease());
            let retired = self.context.user_activity.begin_quiesce(self.persistence.lease())
                .map(|guard| guard.retire());
            if !std::thread::panicking() { delivery.unwrap(); previews.unwrap(); retired.unwrap(); }
        }
    }

    fn failed_card() -> AssetData {
        AssetData {
            id: "metadata-card".into(), conversation_id: "original-conversation".into(),
            title: "Retained title".into(), category: "scene".into(), kind: "game".into(),
            time: "2026-09-08 00:00".into(), prompt: "Retained prompt".into(),
            ratio: "1:1".into(), quality: "1K".into(), model: "retained-model".into(),
            origin: "backend".into(), width: 0, height: 0, source_path: "failed".into(),
            reference_paths: vec![], cutout_done: false, remove_black_done: false,
            upscale_done: false, is_new: true, delivery_recoverable: false,
            delivery_downloading: false,
        }
    }

    fn reset_private_metadata(app: &AppWindow, context: &AppContext) {
        let state = app.global::<AppState>();
        state.set_page("generation".into()); state.set_asset_type("scene".into());
        state.set_logged_in(true); state.set_session_state("online".into());
        state.set_generation_status("original status".into());
        state.set_current_conversation_id("original-conversation".into());
        state.set_quote_title("original quote".into()); state.set_quote_prompt("original quote prompt".into());
        state.set_quote_ratio("original ratio".into()); state.set_quote_quality("original quality".into());
        state.set_quote_width(17); state.set_quote_height(23);
        state.set_prompt_history_open(true);
        state.set_prompt_history(ModelRc::new(VecModel::from(vec![SharedString::from("Retained prompt")])));
        state.set_prompt_history_previews(ModelRc::new(VecModel::from(vec![SharedString::from("Retained prompt")])));
        let mut store = context.store.borrow_mut();
        store.generations = vec![failed_card()];
        store.dismissed_prompt_history.clear();
    }

    // No billable/start/stop/native callback is invoked. Every preview source is
    // the real failed-card sentinel; actual gallery scheduling skips it.
    fn ordinary_mutations(app: &AppWindow, context: &AppContext) -> Vec<&'static str> {
        let state = app.global::<AppState>();
        let mut changed = Vec::new();
        reset_private_metadata(app, context);
        state.invoke_remove_prompt_history("Retained prompt".into());
        if !context.store.borrow().dismissed_prompt_history.is_empty()
            || !state.get_prompt_history_open()
            || state.get_prompt_history().row_count() != 1
            || state.get_generation_status() != "original status" {
            changed.push("remove-history");
        }
        reset_private_metadata(app, context);
        state.invoke_clear_prompt_history();
        if !context.store.borrow().dismissed_prompt_history.is_empty()
            || !state.get_prompt_history_open()
            || state.get_prompt_history().row_count() != 1
            || state.get_generation_status() != "original status" {
            changed.push("clear-history");
        }
        reset_private_metadata(app, context);
        state.invoke_open_conversation("replacement-conversation".into());
        if state.get_current_conversation_id() != "original-conversation" {
            changed.push("open-conversation");
        }
        reset_private_metadata(app, context);
        state.invoke_dismiss_new_generation("metadata-card".into());
        if !context.store.borrow().generations[0].is_new {
            changed.push("dismiss-new");
        }
        reset_private_metadata(app, context);
        state.invoke_quote_generation("metadata-card".into());
        if state.get_quote_title() != "original quote"
            || state.get_quote_prompt() != "original quote prompt"
            || state.get_quote_ratio() != "original ratio"
            || state.get_quote_quality() != "original quality"
            || state.get_quote_width() != 17 || state.get_quote_height() != 23 {
            changed.push("quote-generation");
        }
        changed
    }

    fn active_metadata_fixture() -> (Fixture,AppWindow) {
        let f = Fixture(video_image_callbacks::tests::scoped_inputs::Fixture::new());
        let transition = f.context.namespace_operations.try_begin_transition().unwrap();
        let recovery = transition.begin_prepublication_recovery(f.persistence.lease()).unwrap();
        recovery.verify_no_unsupported_imports(&f.authority).unwrap();
        let recovered = recovery.finish().unwrap();
        transition.prepare_publication(f.persistence.lease(),recovered).unwrap().publish();
        let app = AppWindow::new().unwrap();
        wire_generation_callbacks(&app,f.context.clone());
        reset_private_metadata(&app,&f.context);
        let mut second = failed_card();
        second.id = "second-card".into(); second.conversation_id = "second-conversation".into();
        second.prompt = "Second prompt".into();
        {
            let mut store = f.context.store.borrow_mut();
            store.generations.push(second); store.assets = store.generations.clone();
            store.custom_prompts.push("fixture save trigger".into());
        }
        app.global::<AppState>().set_conversations(ModelRc::new(VecModel::from(vec![
            ConversationItem { id:"original-conversation".into(),title:"Original".into(),image:Image::default(),loading:true },
            ConversationItem { id:"second-conversation".into(),title:"Second".into(),image:Image::default(),loading:true },
        ])));
        push_prompt_history(&app,&f.context.store.borrow());
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        (f,app)
    }
    fn durable_metadata(f:&Fixture) -> LocalStoreData {
        f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap()
    }
    fn pump_metadata(predicate:impl FnMut()->bool) {
        video_image_callbacks::tests::scoped_inputs::pump(predicate);
    }
    fn pump_metadata_ticks() {
        for _ in 0..12 {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    // Catches disabling ordinary actions, losing quote fields or saving badges,
    // and history UI success without a real same-namespace SQLite write.
    #[test]
    fn core_generation_metadata_normal_actions_preserve_quote_badge_and_history() {
        i_slint_backend_testing::init_no_event_loop();
        let(f,app)=active_metadata_fixture(); let state=app.global::<AppState>();
        let before=serde_json::to_value(durable_metadata(&f)).unwrap();
        state.invoke_quote_generation("metadata-card".into());
        assert_eq!(state.get_quote_title(),"Retained title");
        assert_eq!(state.get_quote_prompt(),"Retained prompt");
        assert_eq!(state.get_quote_ratio(),"1:1"); assert_eq!(state.get_quote_quality(),"1K");
        assert_eq!((state.get_quote_width(),state.get_quote_height()),(0,0));
        state.invoke_open_conversation("second-conversation".into());
        assert_eq!(state.get_current_conversation_id(),"second-conversation");
        state.invoke_dismiss_new_generation("metadata-card".into());
        assert!(!f.context.store.borrow().generations[0].is_new);
        assert!(!f.context.store.borrow().assets[0].is_new);
        assert_eq!(serde_json::to_value(durable_metadata(&f)).unwrap(),before);
        let serialized=serde_json::to_value(local_store_data(&app,&f.context.store.borrow())).unwrap();
        assert!(serialized["assets"][0].get("is_new").is_none());
        assert!(serialized["generations"][0].get("is_new").is_none());
        state.invoke_remove_prompt_history("Retained prompt".into());
        pump_metadata(||state.get_generation_status()=="提示词历史已保存");
        assert!(durable_metadata(&f).dismissed_prompt_history.contains("Retained prompt"));
        assert_eq!(state.get_prompt_history().iter().map(|row|row.to_string()).collect::<Vec<_>>(),["Second prompt"]);
        state.set_generation_status("await clear".into());state.invoke_clear_prompt_history();
        pump_metadata(||state.get_generation_status()=="提示词历史已保存");
        let durable=durable_metadata(&f);
        assert!(durable.dismissed_prompt_history.contains("Second prompt"));
        assert_eq!(durable.generations.len(),2);assert_eq!(state.get_prompt_history().row_count(),0);
        assert!(!state.get_prompt_history_open());
    }

    // A same remove after a failed transaction must save CURRENT Store, even
    // though the set insertion is already staged and returns false.
    #[test]
    fn core_generation_history_failed_remove_retry_saves_current_store_and_later_editor() {
        i_slint_backend_testing::init_no_event_loop();
        let(f,app)=active_metadata_fixture();let state=app.global::<AppState>();
        f.writer.reject_custom_prompt_inserts_for_test(true);
        state.invoke_remove_prompt_history("Retained prompt".into());
        pump_metadata(||state.get_generation_status().contains("保存失败"));
        assert!(f.context.store.borrow().dismissed_prompt_history.contains("Retained prompt"));
        assert!(durable_metadata(&f).dismissed_prompt_history.is_empty());
        state.set_prompt("later editor text".into());
        f.context.store.borrow_mut().custom_prompts.push("later private value".into());
        f.writer.reject_custom_prompt_inserts_for_test(false);
        state.invoke_remove_prompt_history("Retained prompt".into());
        pump_metadata(||state.get_generation_status()=="提示词历史已保存");
        let durable=durable_metadata(&f);
        assert!(durable.dismissed_prompt_history.contains("Retained prompt"));
        assert!(durable.custom_prompts.contains(&"later private value".into()));
        assert_eq!(state.get_prompt(),"later editor text");assert_eq!(durable.generations.len(),2);
    }

    // Retrying Clear must retain its original set; a different subsequent Remove
    // must still apply, rather than be swallowed by a family-wide retry bit.
    #[test]
    fn core_generation_history_clear_retry_preserves_prompt_added_after_failed_clear() {
        i_slint_backend_testing::init_no_event_loop();
        let(f,app)=active_metadata_fixture();let state=app.global::<AppState>();
        f.writer.reject_custom_prompt_inserts_for_test(true);state.invoke_clear_prompt_history();
        pump_metadata(||state.get_generation_status().contains("保存失败"));
        assert!(durable_metadata(&f).dismissed_prompt_history.is_empty());
        let mut later=failed_card();later.id="later-card".into();later.prompt="later history".into();
        f.context.store.borrow_mut().generations.push(later);
        f.writer.reject_custom_prompt_inserts_for_test(false);state.invoke_clear_prompt_history();
        pump_metadata(||state.get_generation_status()=="提示词历史已保存");
        let durable=durable_metadata(&f);
        assert_eq!(durable.dismissed_prompt_history.len(),2);
        assert!(!durable.dismissed_prompt_history.contains("later history"));
        assert_eq!(state.get_prompt_history().iter().map(|row|row.to_string()).collect::<Vec<_>>(),["later history"]);
        // This DIFFERENT action is made while a previous clear save has failed.
        f.writer.reject_custom_prompt_inserts_for_test(true);state.invoke_clear_prompt_history();
        pump_metadata(||state.get_generation_status().contains("保存失败"));
        let mut newest=failed_card();newest.id="newest-card".into();newest.prompt="newest history".into();
        f.context.store.borrow_mut().generations.push(newest);
        f.writer.reject_custom_prompt_inserts_for_test(false);
        state.invoke_remove_prompt_history("newest history".into());
        pump_metadata(||state.get_generation_status()=="提示词历史已保存");
        let durable=durable_metadata(&f);
        assert!(durable.dismissed_prompt_history.contains("later history"));
        assert!(durable.dismissed_prompt_history.contains("newest history"));
        assert_eq!(durable.generations.len(),4);
    }

    struct HeldMetadataAck(Option<mpsc::Sender<()>>);
    impl HeldMetadataAck {
        fn release(&mut self){if let Some(sender)=self.0.take(){let _=sender.send(());}}
    }
    impl Drop for HeldMetadataAck {fn drop(&mut self){self.release();}}
    // The actual waiter has already sent its failed receipt but has not exited:
    // neither premature success nor stale errors may reach a replacement binding.
    #[test]
    fn core_generation_history_sent_ack_cannot_publish_after_binding_or_page_change() {
        i_slint_backend_testing::init_no_event_loop();
        // The two same-page cases use actual failed and successful SQLite acks.
        // Only their shared status changes; original persistence stays current.
        for case in 0..4 {
            let(f,app)=active_metadata_fixture();let state=app.global::<AppState>();
            let(arrived_tx,arrived)=mpsc::channel();let(release_tx,release)=mpsc::channel();
            let mut held=HeldMetadataAck(Some(release_tx));
            set_delivery_preparation_after_send_for_test(move||{
                arrived_tx.send(()).unwrap();release.recv_timeout(Duration::from_secs(6)).unwrap();
            });
            if case != 3 { f.writer.reject_custom_prompt_inserts_for_test(true); }
            state.invoke_remove_prompt_history("Retained prompt".into());
            let mut sent=false;
            pump_metadata(||{sent=sent || arrived.try_recv().is_ok();sent});pump_metadata_ticks();
            assert_eq!(state.get_generation_status(),"original status","sent receipt was consumed before real join");
            if case == 0 {
                f.context.store.borrow_mut().private_persistence=Some(PrivatePersistence::for_test(
                    (*f.writer).clone(),f.persistence.lease().clone(),f.context.user_activity.clone(),f.persistence.upgrade_latch()));
            } else if case == 1 {state.set_page("assets".into());state.set_asset_type("other".into());}
            state.set_generation_status("replacement status".into());state.set_quote_title("replacement quote".into());
            state.set_prompt("replacement editor".into());held.release();
            drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();pump_metadata_ticks();
            assert_eq!(state.get_generation_status(),"replacement status");
            assert_eq!(state.get_quote_title(),"replacement quote");assert_eq!(state.get_prompt(),"replacement editor");
            let durable=durable_metadata(&f);
            if case == 3 {
                assert!(durable.dismissed_prompt_history.contains("Retained prompt"));
                // Successful debt must settle even when a later status owns UI:
                // a same removal is now a no-op, not a hidden failed re-save.
                f.writer.reject_custom_prompt_inserts_for_test(true);
                state.invoke_remove_prompt_history("Retained prompt".into());
                drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();pump_metadata_ticks();
                assert_eq!(state.get_generation_status(),"replacement status");
            } else {assert!(durable.dismissed_prompt_history.is_empty());}
            f.writer.reject_custom_prompt_inserts_for_test(false);
        }
    }

    #[test]
    fn core_generation_private_metadata_callbacks_deny_missing_binding() {
        i_slint_backend_testing::init_no_event_loop();
        let context = AppContext::default(); let app = AppWindow::new().unwrap();
        wire_generation_callbacks(&app, context.clone());
        let changes = ordinary_mutations(&app, &context);
        assert!(changes.is_empty(), "missing binding changed private metadata: {changes:?}");
    }

    #[test]
    fn core_generation_private_metadata_callbacks_deny_exact_upgrade() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = Fixture(video_image_callbacks::tests::scoped_inputs::Fixture::new());
        let transition = fixture.context.namespace_operations.try_begin_transition().unwrap();
        let recovery = transition.begin_prepublication_recovery(fixture.persistence.lease()).unwrap();
        recovery.verify_no_unsupported_imports(&fixture.authority).unwrap();
        let recovered = recovery.finish().unwrap();
        transition.prepare_publication(fixture.persistence.lease(), recovered).unwrap().publish();
        let app = AppWindow::new().unwrap();
        wire_generation_callbacks(&app, fixture.context.clone());
        reset_private_metadata(&app, &fixture.context);
        // Positive control proves this fixture can reach the real ordinary action.
        app.global::<AppState>().invoke_quote_generation("metadata-card".into());
        assert_eq!(app.global::<AppState>().get_quote_title(), "Retained title");
        reset_private_metadata(&app, &fixture.context);
        fixture.persistence.save_store(local_store_data(&app, &fixture.context.store.borrow())).unwrap();
        let before = serde_json::to_value(fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).unwrap().unwrap()).unwrap();
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        let changes = ordinary_mutations(&app, &fixture.context);
        let durable = fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).unwrap().unwrap();
        assert!(durable.dismissed_prompt_history.is_empty());
        assert_eq!(durable.generations.len(), 1);
        assert_eq!(serde_json::to_value(&durable).unwrap(), before, "denied callbacks changed actual durable metadata");
        assert!(changes.is_empty(), "exact 426 changed private metadata: {changes:?}");
    }
}

#[derive(Clone)]
struct GenerationMetadataCapture {
    context: AppContext,
    persistence: PrivatePersistence,
    session: SessionScope,
    page: String,
    category: String,
}
impl GenerationMetadataCapture {
    fn capture(app: &AppWindow, context: &AppContext) -> Option<Self> {
        let persistence = context.store.borrow().private_persistence.clone()?;
        let owner = context.current_user_id.lock().ok()?.clone()?;
        let session = context.backend.as_ref()?.api.session().scope_for_user(&owner)?;
        if !persistence.is_current() || persistence.lease().namespace.user_public_id() != session.owner_user_id
            || persistence.lease().auth_epoch != session.auth_epoch { return None; }
        let captured = Self { context: context.clone(), persistence, session,
            page: app.global::<AppState>().get_page().to_string(),
            category: app.global::<AppState>().get_asset_type().to_string() };
        captured.matches(app).then_some(captured)
    }
    // Pure metadata checks only; safe under the already admitted completion.
    fn matches(&self, app: &AppWindow) -> bool {
        self.context.store.borrow().private_persistence.as_ref()
            .is_some_and(|bound| bound.same_binding_metadata(&self.persistence))
            && self.context.active_namespace.lock().unwrap_or_else(|e|e.into_inner()).as_ref() == Some(self.persistence.lease())
            && self.context.current_user_id.lock().unwrap_or_else(|e|e.into_inner()).as_deref() == Some(self.session.owner_user_id.as_str())
            && self.context.backend.as_ref().is_some_and(|backend|backend.api.session().is_scope_current(&self.session))
            && app.global::<AppState>().get_page() == self.page
            && app.global::<AppState>().get_asset_type() == self.category
    }
    fn apply<R>(&self, app: &AppWindow, apply: impl FnOnce() -> R) -> Option<R> {
        if !self.persistence.is_current() { return None; }
        self.context.apply_user_completion(self.persistence.lease(), || {
            self.matches(app).then(apply)
        }).ok().flatten()
    }
}

#[derive(Clone, PartialEq, Eq)]
enum GenerationHistoryAction { Remove(String), Clear }
struct GenerationHistoryDebt {
    capture: GenerationMetadataCapture,
    action: GenerationHistoryAction,
    prompts: Vec<String>,
    revision: u64,
    status: String,
}
#[derive(Default)]
struct GenerationHistoryWrites {
    revision: u64,
    debt: Option<GenerationHistoryDebt>,
}

fn generation_history_save_failed(app: &AppWindow, capture: &GenerationMetadataCapture,
    writes: &Rc<RefCell<GenerationHistoryWrites>>, revision: u64) {
    let _ = capture.apply(app, || {
        if writes.borrow().debt.as_ref().is_some_and(|debt|debt.revision == revision
            && app.global::<AppState>().get_generation_status() == debt.status) {
            app.global::<AppState>().set_generation_status("提示词历史保存失败；更改已保留，请重试原操作".into());
        }
    });
}

fn mutate_generation_history(app: &AppWindow, context: &AppContext,
    writes: Rc<RefCell<GenerationHistoryWrites>>, action: GenerationHistoryAction) {
    let Some(capture) = GenerationMetadataCapture::capture(app, context) else { return; };
    let Ok(_activity) = capture.persistence.begin_activity() else { return; };
    let retry_prompts = writes.borrow().debt.as_ref().filter(|debt|
        debt.action == action && debt.capture.persistence.same_binding_metadata(&capture.persistence)
            && debt.capture.page == capture.page && debt.capture.category == capture.category)
        .map(|debt|debt.prompts.clone());
    let prompts = retry_prompts.clone().unwrap_or_else(||match &action {
        GenerationHistoryAction::Remove(prompt) => vec![prompt.trim().to_string()],
        GenerationHistoryAction::Clear => context.store.borrow().generations.iter()
            .map(|item|item.prompt.trim().to_string()).filter(|prompt|!prompt.is_empty()).collect(),
    });
    if retry_prompts.is_none() && !prompts.iter().any(|prompt|
        !prompt.is_empty() && !context.store.borrow().dismissed_prompt_history.contains(prompt)) {
        let _ = capture.apply(app,||push_prompt_history(app,&context.store.borrow()));
        return;
    }
    let Some(revision) = writes.borrow().revision.checked_add(1) else { return; };
    let prepared = match capture.persistence.prepare_ordered_save() {
        Ok(prepared) => prepared,
        Err(_) => { let _ = capture.apply(app, || {
            app.global::<AppState>().set_generation_status("提示词历史暂时无法保存，请重试".into());
        }); return; }
    };
    let mut prepared = Some(prepared);
    let enqueued = capture.apply(app, || {
        let mut store = context.store.borrow_mut();
        for prompt in &prompts { dismiss_prompt_history_entry(&mut store, prompt); }
        push_prompt_history(app, &store);
        let mut pending = writes.borrow_mut();
        pending.revision = revision;
        pending.debt = Some(GenerationHistoryDebt { capture: capture.clone(), action, prompts, revision,
            status: app.global::<AppState>().get_generation_status().to_string() });
        // Assign sequence and enqueue CURRENT Store now; do not send an old snapshot
        // to an arbitrary worker to enqueue later. Return guarded errors intact.
        prepared.take().unwrap().enqueue(local_store_data(app, &store))
    });
    drop(prepared);
    let receiver = match enqueued {
        Some(Ok(receiver)) => receiver,
        Some(Err(error)) => { drop(error); generation_history_save_failed(app,&capture,&writes,revision); return; }
        None => return,
    };
    let worker = spawn_delivery_preparation(&capture.persistence, move|_,_,_| {
        receiver.recv().map_err(|_|anyhow!("history Store acknowledgment disconnected"))?
            .map_err(anyhow::Error::from)?;
        Ok(())
    });
    match worker {
        Ok((cancel, receiver)) => poll_generation_history_save(app.as_weak(),capture,writes,revision,cancel,receiver),
        Err(_) => generation_history_save_failed(app,&capture,&writes,revision),
    }
}

fn poll_generation_history_save(weak: Weak<AppWindow>, capture: GenerationMetadataCapture,
    writes: Rc<RefCell<GenerationHistoryWrites>>, revision: u64,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    receiver: mpsc::Receiver<std::result::Result<(),DeliveryRetryError>>) {
    slint::Timer::single_shot(Duration::from_millis(40),move|| {
        let app = weak.upgrade();
        if !capture.persistence.is_current() || !app.as_ref().is_some_and(|app|capture.matches(app)) {
            cancel.store(true,Ordering::SeqCst);
        }
        let finished = finish_delivery_preparation(&cancel);
        if matches!(finished,Ok(true)) {
            poll_generation_history_save(weak,capture,writes,revision,cancel,receiver); return;
        }
        let Some(app) = app else { return; };
        let success = matches!(finished,Ok(false)) && matches!(receiver.try_recv(),Ok(Ok(())));
        if !success { generation_history_save_failed(&app,&capture,&writes,revision); return; }
        let _ = capture.apply(&app, || {
            let mut pending = writes.borrow_mut();
            if pending.debt.as_ref().is_some_and(|debt|debt.revision == revision) {
                let owns_status = pending.debt.as_ref().is_some_and(|debt|
                    app.global::<AppState>().get_generation_status() == debt.status);
                pending.debt = None;
                if owns_status { app.global::<AppState>().set_generation_status("提示词历史已保存".into()); }
            }
        });
    });
}

pub(super) fn wire_generation_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    state.on_is_generation_error(|message| is_generation_error_message(message.as_str()));
    let store = context.store.clone();
    let history_writes = Rc::new(RefCell::new(GenerationHistoryWrites::default()));

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_generate(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_generation(
                &app,
                context.clone(),
                None,
                true,
                None,
                None,
                ExistingGenerationPolicy::StopExisting,
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_stop_generation(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            stop_generation(&app, &context);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let writes = history_writes.clone();
        state.on_remove_prompt_history(move |prompt| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            mutate_generation_history(&app,&context,writes.clone(),GenerationHistoryAction::Remove(prompt.trim().to_string()));
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let writes = history_writes.clone();
        state.on_clear_prompt_history(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            mutate_generation_history(&app,&context,writes.clone(),GenerationHistoryAction::Clear);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_optimize_current_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            optimize_current_prompt(&app, context.clone(), false);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_cancel_current_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            cancel_current_prompt_task(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_optimize_canvas_text_node(move |id, prompt| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            optimize_canvas_text_node(&app, context.clone(), id.to_string(), prompt.to_string());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_visual_optimize_current_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            optimize_current_prompt(&app, context.clone(), true);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_translate_current_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            translate_current_prompt(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_open_conversation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = GenerationMetadataCapture::capture(&app,&context) else { return; };
            let Ok(_activity) = capture.persistence.begin_activity() else { return; };
            let effects = capture.apply(&app,|| {
                let state = app.global::<AppState>();
                let store = context.store.borrow();
                if !store.generations.iter().any(|item|item.conversation_id == id.as_str())
                    && !state.get_conversations().iter().any(|row|row.id == id) { return None; }
                state.set_current_conversation_id(id.clone());
                let effects = prepare_delivery_visuals(&app,&store).publish_metadata(&app,capture.persistence.clone());
                state.set_current_conversation_id(id);
                Some(effects)
            }).flatten();
            if let Some(effects) = effects { start_activation_visual_effects(&app,context.clone(),effects); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let context = context.clone();
        state.on_regenerate(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let item = store
                .borrow()
                .generations
                .iter()
                .find(|g| g.id == id.to_string())
                .cloned();
            if let Some(item) = item {
                start_asset_regeneration(&app, context.clone(), item);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_retry_generation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            retry_failed_generation(&app, context.clone(), id.to_string());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_retry_generation_delivery(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            retry_failed_delivery(&app, context.clone(), id.to_string());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_optimize_custom_prompt_content(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            optimize_custom_prompt_content(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_dismiss_new_generation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            let Some(capture) = GenerationMetadataCapture::capture(&app,&context) else { return; };
            let Ok(_activity) = capture.persistence.begin_activity() else { return; };
            let effects = capture.apply(&app,|| {
            let mut store_mut = context.store.borrow_mut();
            for item in store_mut.generations.iter_mut() {
                if item.id == id {
                    item.is_new = false;
                }
            }
            for item in store_mut.assets.iter_mut() {
                if item.id == id {
                    item.is_new = false;
                }
            }
            prepare_delivery_visuals(&app,&store_mut).publish_metadata(&app,capture.persistence.clone())
            });
            if let Some(effects) = effects { start_activation_visual_effects(&app,context.clone(),effects); }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_quote_generation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            let Some(capture) = GenerationMetadataCapture::capture(&app,&context) else { return; };
            let Ok(_activity) = capture.persistence.begin_activity() else { return; };
            let _ = capture.apply(&app,|| {
            if let Some(item) = context.store
                .borrow()
                .generations
                .iter()
                .find(|g| g.id == id)
                .cloned()
            {
                let state = app.global::<AppState>();
                state.set_quote_title(item.title.into());
                state.set_quote_prompt(item.prompt.into());
                state.set_quote_ratio(item.ratio.into());
                state.set_quote_quality(item.quality.into());
                state.set_quote_width(item.width);
                state.set_quote_height(item.height);
            }
            });
        });
    }
}

pub(super) fn optimize_current_prompt(app: &AppWindow, context: AppContext, visual_mode: bool) {
    let state = app.global::<AppState>();
    if !require_online_operation(app, "优化提示词") {
        return;
    }
    let category = current_workspace_category(app);
    if !workspace_prompt_optimization_request_id(&state, &category)
        .trim()
        .is_empty()
    {
        return;
    }
    let target_input = state.get_prompt().to_string();
    let raw_prompt = target_input.trim().to_string();
    if raw_prompt.is_empty() {
        state.set_generation_status("请输入需要优化的提示词".into());
        return;
    }
    if visual_mode {
        let category = resolve_category(&state.get_asset_type().to_string(), &raw_prompt);
        if references_for_category(&context.store.borrow().references, &category).is_empty() {
            state.set_generation_status("请先上传参考图".into());
            return;
        }
    }
    if context.backend.is_none() {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    }
    let model_code = if visual_mode {
        let selection = sync_style_analysis_selection(&state);
        if !selection.available {
            state.set_generation_status("服务端没有可用的图片风格分析模型".into());
            return;
        }
        selection.model_code
    } else {
        let preferred_model = state.get_reasoning_model().to_string();
        if preferred_model.trim().is_empty() {
            state.set_generation_status("服务端没有可用的提示词模型".into());
            return;
        }
        preferred_model
    };
    let reference_paths = if visual_mode {
        let category = resolve_category(&state.get_asset_type().to_string(), &raw_prompt);
        references_for_category(&context.store.borrow().references, &category)
            .iter()
            .take(MAX_REFERENCE_IMAGES)
            .map(|reference| PathBuf::from(&reference.source_path))
            .collect()
    } else {
        Vec::new()
    };
    state.set_generation_status(if visual_mode {
        "正在上传参考图并分析风格...".into()
    } else {
        "正在优化提示词...".into()
    });
    start_backend_prompt_task(
        app,
        context.clone(),
        PromptTaskRequest {
            model_code,
            task_type: if visual_mode {
                "image_style_analysis"
            } else {
                "prompt_optimize"
            },
            prompt: if visual_mode {
                format!(
                    "结合上传参考图的视觉风格优化以下生图描述，只返回优化后的提示词：{raw_prompt}"
                )
            } else {
                raw_prompt
            },
            target_language: None,
            optimize: true,
            target: PromptResultTarget::Composer {
                category,
                input: target_input,
            },
            reference_paths,
        },
    );
}

fn optimize_custom_prompt_content(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    if !require_online_operation(app, "优化提示词") {
        state.set_custom_prompt_message("提示词优化需要联网，请检查网络后重试".into());
        return;
    }
    if state.get_optimizing_prompt() {
        return;
    }
    let target_input = state.get_custom_prompt_input().to_string();
    let raw_prompt = target_input.trim().to_string();
    if raw_prompt.is_empty() {
        state.set_custom_prompt_message("请先输入需要优化的提示词内容".into());
        return;
    }
    if context.backend.is_none() {
        state.set_custom_prompt_message("服务端尚未初始化，请重启客户端后重试".into());
        return;
    }
    let model_code = state.get_reasoning_model().to_string();
    if model_code.trim().is_empty() {
        state.set_custom_prompt_message("服务端没有可用的提示词模型".into());
        return;
    }
    let target_id = state.get_custom_prompt_editor_session_id().to_string();
    state.set_optimizing_prompt(true);
    state.set_custom_prompt_message("正在优化提示词内容...".into());
    start_backend_prompt_task(
        app,
        context.clone(),
        PromptTaskRequest {
            model_code,
            task_type: "prompt_optimize",
            prompt: raw_prompt,
            target_language: None,
            optimize: true,
            target: PromptResultTarget::CustomPrompt {
                session_id: target_id,
                input: target_input,
                append_result: false,
            },
            reference_paths: Vec::new(),
        },
    );
}

fn optimize_canvas_text_node(app: &AppWindow, context: AppContext, id: String, prompt: String) {
    let state = app.global::<AppState>();
    if !require_online_operation(app, "优化提示词") || state.get_optimizing_prompt() {
        return;
    }
    let target_input = prompt;
    let raw_prompt = target_input.trim().to_string();
    if raw_prompt.is_empty() {
        state.set_generation_status("请先输入需要优化的文字内容".into());
        return;
    }
    if context.backend.is_none() {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    }
    let model_code = state.get_reasoning_model().to_string();
    if model_code.trim().is_empty() {
        state.set_generation_status("服务端没有可用的提示词模型".into());
        return;
    }

    state.set_generation_status("正在优化文字节点提示词...".into());
    state.set_optimizing_prompt(true);
    start_backend_prompt_task(
        app,
        context.clone(),
        PromptTaskRequest {
            model_code,
            task_type: "prompt_optimize",
            prompt: raw_prompt,
            target_language: None,
            optimize: true,
            target: PromptResultTarget::CanvasNode { id, input: target_input },
            reference_paths: Vec::new(),
        },
    );
}

pub(super) fn translate_current_prompt(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    if !require_online_operation(app, "翻译提示词") {
        state.set_translate_prompt(false);
        return;
    }
    if state.get_translating_prompt() {
        return;
    }
    let target_input = state.get_prompt().to_string();
    let raw_prompt = target_input.trim().to_string();
    if raw_prompt.is_empty() {
        state.set_translate_prompt(false);
        return;
    }
    if context.backend.is_none() {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        state.set_translate_prompt(false);
        return;
    }
    let model_code = state.get_reasoning_model().to_string();
    if model_code.trim().is_empty() {
        state.set_generation_status("服务端没有可用的提示词模型".into());
        state.set_translate_prompt(false);
        return;
    }
    state.set_translating_prompt(true);
    state.set_generation_status("正在翻译提示词...".into());
    start_backend_prompt_task(
        app,
        context.clone(),
        PromptTaskRequest {
            model_code,
            task_type: "prompt_translate",
            prompt: raw_prompt,
            target_language: Some("English".to_string()),
            optimize: false,
            target: PromptResultTarget::Composer {
                category: current_workspace_category(app),
                input: target_input,
            },
            reference_paths: Vec::new(),
        },
    );
}
