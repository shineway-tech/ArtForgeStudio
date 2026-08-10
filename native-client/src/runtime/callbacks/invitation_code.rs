use super::*;

pub(super) fn wire_invitation_code_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let state = app.global::<AppState>();
    let app_weak = app.as_weak();

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

        state.set_invitation_code_busy(true);
        state.set_invitation_code_status("正在验证邀请码…".into());
        let api = AccountApi::new(backend.api.clone());
        let weak = app.as_weak();
        std::thread::spawn(move || {
            let result = api.submit_invitation_code(&code);
            let _ = weak.upgrade_in_event_loop(move |app| {
                let state = app.global::<AppState>();
                state.set_invitation_code_busy(false);
                match result {
                    Ok(message) => {
                        state.set_invitation_code("".into());
                        state.set_invitation_code_success(true);
                        state.set_invitation_code_submitted(true);
                        state.set_invitation_code_status(
                            message
                                .filter(|value| !value.trim().is_empty())
                                .unwrap_or_else(|| "邀请码填写成功".to_string())
                                .into(),
                        );
                    }
                    Err(ApiError::Http { status: 404, .. }) => {
                        state.set_invitation_code_status("邀请码功能暂未开放".into());
                    }
                    Err(error) if error.is_invitation_code_already_submitted() => {
                        state.set_invitation_code("".into());
                        state.set_invitation_code_success(true);
                        state.set_invitation_code_submitted(true);
                        state.set_invitation_code_status(error.user_message().into());
                    }
                    Err(error) => {
                        state.set_invitation_code_status(
                            format!("邀请码验证失败：{}", error.user_message()).into(),
                        );
                    }
                }
            });
        });
    });
}
