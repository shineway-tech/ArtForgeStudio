use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GenerationScopeDisposition {
    Current,
    CapturedTerminal,
    Stale,
}

pub(super) fn current_workspace_category(app: &AppWindow) -> String {
    resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "")
}

pub(super) fn category_is_generating(context: &AppContext, category: &str) -> bool {
    context.generations.active.borrow().contains_key(category)
}

pub(super) fn active_generation_matches(
    context: &AppContext,
    category: &str,
    task_id: &str,
) -> bool {
    context
        .generations
        .active
        .borrow()
        .get(category)
        .is_some_and(|task| task.task_id == task_id)
}

pub(super) fn active_generation_matches_scope(
    context: &AppContext,
    category: &str,
    task_id: &str,
    session_scope: &SessionScope,
) -> bool {
    context
        .generations
        .active
        .borrow()
        .get(category)
        .is_some_and(|task| task.task_id == task_id && task.session_scope == *session_scope)
}

pub(super) fn current_generation_session_scope(context: &AppContext) -> Option<SessionScope> {
    let owner_user_id = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone()
        .filter(|value| !value.trim().is_empty())?;
    context
        .backend
        .as_ref()?
        .api
        .session()
        .scope_for_user(&owner_user_id)
}

pub(super) fn generation_scope_matches_context(
    context: &AppContext,
    session_scope: &SessionScope,
) -> bool {
    let current_owner = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone();
    current_owner.as_deref() == Some(session_scope.owner_user_id.as_str())
        && context
            .backend
            .as_ref()
            .is_some_and(|backend| backend.api.session().is_scope_current(session_scope))
}

pub(super) fn generation_scope_disposition(
    context: &AppContext,
    session_scope: &SessionScope,
) -> GenerationScopeDisposition {
    if generation_scope_matches_context(context, session_scope) {
        GenerationScopeDisposition::Current
    } else if terminal_auth_scope_matches_context(context, session_scope) {
        GenerationScopeDisposition::CapturedTerminal
    } else {
        GenerationScopeDisposition::Stale
    }
}

pub(super) fn generation_scope_allows_polling(
    app_weak: &Weak<AppWindow>,
    context: &AppContext,
    session_scope: &SessionScope,
) -> bool {
    match generation_scope_disposition(context, session_scope) {
        GenerationScopeDisposition::Current => true,
        GenerationScopeDisposition::CapturedTerminal => {
            if terminal_auth_scope_matches_context(context, session_scope) {
                if let Some(app) = app_weak.upgrade() {
                    sign_out_locally(&app, context, true, Some(session_scope.auth_epoch));
                }
            }
            false
        }
        GenerationScopeDisposition::Stale => false,
    }
}

pub(super) fn observe_detached_generation_scope(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<()>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let finished = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => {
                    slot.take();
                    true
                }
                Err(TryRecvError::Empty) => false,
            }
        };
        if !finished {
            observe_detached_generation_scope(app_weak, context, session_scope, receiver);
        }
    });
}

pub(super) fn clear_generation_account_state(app: &AppWindow, context: &AppContext) {
    context.generations.active.borrow_mut().clear();
    context.generations.statuses.borrow_mut().clear();
    if let Ok(mut cancellations) = context.cancelled_generation_requests.lock() {
        cancellations.clear();
    }
    let state = app.global::<AppState>();
    state.set_generating(false);
    state.set_generation_loading_count(0);
    state.set_generation_progress(0);
    state.set_generation_eta(0);
    state.set_generation_task_id("".into());
    state.set_generation_active_category("".into());
    state.set_generation_active_prompt("".into());
    state.set_current_conversation_id("".into());
    push_conversations(app, &context.store.borrow());
    state.set_image_editor_generating(false);
    state.set_viewer_processing(false);
    state.set_viewer_processing_progress(0);
    state.set_cutout_processing(false);
    state.set_cutout_progress(0);
    state.set_enhance_processing(false);
    state.set_enhance_progress(0);
    state.set_watermark_processing(false);
    state.set_watermark_progress(0);
    state.set_colorize_processing(false);
    state.set_colorize_progress(0);
}

pub(super) fn insert_active_generation(context: &AppContext, task: ActiveGeneration) {
    context
        .generations
        .active
        .borrow_mut()
        .insert(task.category.clone(), task);
}

pub(super) fn remove_active_generation(
    context: &AppContext,
    category: &str,
    task_id: &str,
) -> Option<ActiveGeneration> {
    let mut tasks = context.generations.active.borrow_mut();
    if tasks
        .get(category)
        .is_some_and(|task| task.task_id == task_id)
    {
        tasks.remove(category)
    } else {
        None
    }
}

pub(super) fn set_generation_status_for_category(
    context: &AppContext,
    app: &AppWindow,
    category: &str,
    status: &str,
) {
    context
        .generations
        .statuses
        .borrow_mut()
        .insert(category.to_string(), status.to_string());
    if current_workspace_category(app) == category {
        app.global::<AppState>()
            .set_generation_status(status.to_string().into());
    }
}

pub(super) fn update_active_generation_progress(
    context: &AppContext,
    app: &AppWindow,
    category: &str,
    task_id: &str,
    progress: i32,
    eta: i32,
) {
    if let Some(task) = context.generations.active.borrow_mut().get_mut(category) {
        if task.task_id == task_id {
            task.progress = progress;
            task.eta = eta;
        }
    }
    if current_workspace_category(app) == category {
        let state = app.global::<AppState>();
        state.set_generation_progress(progress);
        state.set_generation_eta(eta);
    }
}

pub(super) fn mark_active_generation_image_completed(
    context: &AppContext,
    app: &AppWindow,
    category: &str,
    task_id: &str,
    success: bool,
    success_id: Option<String>,
    failure_reason: Option<&str>,
) -> Option<ActiveGeneration> {
    let active = {
        let mut tasks = context.generations.active.borrow_mut();
        let task = tasks.get_mut(category)?;
        if task.task_id != task_id {
            return None;
        }
        task.completed_count = (task.completed_count + 1).min(task.total_count.max(1));
        task.loading_count = (task.total_count - task.completed_count).max(0);
        if success {
            task.success_count += 1;
            task.latest_success_id = success_id;
        } else {
            task.failed_count += 1;
            if let Some(reason) = failure_reason.filter(|value| !value.trim().is_empty()) {
                task.last_failure_reason = Some(reason.to_string());
            }
        }
        let total = task.total_count.max(1);
        task.progress = (8 + task.completed_count * 88 / total).clamp(1, 96);
        task.eta = if task.loading_count > 0 {
            IMAGE_GENERATION_WAIT_SECS as i32
        } else {
            0
        };
        Some(task.clone())
    };
    sync_generation_state_for_current_category(context, app);
    active
}

pub(super) fn sync_generation_state_for_current_category(context: &AppContext, app: &AppWindow) {
    let state = app.global::<AppState>();
    let category = current_workspace_category(app);
    let active = context.generations.active.borrow().get(&category).cloned();
    if let Some(task) = active {
        state.set_generating(true);
        state.set_generation_loading_count(task.loading_count);
        state.set_generation_task_id(task.task_id.into());
        state.set_generation_active_category(category.clone().into());
        state.set_generation_active_prompt(task.prompt.into());
        state.set_generation_active_credit_cost(task.credit_cost);
        state.set_generation_progress(task.progress);
        state.set_generation_eta(task.eta);
        let status = context
            .generations
            .statuses
            .borrow()
            .get(&category)
            .cloned()
            .unwrap_or_else(|| "正在生成...".to_string());
        state.set_generation_status(status.into());
    } else {
        state.set_generating(false);
        state.set_generation_loading_count(0);
        state.set_generation_task_id("".into());
        state.set_generation_active_category("".into());
        state.set_generation_active_prompt("".into());
        state.set_generation_active_credit_cost(0);
        state.set_generation_progress(0);
        state.set_generation_eta(0);
        let status = context
            .generations
            .statuses
            .borrow()
            .get(&category)
            .cloned()
            .unwrap_or_default();
        state.set_generation_status(status.into());
    }
}

pub(super) fn finish_conversation_placeholder(
    state: &AppState,
    conversation_id: &str,
    image: Option<Image>,
) {
    let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
    if let Some(row) = conversations
        .iter_mut()
        .find(|c| c.loading && c.id.as_str() == conversation_id)
    {
        if let Some(image) = image {
            row.image = image;
        }
        row.loading = false;
    }
    state.set_conversations(ModelRc::new(VecModel::from(conversations)));
}

pub(super) fn remove_conversation_placeholder(state: &AppState, conversation_id: &str) {
    let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
    let before = conversations.len();
    conversations.retain(|item| !(item.loading && item.id.as_str() == conversation_id));
    if conversations.len() == before {
        return;
    }

    let was_current = state.get_current_conversation_id().as_str() == conversation_id;
    let next_current = conversations
        .first()
        .map(|item| item.id.to_string())
        .unwrap_or_default();
    state.set_conversations(ModelRc::new(VecModel::from(conversations)));
    if was_current {
        state.set_current_conversation_id(next_current.into());
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    struct ScopeFixture {
        context: AppContext,
        session: Arc<SessionManager>,
        data_dir: PathBuf,
    }

    impl Drop for ScopeFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.data_dir);
        }
    }

    fn token_set(access_token: &str, refresh_token: &str) -> TokenSet {
        TokenSet {
            access_token: access_token.to_string(),
            access_expires_in_seconds: 1800,
            refresh_token: refresh_token.to_string(),
            refresh_expires_at: "2099-01-01T00:00:00Z".to_string(),
            token_type: "X-Token".to_string(),
        }
    }

    fn scope_fixture(owner_user_id: &str) -> (ScopeFixture, SessionScope) {
        let data_dir = std::env::temp_dir().join(format!(
            "artforge-generation-scope-test-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&data_dir).unwrap();
        let session = Arc::new(SessionManager::with_file_store(&data_dir));
        let scope = session
            .install_tokens_for_user(&token_set("access-a", "refresh-a"), owner_user_id)
            .unwrap();
        let api = ApiClient::new(
            ApiClientConfig {
                base_url: reqwest::Url::parse("http://127.0.0.1/").unwrap(),
                app_version: "test".to_string(),
                timeout: Duration::from_secs(1),
            },
            DeviceIdentity {
                id: "generation-scope-test-device".to_string(),
                name: "Generation scope test".to_string(),
                platform: "test".to_string(),
            },
            session.clone(),
        )
        .unwrap();
        let context = AppContext {
            backend: Some(Arc::new(BackendRuntime { api })),
            current_user_id: Arc::new(Mutex::new(Some(owner_user_id.to_string()))),
            ..AppContext::default()
        };
        (
            ScopeFixture {
                context,
                session,
                data_dir,
            },
            scope,
        )
    }

    #[test]
    fn terminal_auth_loss_is_distinct_from_a_stale_new_account() {
        let (fixture, scope_a) = scope_fixture("user-a");
        assert_eq!(
            generation_scope_disposition(&fixture.context, &scope_a),
            GenerationScopeDisposition::Current
        );

        fixture.session.clear_scope(&scope_a).unwrap();
        assert_eq!(
            generation_scope_disposition(&fixture.context, &scope_a),
            GenerationScopeDisposition::CapturedTerminal
        );

        let scope_b = fixture
            .session
            .install_tokens_for_user(&token_set("access-b", "refresh-b"), "user-b")
            .unwrap();
        *fixture
            .context
            .current_user_id
            .lock()
            .unwrap_or_else(|value| value.into_inner()) = Some("user-b".to_string());
        assert_eq!(
            generation_scope_disposition(&fixture.context, &scope_a),
            GenerationScopeDisposition::Stale
        );
        assert_eq!(
            generation_scope_disposition(&fixture.context, &scope_b),
            GenerationScopeDisposition::Current
        );
    }
}
