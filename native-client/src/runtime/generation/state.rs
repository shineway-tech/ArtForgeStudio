use super::*;

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

pub(super) fn finish_conversation_placeholder(state: &AppState, conversation_id: &str, image: Option<Image>) {
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
