use super::*;
use std::cell::Cell;

enum InvitationSubmissionOutcome {
    Failed(ApiError),
    Submitted {
        message: Option<String>,
        dashboard: Result<InvitationDashboard, ApiError>,
    },
}

pub(super) fn apply_invitation_dashboard(app: &AppWindow, dashboard: &InvitationDashboard) {
    let state = app.global::<AppState>();
    let overview = &dashboard.overview;
    state.set_invitation_reward_rate(overview.reward_rate_percent.clone().into());
    state.set_invitation_count(overview.invitation_count.to_string().into());
    state.set_invitation_history_reward(overview.total_reward_credits.clone().into());
    state.set_invitation_own_code(overview.invitation_code.clone().unwrap_or_default().into());
    state.set_invitation_rule_description(overview.rule_description.clone().into());
    state.set_invitation_users(ModelRc::new(VecModel::from(
        dashboard
            .users
            .iter()
            .map(invited_user_view)
            .collect::<Vec<_>>(),
    )));
    let next_cursor = dashboard.users_next_cursor.clone().unwrap_or_default();
    state.set_invitation_users_has_more(!next_cursor.is_empty());
    state.set_invitation_users_next_cursor(next_cursor.into());
    state.set_invitation_users_loading(false);
    state.set_invitation_users_message("".into());
    state.set_invitation_rewards_status(if overview.enabled {
        "".into()
    } else {
        "邀请返利当前未开放".into()
    });
}

pub(super) fn wire_invitation_code_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let state = app.global::<AppState>();
    let app_weak = app.as_weak();
    let operation_epoch = Rc::new(Cell::new(0_u64));
    let pagination_context = context.clone();
    let pagination_backend = backend.clone();

    state.on_submit_invitation_code(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if state.get_invitation_code_busy() || state.get_invitation_code_submitted() {
            if state.get_invitation_code_submitted() {
                state.set_invitation_code_status(
                    "当前账号已填写过邀请码，每个账号只能填写一次".into(),
                );
            }
            return;
        }

        let code = state.get_invitation_code().trim().to_string();
        state.set_invitation_code_success(false);
        if code.is_empty() {
            state.set_invitation_code_status("请填写邀请码".into());
            return;
        }

        let Some(session_scope) = context.current_account_session_scope() else {
            state.set_invitation_code_status("登录状态已失效，请重新登录".into());
            return;
        };

        let request_epoch = advance_invitation_operation(&operation_epoch);
        state.set_invitation_code_busy(true);
        state.set_invitation_code_status("正在验证邀请码…".into());
        let api = AccountApi::new(backend.api.clone());
        let worker_scope = session_scope.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let outcome = match api.submit_invitation_code_scoped(&code, &worker_scope) {
                Ok(message) => InvitationSubmissionOutcome::Submitted {
                    message,
                    dashboard: api.invitation_dashboard_scoped(&worker_scope),
                },
                Err(error) => InvitationSubmissionOutcome::Failed(error),
            };
            let _ = sender.send(outcome);
        });
        poll_invitation_submission(
            app.as_weak(),
            context.clone(),
            operation_epoch.clone(),
            request_epoch,
            session_scope,
            Rc::new(RefCell::new(Some(receiver))),
        );
    });

    let app_weak = app.as_weak();
    let context = pagination_context;
    state.on_load_more_invitation_users(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if state.get_invitation_users_loading() || !state.get_invitation_users_has_more() {
            return;
        }
        let cursor = state.get_invitation_users_next_cursor().trim().to_string();
        if cursor.is_empty() {
            state.set_invitation_users_has_more(false);
            return;
        }
        let Some(session_scope) = context.current_account_session_scope() else {
            state.set_invitation_users_message("登录状态已失效，请重新登录".into());
            return;
        };

        state.set_invitation_users_loading(true);
        state.set_invitation_users_message("".into());
        let api = AccountApi::new(pagination_backend.api.clone());
        let worker_scope = session_scope.clone();
        let requested_cursor = cursor.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(api.invitation_users_scoped(&cursor, &worker_scope));
        });
        poll_invitation_users(
            app.as_weak(),
            context.clone(),
            session_scope,
            requested_cursor,
            Rc::new(RefCell::new(Some(receiver))),
        );
    });
}

fn invited_user_view(user: &InvitedUserDto) -> InvitedUserView {
    InvitedUserView {
        id: user.id.clone().into(),
        email: user.email_masked.clone().into(),
        username: user.nickname.clone().into(),
        reward_detail: format!("{} 积分", user.reward_credits).into(),
        registered_at: user
            .registered_at
            .chars()
            .take(10)
            .collect::<String>()
            .into(),
    }
}

fn poll_invitation_users(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    requested_cursor: String,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<InvitationUserPage, ApiError>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !invitation_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(receiver) = slot.as_ref() else {
                return;
            };
            match receiver.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err(ApiError::LocalState {
                        message: "邀请明细加载任务意外中断".to_string(),
                    }))
                }
            }
        };
        let Some(result) = result else {
            poll_invitation_users(
                app_weak,
                context,
                session_scope,
                requested_cursor,
                receiver,
            );
            return;
        };
        if !invitation_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if state.get_invitation_users_next_cursor().as_str() != requested_cursor.as_str() {
            return;
        }
        state.set_invitation_users_loading(false);
        match result {
            Ok(page) => {
                let model = state.get_invitation_users();
                let mut items = (0..model.row_count())
                    .filter_map(|index| model.row_data(index))
                    .collect::<Vec<_>>();
                let mut ids = items
                    .iter()
                    .map(|item| item.id.to_string())
                    .collect::<BTreeSet<_>>();
                items.extend(page.items.iter().filter_map(|user| {
                    if ids.insert(user.id.clone()) {
                        Some(invited_user_view(user))
                    } else {
                        None
                    }
                }));
                state.set_invitation_users(ModelRc::new(VecModel::from(items)));
                let next_cursor = page.next_cursor.unwrap_or_default();
                state.set_invitation_users_has_more(!next_cursor.is_empty());
                state.set_invitation_users_next_cursor(next_cursor.into());
                state.set_invitation_users_message("".into());
            }
            Err(error) => {
                if (matches!(error, ApiError::AuthenticationRequired)
                    || error.is_terminal_session_error())
                    && terminal_auth_scope_matches_context(&context, &session_scope)
                {
                    sign_out_locally(
                        &app,
                        &context,
                        true,
                        Some(session_scope.auth_epoch),
                    );
                    return;
                }
                state.set_invitation_users_message(
                    format!("邀请明细加载失败：{}", error.user_message()).into(),
                );
            }
        }
    });
}

fn poll_invitation_submission(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    operation_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<InvitationSubmissionOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !invitation_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        if operation_epoch.get() != request_epoch {
            receiver.borrow_mut().take();
            return;
        }
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(receiver) = slot.as_ref() else {
                return;
            };
            match receiver.try_recv() {
                Ok(outcome) => {
                    slot.take();
                    Some(outcome)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(InvitationSubmissionOutcome::Failed(ApiError::LocalState {
                        message: "邀请码验证任务意外中断".to_string(),
                    }))
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_invitation_submission(
                app_weak,
                context,
                operation_epoch,
                request_epoch,
                session_scope,
                receiver,
            );
            return;
        };
        if !invitation_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_invitation_code_busy(false);
        match outcome {
            InvitationSubmissionOutcome::Submitted { message, dashboard } => {
                state.set_invitation_code("".into());
                state.set_invitation_code_success(true);
                state.set_invitation_code_submitted(true);
                state.set_invitation_code_status(
                    message
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| "邀请码填写成功".to_string())
                        .into(),
                );
                if let Ok(dashboard) = dashboard.as_ref() {
                    apply_invitation_dashboard(&app, dashboard);
                }
            }
            InvitationSubmissionOutcome::Failed(ApiError::Http { status: 404, .. }) => {
                state.set_invitation_code_status("邀请码无效或不存在".into());
            }
            InvitationSubmissionOutcome::Failed(error)
                if error.is_invitation_code_already_submitted() =>
            {
                state.set_invitation_code("".into());
                state.set_invitation_code_success(true);
                state.set_invitation_code_submitted(true);
                state.set_invitation_code_status(error.user_message().into());
            }
            InvitationSubmissionOutcome::Failed(error) => {
                state.set_invitation_code_status(
                    format!("邀请码验证失败：{}", error.user_message()).into(),
                );
            }
        }
    });
}

fn invitation_poll_is_current<T>(
    app_weak: &Weak<AppWindow>,
    context: &AppContext,
    session_scope: &SessionScope,
    receiver: &Rc<RefCell<Option<mpsc::Receiver<T>>>>,
) -> bool {
    match context.account_scope_disposition(session_scope) {
        AccountScopeDisposition::Current => true,
        AccountScopeDisposition::CapturedTerminal => {
            receiver.borrow_mut().take();
            if let Some(app) = app_weak.upgrade() {
                sign_out_locally(
                    &app,
                    context,
                    true,
                    Some(session_scope.auth_epoch),
                );
            }
            false
        }
        AccountScopeDisposition::Stale => {
            receiver.borrow_mut().take();
            false
        }
    }
}

fn advance_invitation_operation(operation_epoch: &Cell<u64>) -> u64 {
    let next = operation_epoch.get().wrapping_add(1);
    operation_epoch.set(next);
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invitation_operation_epoch_rejects_an_older_result() {
        let operation_epoch = Cell::new(3_u64);
        let request_epoch = advance_invitation_operation(&operation_epoch);
        assert_eq!(operation_epoch.get(), request_epoch);
        advance_invitation_operation(&operation_epoch);
        assert_ne!(operation_epoch.get(), request_epoch);
    }
}
