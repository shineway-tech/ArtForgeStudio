use super::*;

// Call only inside the caller's existing session / presentation completion guard.
pub(super) fn show_credit_rejection(state: &AppState, error: &ApiError) -> Option<String> {
    if !error.is_billing_rejection() {
        return None;
    }
    let english = state.get_language().as_str() == "en";
    let payer = error.billing_payer().unwrap_or("");
    let owned = state.get_account_groups().iter()
        .find(|group| !payer.is_empty() && group.group_id.as_str() == payer)
        .map(|group| group.owned);
    let (kind, message) = if error.code() == Some("membership_limit_exceeded") {
        ("member-limit", if english {
            "Your available monthly limit is insufficient. Please ask the team owner to adjust it."
        } else {
            "本月可用额度不足，请联系主账号调整额度。"
        })
    } else if owned == Some(true) {
        ("owner-credits", if english {
            "Insufficient credits. Please recharge and try again."
        } else {
            "积分不足，请充值后重试。"
        })
    } else if owned == Some(false) {
        ("team-credits", if english {
            "The team has insufficient credits. Please ask the team owner to recharge."
        } else {
            "团队积分不足，请联系主账号充值。"
        })
    } else {
        // Missing / retired payer metadata must not send the user to another team's wallet.
        ("unknown-credits", if english {
            "The paying account has insufficient credits. Switch to that account to resolve it."
        } else {
            "本次任务的付款账号积分不足，请切换到该账号后处理。"
        })
    };
    state.set_credit_insufficient_kind(kind.into());
    state.set_credit_insufficient_payer_id(payer.into());
    state.set_credit_insufficient_message(message.into());
    state.set_credit_insufficient_open(true);
    Some(message.to_owned())
}
