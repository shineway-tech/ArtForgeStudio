use super::*;

#[path = "video_prompt_format.rs"]
mod formatting;

pub(super) fn wire_video_prompt_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let draft_epoch = Rc::new(Cell::new(0_u64));
    {
        let weak = app.as_weak();
        let context = context.clone();
        state.on_optimize_video_prompt(move || {
            let Some(app) = weak.upgrade() else { return };
            let Some(persistence) = context.store.borrow().private_persistence.clone() else { return };
            let task = apply_video_prompt(&context, &persistence, || {
                let state = app.global::<AppState>();
                if state.get_optimizing_video_prompt() || state.get_video_generating() {
                    return None;
                }
                if state.get_session_state() != "online" {
                    state.set_video_prompt_status("视频提示词优化需要联网，请检查网络后重试".into());
                    return None;
                }
                match video_prompt_task(&state) {
                    Ok(task) => {
                        state.set_video_prompt_status("正在按时间线整理视频提示词的五项内容...".into());
                        state.set_optimizing_video_prompt(true);
                        Some(task)
                    }
                    Err(reason) => {
                        state.set_video_prompt_status(reason.into());
                        None
                    }
                }
            }).flatten();
            if let Some(task) = task {
                // This producer captures billing/namespace itself. Never enter it
                // under apply_user_completion: capture consults the same latch.
                start_backend_prompt_task(&app, context.clone(), task);
            }
        });
    }
    {
        let weak = app.as_weak();
        let context = context.clone();
        state.on_open_video_prompt_editor(move || {
            let Some(app) = weak.upgrade() else { return };
            let Some(persistence) = context.store.borrow().private_persistence.clone() else { return };
            let opened = apply_video_prompt(&context, &persistence, || {
                let state = app.global::<AppState>();
                if state.get_page() == "video-generation" {
                    state.set_video_prompt_expanded_open(true);
                    return true;
                }
                false
            }).unwrap_or(false);
            if opened { set_video_player_visible(false); }
        });
    }
    {
        let weak = app.as_weak();
        state.on_video_prompt_edited(move || {
            let Some(app) = weak.upgrade() else { return };
            let Some(persistence) = context.store.borrow().private_persistence.clone() else { return };
            let eligible = apply_video_prompt(&context, &persistence, || {
                let state = app.global::<AppState>();
                state.get_page() == "video-generation" && !state.get_video_source_id().trim().is_empty()
            }).unwrap_or(false);
            if !eligible { return; }

            // Preparation admits counted durable/activity work outside the latch.
            let mut prepared = match persistence.prepare_ordered_save() {
                Ok(prepared) => Some(prepared),
                Err(_) => {
                    let _ = apply_video_prompt(&context, &persistence, || {
                        app.global::<AppState>().set_video_prompt_status("草稿未保存，输入仍保留，请重试".into());
                    });
                    return;
                }
            };
            let enqueued = apply_video_prompt(&context, &persistence, || {
                let state = app.global::<AppState>();
                if state.get_page() != "video-generation" || state.get_video_source_id().trim().is_empty() {
                    return None;
                }
                let Some(expected) = draft_epoch.get().checked_add(1).filter(|value| *value != u64::MAX) else {
                    draft_epoch.set(u64::MAX);
                    state.set_video_prompt_status("草稿未保存，请重启后继续；输入仍保留".into());
                    return None;
                };
                draft_epoch.set(expected);
                let source_id = state.get_video_source_id().to_string();
                let prompt = state.get_video_prompt().to_string();
                let data = {
                    let mut store = context.store.borrow_mut();
                    store_video_prompt_draft(
                        &mut store.prompt_drafts, persistence.lease().namespace.user_public_id(),
                        &source_id, &prompt,
                    );
                    local_store_data(&app, &store)
                };
                state.set_video_prompt_status("正在保存草稿…".into());
                // Return the WHOLE guarded result. No error conversion/drop in this closure.
                let result = prepared.take().expect("prepared once").enqueue(data);
                Some((expected, source_id, prompt, result))
            }).flatten();
            drop(prepared); // Completion denial releases admission only outside the latch.
            let Some((expected, source_id, prompt, result)) = enqueued else { return };
            match result {
                Ok(receiver) => poll_video_prompt_save(
                    app.as_weak(), context.clone(), persistence, draft_epoch.clone(),
                    expected, source_id, prompt, receiver,
                ),
                Err(error) => {
                    drop(error); // Owns unqueued permits; never drop within completion.
                    let _ = apply_video_prompt(&context, &persistence, || {
                        app.global::<AppState>().set_video_prompt_status("草稿未保存，输入仍保留，请重试".into());
                    });
                }
            }
        });
    }
}

fn apply_video_prompt<R>(
    context: &AppContext, persistence: &PrivatePersistence, apply: impl FnOnce() -> R,
) -> Option<R> {
    let _activity = persistence.begin_activity().ok()?;
    // same_binding checks the captured latch; do not nest it in completion.
    if !context.store.borrow().private_persistence.as_ref()
        .is_some_and(|current| current.same_binding(persistence)) {
        return None;
    }
    context.apply_user_completion(persistence.lease(), apply).ok()
}

fn poll_video_prompt_save(
    weak: Weak<AppWindow>, context: AppContext, persistence: PrivatePersistence,
    epoch: Rc<Cell<u64>>, expected: u64, source_id: String, prompt: String,
    receiver: mpsc::Receiver<client_state::WriteResult>,
) {
    // The write is already ordered/enqueued; this timer merely observes its ack.
    // Its receiver does not own the writer's durable/activity admission.
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let succeeded = match receiver.try_recv() {
            Ok(result) => result.is_ok(),
            Err(TryRecvError::Empty) => {
                poll_video_prompt_save(weak, context, persistence, epoch, expected, source_id, prompt, receiver);
                return;
            }
            Err(TryRecvError::Disconnected) => false,
        };
        let Some(app) = weak.upgrade() else { return };
        let _ = apply_video_prompt(&context, &persistence, || {
            let state = app.global::<AppState>();
            if epoch.get() != expected || state.get_page() != "video-generation"
                || state.get_video_source_id().as_str() != source_id
                || state.get_video_prompt().as_str() != prompt {
                return;
            }
            state.set_video_prompt_status(if succeeded {
                "草稿已保存"
            } else {
                "草稿未保存，输入仍保留，请重试"
            }.into());
        });
    });
}

fn video_prompt_task(state: &AppState) -> std::result::Result<PromptTaskRequest, &'static str> {
    if state.get_page() != "video-generation" || state.get_video_source_id().trim().is_empty() {
        return Err("请先选择需要生成视频的图片");
    }
    let input = state.get_video_prompt().to_string();
    if input.trim().is_empty() {
        return Err("请先填写视频提示词");
    }
    let model_code = state.get_reasoning_model().to_string();
    if model_code.trim().is_empty() {
        return Err("服务端没有可用的提示词模型");
    }
    Ok(PromptTaskRequest {
        model_code,
        task_type: "prompt_optimize",
        prompt: formatting::optimization_input(&input, state.get_video_duration_seconds()),
        target_language: None,
        optimize: true,
        target: PromptResultTarget::Video {
            source_id: state.get_video_source_id().to_string(),
            input,
        },
        reference_paths: Vec::new(),
    })
}

pub(super) fn store_video_prompt_draft(
    drafts: &mut PromptDrafts,
    owner: &str,
    source_id: &str,
    prompt: &str,
) {
    // Retain the latest video draft per account, without mixing it with image drafts.
    drafts.video_by_owner.insert(
        owner.to_string(),
        VideoPromptDraft {
            source_id: source_id.to_string(),
            prompt: prompt.to_string(),
        },
    );
}

pub(super) fn video_prompt_for_source(
    drafts: &PromptDrafts,
    owner: &str,
    source_id: &str,
    fallback: &str,
) -> String {
    drafts
        .video_by_owner
        .get(owner)
        .filter(|draft| draft.source_id == source_id)
        .map(|draft| draft.prompt.clone())
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_prompt_failure_preserves_input_and_does_not_overwrite_quote_status() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let fixture = core_scope_tests::Fixture::new();
        wire_video_prompt_callbacks(&app, fixture.context.clone());
        let state = app.global::<AppState>();
        state.set_page("video-generation".into());
        state.set_session_state("online".into());
        state.set_video_source_id("image-a".into());
        state.set_video_prompt("original video prompt".into());
        state.set_prompt("original image prompt".into());
        state.set_reasoning_model("server-prompt-model".into());
        state.set_video_status("quote ready".into());
        state.invoke_optimize_video_prompt();
        assert!(!state.get_optimizing_video_prompt());
        assert!(!state.get_video_prompt_status().is_empty());
        assert_eq!(state.get_video_prompt(), "original video prompt");
        assert_eq!(state.get_prompt(), "original image prompt");
        assert_eq!(state.get_video_status(), "quote ready");
        state.set_video_generating(true);
        state.set_video_prompt_status("unchanged while generating".into());
        state.invoke_optimize_video_prompt();
        assert_eq!(
            state.get_video_prompt_status(),
            "unchanged while generating"
        );
    }

    #[test]
    fn video_prompt_task_uses_existing_api_and_captures_only_the_video_input() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_page("video-generation".into());
        state.set_video_source_id("image-a".into());
        state.set_video_prompt("  镜头缓慢推进，保持人物不变。\n  ".into());
        state.set_prompt("unrelated image prompt".into());
        state.set_reasoning_model("server-prompt-model".into());
        state.set_video_duration_seconds(8);
        let task = video_prompt_task(&state).unwrap();
        assert_eq!(task.task_type, "prompt_optimize");
        assert_eq!(task.model_code, "server-prompt-model");
        assert!(task.prompt.contains("图生视频"));
        assert!(task.prompt.ends_with("镜头缓慢推进，保持人物不变。"));
        assert!(!task.prompt.contains("unrelated image prompt"));
        assert!(task.reference_paths.is_empty());
        assert!(task.prompt.contains("时间线：0–4秒"));
        assert!(task.prompt.contains("时间线：4–8秒"));
        assert!(
            matches!(task.target, PromptResultTarget::Video { source_id, input }
            if source_id == "image-a" && input == "  镜头缓慢推进，保持人物不变。\n  ")
        );
        state.set_video_duration_seconds(15);
        let task = video_prompt_task(&state).unwrap();
        assert!(task.prompt.contains("时间线：12–15秒"));
        state.set_video_prompt(" \n ".into());
        assert!(video_prompt_task(&state).is_err());
    }

    #[test]
    fn video_prompt_draft_is_scoped_to_account_and_source() {
        let mut drafts = PromptDrafts::default();
        drafts.scene = "image prompt".into();
        store_video_prompt_draft(&mut drafts, "user-a", "image-a", "edited video prompt");
        assert_eq!(
            video_prompt_for_source(&drafts, "user-a", "image-a", "original"),
            "edited video prompt"
        );
        assert_eq!(
            video_prompt_for_source(&drafts, "user-b", "image-a", "original"),
            "original"
        );
        assert_eq!(
            video_prompt_for_source(&drafts, "user-a", "image-b", "original"),
            "original"
        );
        assert_eq!(drafts.scene, "image prompt");
    }
}

#[cfg(test)]
mod core_scope_tests {
    use super::*;

    const OWNER: &str = "11111111-1111-4111-8111-111111111111";
    const OTHER: &str = "22222222-2222-4222-8222-222222222222";

    pub(super) struct Fixture {
        pub context: AppContext,
        pub persistence: PrivatePersistence,
        pub lease: NamespaceLease,
        pub writer: client_state::tests::Fixture,
    }

    impl Fixture {
        pub(super) fn new() -> Self {
            let writer = client_state::tests::Fixture::new(false, false);
            let session = Arc::new(SessionManager::new(Arc::new(
                crate::runtime::test_support::MemoryRefreshTokenStore::default(),
            )));
            let scope = session.install_tokens_for_user(&TokenSet {
                access_token: "draft-access".into(), access_expires_in_seconds: 1800,
                refresh_token: "draft-refresh".into(),
                refresh_expires_at: "2099-01-01T00:00:00Z".into(), token_type: "X-Token".into(),
            }, OWNER).unwrap();
            let lease = writer.lease(OWNER, scope.auth_epoch, 1);
            writer.activate(lease.clone()).unwrap();
            let api = ApiClient::new(ApiClientConfig {
                base_url: reqwest::Url::parse("http://127.0.0.1:9/").unwrap(),
                app_version: "999.0.0".into(), timeout: Duration::from_millis(50),
            }, DeviceIdentity { id: Uuid::new_v4().to_string(), name: "draft-fixture".into(), platform: "macos".into() }, session).unwrap();
            let context = AppContext {
                backend: Some(Arc::new(BackendRuntime { api })),
                current_user_id: Arc::new(Mutex::new(Some(OWNER.into()))),
                ..Default::default()
            };
            context.user_activity.activate(lease.clone()).unwrap();
            *context.active_namespace.lock().unwrap() = Some(lease.clone());
            let persistence = PrivatePersistence::for_test(
                (*writer).clone(), lease.clone(), context.user_activity.clone(),
                context.backend.as_ref().unwrap().api.upgrade_latch().clone(),
            );
            context.store.borrow_mut().private_persistence = Some(persistence.clone());
            Self { context, persistence, lease, writer }
        }

        fn connection(&self) -> rusqlite::Connection {
            // Test-owned fixture location only; production must never reopen a writer path.
            let root = self.lease.namespace.root().parent().unwrap().parent().unwrap();
            rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap()
        }

        fn stored_prompt(&self) -> Option<String> {
            self.writer.load_client_state_for_namespace(&self.lease).unwrap()
                .and_then(|data| data.prompt_drafts.video_by_owner.get(OWNER).map(|draft| draft.prompt.clone()))
        }

        fn drain(&self) {
            self.context.user_activity.begin_quiesce(&self.lease).unwrap().retire();
        }
    }

    fn setup() -> (Fixture, AppWindow) {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = Fixture::new();
        let app = AppWindow::new().unwrap();
        wire_video_prompt_callbacks(&app, fixture.context.clone());
        let state = app.global::<AppState>();
        state.set_page("video-generation".into());
        state.set_session_state("online".into());
        state.set_video_source_id("image-a".into());
        state.set_video_prompt("first draft".into());
        state.set_reasoning_model("server-prompt-model".into());
        (fixture, app)
    }

    fn pump(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(6);
        while !ready() && Instant::now() < deadline {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(ready(), "video draft completion was not observed");
    }

    #[test]
    fn core_video_prompt_exact_upgrade_blocks_editor_optimization_and_draft() {
        let (fixture, app) = setup();
        let state = app.global::<AppState>();
        state.set_video_prompt_status("unchanged".into());
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        state.invoke_open_video_prompt_editor();
        state.invoke_optimize_video_prompt();
        state.invoke_video_prompt_edited();
        assert!(!state.get_video_prompt_expanded_open());
        assert!(!state.get_optimizing_video_prompt());
        assert_eq!(state.get_video_prompt_status(), "unchanged");
        assert!(fixture.context.store.borrow().prompt_drafts.video_by_owner.is_empty());
        assert!(fixture.stored_prompt().is_none());
        fixture.drain();
    }

    #[test]
    fn core_video_prompt_foreign_active_namespace_cannot_use_an_old_store() {
        let (fixture, app) = setup();
        *fixture.context.active_namespace.lock().unwrap() = Some(fixture.writer.lease(OTHER, 1, 2));
        app.global::<AppState>().set_video_prompt_status("B state".into());
        app.global::<AppState>().invoke_open_video_prompt_editor();
        app.global::<AppState>().invoke_optimize_video_prompt();
        app.global::<AppState>().invoke_video_prompt_edited();
        assert!(!app.global::<AppState>().get_video_prompt_expanded_open());
        assert_eq!(app.global::<AppState>().get_video_prompt_status(), "B state");
        assert!(fixture.context.store.borrow().prompt_drafts.video_by_owner.is_empty());
        assert!(fixture.stored_prompt().is_none());
        fixture.drain();
    }

    #[test]
    fn core_video_prompt_draft_uses_lease_owner_and_keeps_enqueue_order() {
        let (fixture, app) = setup();
        *fixture.context.current_user_id.lock().unwrap() = Some(OTHER.into());
        app.global::<AppState>().invoke_video_prompt_edited();
        app.global::<AppState>().set_video_prompt("latest draft".into());
        app.global::<AppState>().invoke_video_prompt_edited();
        fixture.writer.flush(&fixture.lease).unwrap();
        pump(|| app.global::<AppState>().get_video_prompt_status().as_str() == "草稿已保存");
        assert_eq!(fixture.stored_prompt().as_deref(), Some("latest draft"));
        let store = fixture.context.store.borrow();
        assert_eq!(store.prompt_drafts.video_by_owner.get(OWNER).unwrap().prompt, "latest draft");
        assert!(!store.prompt_drafts.video_by_owner.contains_key(OTHER));
        drop(store);
        fixture.drain();
    }

    #[test]
    fn core_video_prompt_draft_waits_for_sqlite_ack_before_success() {
        let (fixture, app) = setup();
        let connection = fixture.connection();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        app.global::<AppState>().invoke_video_prompt_edited();
        assert_ne!(app.global::<AppState>().get_video_prompt_status(), "草稿已保存");
        connection.execute_batch("ROLLBACK").unwrap();
        fixture.writer.flush(&fixture.lease).unwrap();
        pump(|| app.global::<AppState>().get_video_prompt_status().as_str() == "草稿已保存");
        assert_eq!(fixture.stored_prompt().as_deref(), Some("first draft"));
        fixture.drain();
    }

    #[test]
    fn core_video_prompt_draft_sqlite_failure_preserves_input_without_success() {
        let (fixture, app) = setup();
        fixture.connection().execute_batch(
            "CREATE TRIGGER reject_video_draft BEFORE INSERT ON user_settings WHEN NEW.key = 'prompt_drafts' BEGIN SELECT RAISE(ABORT, 'fixture refused draft'); END;"
        ).unwrap();
        app.global::<AppState>().invoke_video_prompt_edited();
        fixture.writer.flush(&fixture.lease).unwrap();
        pump(|| app.global::<AppState>().get_video_prompt_status().contains("未保存"));
        assert_eq!(app.global::<AppState>().get_video_prompt(), "first draft");
        assert_eq!(fixture.context.store.borrow().prompt_drafts.video_by_owner.get(OWNER).unwrap().prompt, "first draft");
        assert!(fixture.stored_prompt().is_none());
        fixture.drain();
    }

    #[test]
    fn core_video_prompt_late_draft_ack_cannot_change_retired_ui() {
        let (fixture, app) = setup();
        app.global::<AppState>().invoke_video_prompt_edited();
        fixture.writer.flush(&fixture.lease).unwrap();
        fixture.drain();
        app.global::<AppState>().set_video_prompt_status("new account".into());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(app.global::<AppState>().get_video_prompt_status(), "new account");
        assert_eq!(fixture.stored_prompt().as_deref(), Some("first draft"));
    }

    #[test]
    fn core_video_prompt_late_ack_after_upgrade_preserves_disk_without_ui_success() {
        let (fixture, app) = setup();
        app.global::<AppState>().invoke_video_prompt_edited();
        fixture.writer.flush(&fixture.lease).unwrap();
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        app.global::<AppState>().set_video_prompt_status("upgrade boundary".into());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(app.global::<AppState>().get_video_prompt_status(), "upgrade boundary");
        assert_eq!(fixture.stored_prompt().as_deref(), Some("first draft"));
        fixture.drain();
    }
}
