use super::*;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PasswordInputError {
    TooShort,
    TooLong,
    TooManyBytes,
    AllWhitespace,
    ConfirmationMismatch,
}

fn validate_new_password(password: &str, confirmation: &str) -> Result<(), PasswordInputError> {
    if password.len() > 512 {
        return Err(PasswordInputError::TooManyBytes);
    }
    let characters = password.chars().count();
    if characters < 12 {
        return Err(PasswordInputError::TooShort);
    }
    if characters > 128 {
        return Err(PasswordInputError::TooLong);
    }
    if password.chars().all(char::is_whitespace) {
        return Err(PasswordInputError::AllWhitespace);
    }
    if password != confirmation {
        return Err(PasswordInputError::ConfirmationMismatch);
    }
    Ok(())
}

fn valid_password_code(code: &str) -> bool {
    code.len() == 6 && code.chars().all(|value| value.is_ascii_digit())
}

fn normalized_password_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn reset_operation_matches(
    dialog_open: bool,
    current_epoch: u64,
    request_epoch: u64,
    current_email: &str,
    request_email: &str,
    current_code: &str,
    request_code: &str,
) -> bool {
    dialog_open
        && current_epoch == request_epoch
        && normalized_password_email(current_email) == request_email
        && current_code.trim() == request_code
}

fn management_operation_matches(
    dialog_open: bool,
    current_epoch: u64,
    request_epoch: u64,
    current_mode: &str,
    request_mode: &str,
) -> bool {
    dialog_open && current_epoch == request_epoch && current_mode.trim() == request_mode
}

fn advance_password_epoch(operation_epoch: &Cell<u64>) -> u64 {
    let next = operation_epoch.get().wrapping_add(1);
    operation_epoch.set(next);
    next
}

pub(super) fn clear_password_reset_state(state: &AppState) {
    state.set_password_reset_open(false);
    state.set_password_reset_email("".into());
    state.set_password_reset_code("".into());
    state.set_password_reset_new_password("".into());
    state.set_password_reset_confirm_password("".into());
    state.set_password_reset_code_busy(false);
    state.set_password_reset_busy(false);
    state.set_password_reset_countdown(0);
    state.set_password_reset_status("".into());
}

pub(super) fn clear_password_management_state(state: &AppState) {
    state.set_password_manage_open(false);
    state.set_password_verification_mode("email_code".into());
    state.set_password_current_password("".into());
    state.set_password_new_password("".into());
    state.set_password_confirm_password("".into());
    state.set_password_change_code("".into());
    state.set_password_change_code_busy(false);
    state.set_password_change_busy(false);
    state.set_password_change_countdown(0);
    state.set_password_change_status("".into());
}

pub(super) fn wire_password_callbacks(app: &AppWindow, _context: AppContext) {
    let state = app.global::<AppState>();
    let reset_epoch = Rc::new(Cell::new(0_u64));
    let management_epoch = Rc::new(Cell::new(0_u64));

    {
        let app_weak = app.as_weak();
        let reset_epoch = reset_epoch.clone();
        state.on_open_password_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            advance_password_epoch(&reset_epoch);
            let state = app.global::<AppState>();
            let email = normalized_password_email(state.get_auth_email().as_str());
            clear_password_reset_state(&state);
            state.set_password_reset_email(email.into());
            state.set_password_reset_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        let reset_epoch = reset_epoch.clone();
        state.on_close_password_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            advance_password_epoch(&reset_epoch);
            clear_password_reset_state(&app.global::<AppState>());
        });
    }

    {
        let app_weak = app.as_weak();
        let management_epoch = management_epoch.clone();
        state.on_open_password_management(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_email_bound() {
                state.set_generation_status("请先绑定并验证邮箱".into());
                return;
            }
            advance_password_epoch(&management_epoch);
            let verification_mode = if state.get_password_set() {
                "current_password"
            } else {
                "email_code"
            };
            clear_password_management_state(&state);
            state.set_password_verification_mode(verification_mode.into());
            state.set_password_manage_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        let management_epoch = management_epoch.clone();
        state.on_close_password_management(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            advance_password_epoch(&management_epoch);
            clear_password_management_state(&app.global::<AppState>());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_password_validation_preserves_unicode_and_enforces_every_structural_bound() {
        assert!(validate_new_password("  正确 horse battery  ", "  正确 horse battery  ").is_ok());
        assert_eq!(
            validate_new_password("short", "short"),
            Err(PasswordInputError::TooShort)
        );
        assert_eq!(
            validate_new_password("            ", "            "),
            Err(PasswordInputError::AllWhitespace)
        );
        assert_eq!(
            validate_new_password(&"界".repeat(129), &"界".repeat(129)),
            Err(PasswordInputError::TooLong)
        );
        assert_eq!(
            validate_new_password(&"🦀".repeat(129), &"🦀".repeat(129)),
            Err(PasswordInputError::TooManyBytes)
        );
        assert_eq!(
            validate_new_password("a sufficiently long passphrase", "different confirmation"),
            Err(PasswordInputError::ConfirmationMismatch)
        );
    }

    #[test]
    fn password_code_requires_exactly_six_ascii_digits() {
        assert!(valid_password_code("123456"));
        assert!(!valid_password_code("12345"));
        assert!(!valid_password_code("１２３４５６"));
        assert!(!valid_password_code("12345a"));
    }

    #[test]
    fn password_operation_snapshots_reject_closed_or_changed_flows() {
        assert!(reset_operation_matches(
            true,
            4,
            4,
            " Artist@Example.com ",
            "artist@example.com",
            " 123456 ",
            "123456",
        ));
        assert!(!reset_operation_matches(
            false,
            4,
            4,
            "artist@example.com",
            "artist@example.com",
            "123456",
            "123456",
        ));
        assert!(!reset_operation_matches(
            true,
            5,
            4,
            "artist@example.com",
            "artist@example.com",
            "123456",
            "123456",
        ));
        assert!(!management_operation_matches(
            true,
            9,
            9,
            "email_code",
            "current_password",
        ));
    }
}
