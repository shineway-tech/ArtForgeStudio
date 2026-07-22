use super::*;

pub(super) fn wire_email_binding_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else { return; };
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        state.on_request_email_binding_code(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            if state.get_email_bind_code_busy()
                || state.get_email_bind_busy()
                || state.get_email_bind_countdown() > 0
            {
                return;
            }
            let email = state.get_email_bind_email().trim().to_ascii_lowercase();
            if !valid_binding_email(&email) {
                state.set_email_bind_status("请输入正确的邮箱地址".into());
                return;
            }
            state.set_email_bind_code_busy(true);
            state.set_email_bind_status("正在发送验证码...".into());
            let api = AccountApi::new(backend.api.clone());
            let weak = app.as_weak();
            std::thread::spawn(move || {
                let result = api.request_email_binding_code(&email);
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let state = app.global::<AppState>();
                    state.set_email_bind_code_busy(false);
                    match result {
                        Ok(response) => {
                            let seconds = response.resend_after_seconds.min(i32::MAX as u64) as i32;
                            state.set_email_bind_countdown(seconds);
                            state.set_email_bind_status(format!(
                                "验证码已发送至 {}，{} 秒内有效",
                                response.email_masked,
                                response.expires_in_seconds,
                            ).into());
                            start_email_binding_countdown(app.as_weak());
                        }
                        Err(error) => state.set_email_bind_status(error.user_message().into()),
                    }
                });
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        state.on_bind_email(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            if state.get_email_bind_busy() || state.get_email_bind_code_busy() {
                return;
            }
            let email = state.get_email_bind_email().trim().to_ascii_lowercase();
            let code = state.get_email_bind_code().trim().to_string();
            if !valid_binding_email(&email) {
                state.set_email_bind_status("请输入正确的邮箱地址".into());
                return;
            }
            if code.len() != 6 || !code.chars().all(|value| value.is_ascii_digit()) {
                state.set_email_bind_status("请输入 6 位数字验证码".into());
                return;
            }
            state.set_email_bind_busy(true);
            state.set_email_bind_status("正在绑定邮箱...".into());
            let api = AccountApi::new(backend.api.clone());
            let weak = app.as_weak();
            std::thread::spawn(move || {
                let result = api.bind_email(&email, &code);
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let state = app.global::<AppState>();
                    state.set_email_bind_busy(false);
                    match result {
                        Ok(response) if response.bound => {
                            state.set_email_bound(true);
                            state.set_email_mask(response.email_masked.into());
                            state.set_email_bind_open(false);
                            state.set_email_bind_email("".into());
                            state.set_email_bind_code("".into());
                            state.set_email_bind_countdown(0);
                            state.set_email_bind_status("".into());
                            state.set_generation_status("邮箱绑定成功".into());
                            save_user_profile(&app);
                        }
                        Ok(_) => state.set_email_bind_status("邮箱绑定未完成，请重试".into()),
                        Err(error) => state.set_email_bind_status(error.user_message().into()),
                    }
                });
            });
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_close_email_binding(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            state.set_email_bind_open(false);
            state.set_email_bind_email("".into());
            state.set_email_bind_code("".into());
            state.set_email_bind_code_busy(false);
            state.set_email_bind_busy(false);
            state.set_email_bind_countdown(0);
            state.set_email_bind_status("".into());
        });
    }
}

fn start_email_binding_countdown(app_weak: Weak<AppWindow>) {
    slint::Timer::single_shot(Duration::from_secs(1), move || {
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if !state.get_email_bind_open() { return; }
        let remaining = (state.get_email_bind_countdown() - 1).max(0);
        state.set_email_bind_countdown(remaining);
        if remaining > 0 {
            start_email_binding_countdown(app.as_weak());
        }
    });
}

fn valid_binding_email(email: &str) -> bool {
    let mut parts = email.split('@');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(local), Some(domain), None)
            if !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
    )
}
