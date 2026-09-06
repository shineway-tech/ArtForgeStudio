use super::*;

const PAYMENT_STATUS_UNAVAILABLE: &str = "暂时无法确认支付结果，请稍后查看订单状态";

struct PaymentStarted {
    order: OrderDetail,
    client_request_id: String,
    kind: PaymentOrderKind,
    presentation: PaymentPresentation,
    session_scope: SessionScope,
}

#[derive(Clone, Debug)]
struct PaymentPresentation {
    waiting_message: String,
    success_message: String,
    success_detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaymentOrderKind {
    Credit,
    Membership,
}

impl PaymentOrderKind {
    fn state_value(self) -> &'static str {
        match self {
            Self::Credit => "credit",
            Self::Membership => "membership",
        }
    }
}

impl PaymentPresentation {
    fn credit(credits: &str) -> Self {
        let credits = credits.trim();
        Self {
            waiting_message: "已在浏览器中打开支付宝，客户端正在等待积分充值结果".to_string(),
            success_message: if credits.is_empty() {
                "积分已到账".to_string()
            } else {
                format!("{credits} 积分已到账")
            },
            success_detail: "积分余额已更新".to_string(),
        }
    }

    fn membership(plan_name: &str) -> Self {
        let plan_name = plan_name.trim();
        Self {
            waiting_message: "已在浏览器中打开支付宝，客户端正在等待会员权益生效".to_string(),
            success_message: if plan_name.is_empty() {
                "会员权益已生效".to_string()
            } else if plan_name.ends_with("会员") {
                format!("{plan_name}已生效")
            } else {
                format!("{plan_name}会员已生效")
            },
            success_detail: "会员权益与有效期已更新".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaymentOrderPhase {
    PendingPayment,
    PaidFulfilling,
    Fulfilled,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaymentScopeDisposition {
    Current,
    CapturedTerminal,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingOrderGate {
    None,
    Recoverable,
    ManualReview,
}

fn payment_order_phase(order: &OrderDetail) -> PaymentOrderPhase {
    if order.status == "paid" && order.fulfillment_status == "fulfilled" {
        PaymentOrderPhase::Fulfilled
    } else if matches!(order.status.as_str(), "closed" | "expired") {
        PaymentOrderPhase::Closed
    } else if order.status == "paid" {
        PaymentOrderPhase::PaidFulfilling
    } else {
        PaymentOrderPhase::PendingPayment
    }
}

fn required_purchase_acceptances(
    app: &AppWindow,
) -> std::result::Result<Vec<AgreementAcceptance>, &'static str> {
    let state = app.global::<AppState>();
    let mut acceptances = Vec::new();
    if state.get_purchase_membership_required() {
        if !state.get_purchase_membership_accepted() {
            return Err("请先阅读并同意会员服务协议");
        }
        acceptances.push(AgreementAcceptance {
            agreement_type: "membership_service".to_string(),
            version: state.get_purchase_membership_version().to_string(),
        });
    }
    if state.get_purchase_credit_rules_required() {
        if !state.get_purchase_credit_rules_accepted() {
            return Err("请先阅读并同意积分使用规则");
        }
        acceptances.push(AgreementAcceptance {
            agreement_type: "credit_rules".to_string(),
            version: state.get_purchase_credit_rules_version().to_string(),
        });
    }
    Ok(acceptances)
}

pub(super) fn wire_payment_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let state = app.global::<AppState>();
    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let trusted_api_base = backend.api.base_url().clone();
        state.on_retry_payment_browser(move || {
            if let Some(app) = app_weak.upgrade() {
                reopen_payment_checkout(&app, &context, &trusted_api_base);
            }
        });
    }
    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_dismiss_payment(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            dismiss_payment_session(&app.global::<AppState>(), &context);
        });
    }
    {
        let app_weak = app.as_weak();
        state.on_confirm_payment_success(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            close_payment_success(&app.global::<AppState>());
        });
    }
    let app_weak = app.as_weak();
    state.on_recharge_credits(move |_pack_code| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        // TEMP(team-accounts): Task 10 supplies an admitted owned-group billing scope.
        app.global::<AppState>().set_credit_payment_message(
            ApiError::LocalState {
                message: "无法保存订单恢复记录，请稍后重试".to_owned(),
            }
            .user_message()
            .into(),
        );
    });

    let app_weak = app.as_weak();
    state.on_purchase_membership(move |_plan_code| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        // TEMP(team-accounts): Task 10 supplies an admitted owned-group billing scope.
        app.global::<AppState>().set_membership_payment_message(
            ApiError::LocalState {
                message: "无法保存订单恢复记录，请稍后重试".to_owned(),
            }
            .user_message()
            .into(),
        );
    });
}

fn poll_payment_started(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<PaymentStarted, ApiError>>>>>,
    client_request_id: String,
    kind: PaymentOrderKind,
    session_scope: SessionScope,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(value) => {
                    slot.take();
                    Some(value)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err(ApiError::Protocol {
                        message: "支付任务已中断".to_string(),
                        request_id: None,
                    }))
                }
            }
        };
        let Some(result) = result else {
            poll_payment_started(
                app_weak,
                context,
                backend,
                receiver,
                client_request_id,
                kind,
                session_scope,
            );
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match payment_scope_disposition(&context, &session_scope) {
            PaymentScopeDisposition::CapturedTerminal => {
                sign_out_locally(&app, &context, true, Some(session_scope.auth_epoch));
                return;
            }
            PaymentScopeDisposition::Stale => return,
            PaymentScopeDisposition::Current => {}
        }
        if !payment_session_is_current(&context, &client_request_id, &session_scope) {
            return;
        }
        let state = app.global::<AppState>();
        match result {
            Ok(started) => {
                continue_payment_order(&app, context, backend, started, true);
            }
            Err(error) => {
                let preserve_recovery = payment_error_preserves_order_recovery(&error);
                if !preserve_recovery {
                    let _ = remove_pending_order(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &client_request_id,
                    );
                }
                remove_recovering_order(&context, &session_scope, &client_request_id);
                clear_payment_session(&state, &context, Some((&client_request_id, &session_scope)));
                apply_agreements_from_payment_error(&app, &error);
                let message = if preserve_recovery {
                    format!("订单结果暂未确认，已保留恢复记录：{}", error.user_message())
                } else {
                    error.user_message()
                };
                state.set_payment_status_message(message.clone().into());
                match kind {
                    PaymentOrderKind::Credit => {
                        state.set_credit_payment_busy(false);
                        state.set_credit_payment_message(message.clone().into());
                    }
                    PaymentOrderKind::Membership => {
                        state.set_membership_payment_busy(false);
                        state.set_membership_payment_message(message.into());
                    }
                }
            }
        }
    });
}

fn poll_payment_order(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    order_id: String,
    client_request_id: String,
    kind: PaymentOrderKind,
    attempt: u32,
    session_scope: SessionScope,
) {
    slint::Timer::single_shot(Duration::from_secs(3), move || {
        let (sender, receiver) = mpsc::channel();
        let api = PaymentApi::new(backend.api.clone());
        let id = order_id.clone();
        let worker_scope = session_scope.clone();
        std::thread::spawn(move || {
            let _ = sender.send(api.sync_order_scoped(&id, &worker_scope));
        });
        poll_payment_sync_result(
            app_weak,
            context,
            backend,
            order_id,
            client_request_id,
            kind,
            attempt,
            session_scope,
            Rc::new(RefCell::new(Some(receiver))),
        );
    });
}

fn poll_payment_sync_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    order_id: String,
    client_request_id: String,
    kind: PaymentOrderKind,
    attempt: u32,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<OrderDetail, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(value) => {
                    slot.take();
                    Some(value)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err(ApiError::Protocol {
                        message: "支付状态同步已中断".to_string(),
                        request_id: None,
                    }))
                }
            }
        };
        let Some(result) = result else {
            poll_payment_sync_result(
                app_weak,
                context,
                backend,
                order_id,
                client_request_id,
                kind,
                attempt,
                session_scope,
                receiver,
            );
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match payment_scope_disposition(&context, &session_scope) {
            PaymentScopeDisposition::CapturedTerminal => {
                sign_out_locally(&app, &context, true, Some(session_scope.auth_epoch));
                return;
            }
            PaymentScopeDisposition::Stale => return,
            PaymentScopeDisposition::Current => {}
        }
        if !payment_session_is_current(&context, &client_request_id, &session_scope) {
            return;
        }
        let state = app.global::<AppState>();
        match result {
            Ok(order) if payment_order_phase(&order) == PaymentOrderPhase::Fulfilled => {
                finish_fulfilled_payment(&app, context, &client_request_id, kind, &session_scope);
            }
            Ok(order) if payment_order_phase(&order) == PaymentOrderPhase::Closed => {
                finish_closed_payment(&state, &context, &client_request_id, kind, &session_scope);
            }
            Ok(_) if attempt < 200 => poll_payment_order(
                app.as_weak(),
                context,
                backend,
                order_id,
                client_request_id,
                kind,
                attempt + 1,
                session_scope,
            ),
            Ok(_) => {
                finish_unavailable_payment(
                    &state,
                    &context,
                    &client_request_id,
                    kind,
                    &session_scope,
                );
            }
            Err(_) if attempt < 200 => {
                poll_payment_order(
                    app.as_weak(),
                    context,
                    backend,
                    order_id,
                    client_request_id,
                    kind,
                    attempt + 1,
                    session_scope,
                );
            }
            Err(_) => {
                finish_unavailable_payment(
                    &state,
                    &context,
                    &client_request_id,
                    kind,
                    &session_scope,
                );
            }
        }
    });
}

fn continue_payment_order(
    app: &AppWindow,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    started: PaymentStarted,
    launch_checkout: bool,
) {
    let session_scope = started.session_scope.clone();
    match payment_scope_disposition(&context, &session_scope) {
        PaymentScopeDisposition::CapturedTerminal => {
            sign_out_locally(app, &context, true, Some(session_scope.auth_epoch));
            return;
        }
        PaymentScopeDisposition::Stale => return,
        PaymentScopeDisposition::Current => {}
    }
    if context
        .active_payment
        .borrow()
        .as_ref()
        .is_some_and(|active| {
            active.client_request_id != started.client_request_id
                || active.session_scope != session_scope
        })
    {
        remove_recovering_order(&context, &session_scope, &started.client_request_id);
        return;
    }
    let state = app.global::<AppState>();
    apply_payment_presentation(&state, started.kind, &started.presentation);
    if context.active_payment.borrow().is_none() {
        *context.active_payment.borrow_mut() = Some(ActivePaymentSession {
            client_request_id: started.client_request_id.clone(),
            checkout_url: None,
            session_scope: session_scope.clone(),
        });
    }
    let phase = payment_order_phase(&started.order);
    if phase == PaymentOrderPhase::Fulfilled {
        finish_fulfilled_payment(
            app,
            context,
            &started.client_request_id,
            started.kind,
            &session_scope,
        );
        return;
    }
    if phase == PaymentOrderPhase::Closed {
        finish_closed_payment(
            &state,
            &context,
            &started.client_request_id,
            started.kind,
            &session_scope,
        );
        return;
    }

    let checkout_url = started
        .order
        .payment
        .as_ref()
        .and_then(|payment| payment.checkout_url.clone());
    if phase == PaymentOrderPhase::PendingPayment && checkout_url.is_none() {
        let message = "暂时无法获取支付宝支付地址，订单恢复记录已保留";
        remove_recovering_order(&context, &session_scope, &started.client_request_id);
        clear_payment_session(
            &state,
            &context,
            Some((&started.client_request_id, &session_scope)),
        );
        state.set_payment_status_message(message.into());
        match started.kind {
            PaymentOrderKind::Credit => {
                state.set_credit_payment_busy(false);
                state.set_credit_payment_message(message.into());
            }
            PaymentOrderKind::Membership => {
                state.set_membership_payment_busy(false);
                state.set_membership_payment_message(message.into());
            }
        }
        return;
    }

    let message = if phase == PaymentOrderPhase::PaidFulfilling {
        match started.kind {
            PaymentOrderKind::Credit => "付款已确认，正在等待权益到账...",
            PaymentOrderKind::Membership => "付款已确认，正在等待会员权益生效...",
        }
    } else if launch_checkout {
        "正在打开支付宝网站..."
    } else {
        "已恢复未完成订单，可重新打开支付宝继续支付"
    };

    *context.active_payment.borrow_mut() = Some(ActivePaymentSession {
        client_request_id: started.client_request_id.clone(),
        checkout_url: checkout_url.clone(),
        session_scope: session_scope.clone(),
    });
    state.set_payment_active(true);
    state.set_payment_dialog_open(true);
    state.set_payment_dialog_mode("waiting".into());
    state.set_payment_browser_ready(checkout_url.is_some());
    state.set_payment_status_message(message.into());

    if launch_checkout {
        if let Some(checkout_url) = checkout_url {
            let request_id = started.client_request_id.clone();
            let context_for_launch = context.clone();
            let scope_for_launch = session_scope.clone();
            let trusted_api_base = backend.api.base_url().clone();
            let app_weak = app.as_weak();
            slint::Timer::single_shot(Duration::from_millis(16), move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                if !payment_scope_matches_context(&context_for_launch, &scope_for_launch)
                    || !payment_session_is_current(
                        &context_for_launch,
                        &request_id,
                        &scope_for_launch,
                    )
                {
                    return;
                }
                let state = app.global::<AppState>();
                match open_payment_checkout(&checkout_url, &trusted_api_base) {
                    Ok(()) => state.set_payment_status_message(state.get_payment_waiting_message()),
                    Err(_) => state.set_payment_status_message(
                        "无法自动打开浏览器，请点击“重新打开支付宝”".into(),
                    ),
                }
            });
        }
    }

    match started.kind {
        PaymentOrderKind::Credit => {
            state.set_credit_payment_busy(true);
            state.set_credit_payment_message(message.into());
        }
        PaymentOrderKind::Membership => {
            state.set_membership_payment_busy(true);
            state.set_membership_payment_message(message.into());
        }
    }
    poll_payment_order(
        app.as_weak(),
        context,
        backend,
        started.order.id,
        started.client_request_id,
        started.kind,
        0,
        session_scope,
    );
}

fn begin_payment_session(
    state: &AppState,
    context: &AppContext,
    client_request_id: &str,
    kind: PaymentOrderKind,
    presentation: &PaymentPresentation,
    session_scope: SessionScope,
    message: &str,
) {
    *context.active_payment.borrow_mut() = Some(ActivePaymentSession {
        client_request_id: client_request_id.to_string(),
        checkout_url: None,
        session_scope,
    });
    state.set_payment_active(true);
    state.set_payment_dialog_open(true);
    state.set_payment_dialog_mode("waiting".into());
    state.set_payment_browser_ready(false);
    apply_payment_presentation(state, kind, presentation);
    state.set_payment_status_message(message.into());
}

fn apply_payment_presentation(
    state: &AppState,
    kind: PaymentOrderKind,
    presentation: &PaymentPresentation,
) {
    state.set_payment_kind(kind.state_value().into());
    state.set_payment_waiting_message(presentation.waiting_message.clone().into());
    state.set_payment_success_message(presentation.success_message.clone().into());
    state.set_payment_success_detail(presentation.success_detail.clone().into());
}

fn current_payment_session_scope(context: &AppContext) -> Option<SessionScope> {
    let owner_user_id = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone()
        .filter(|value| !value.trim().is_empty())?;
    let session_scope = context
        .backend
        .as_ref()?
        .api
        .session()
        .scope_for_user(&owner_user_id)?;
    account_snapshot_scope_is_current(context, &session_scope).then_some(session_scope)
}

fn ensure_payment_scope_active(
    backend: &BackendRuntime,
    session_scope: &SessionScope,
) -> std::result::Result<(), ApiError> {
    if backend.api.session().is_scope_current(session_scope) {
        Ok(())
    } else {
        Err(ApiError::AuthenticationRequired)
    }
}

fn payment_scope_matches_context(context: &AppContext, session_scope: &SessionScope) -> bool {
    let current_user_matches = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .as_deref()
        == Some(session_scope.owner_user_id.as_str());
    current_user_matches
        && context
            .backend
            .as_ref()
            .is_some_and(|backend| backend.api.session().is_scope_current(session_scope))
}

fn payment_scope_disposition(
    context: &AppContext,
    session_scope: &SessionScope,
) -> PaymentScopeDisposition {
    if payment_scope_matches_context(context, session_scope) {
        PaymentScopeDisposition::Current
    } else if terminal_auth_scope_matches_context(context, session_scope) {
        PaymentScopeDisposition::CapturedTerminal
    } else {
        PaymentScopeDisposition::Stale
    }
}

fn recovering_order_key(session_scope: &SessionScope, client_request_id: &str) -> String {
    format!(
        "{}:{}:{}",
        session_scope.owner_user_id, session_scope.auth_epoch, client_request_id
    )
}

fn remove_recovering_order(
    context: &AppContext,
    session_scope: &SessionScope,
    client_request_id: &str,
) {
    context
        .recovering_orders
        .borrow_mut()
        .remove(&recovering_order_key(session_scope, client_request_id));
}

fn payment_session_is_current(
    context: &AppContext,
    client_request_id: &str,
    session_scope: &SessionScope,
) -> bool {
    context
        .active_payment
        .borrow()
        .as_ref()
        .is_some_and(|session| {
            session.client_request_id == client_request_id
                && session.session_scope == *session_scope
        })
}

fn payment_error_preserves_order_recovery(error: &ApiError) -> bool {
    error.should_preserve_generation_recovery()
        || error.is_access_token_rejected()
        || matches!(
            error,
            ApiError::AuthenticationRequired
                | ApiError::Credential { .. }
                | ApiError::LocalState { .. }
        )
        || error.is_terminal_session_error()
        || matches!(
            error.code(),
            Some("request_in_progress" | "idempotency_key_conflict")
        )
}

fn dismiss_payment_session(state: &AppState, context: &AppContext) {
    let payment_kind = state.get_payment_kind();
    let active = context.active_payment.borrow().clone();
    if let Some(active) = active {
        remove_recovering_order(context, &active.session_scope, &active.client_request_id);
    }
    context.active_payment.borrow_mut().take();
    state.set_payment_active(false);
    state.set_payment_dialog_open(false);
    state.set_payment_browser_ready(false);
    state.set_credit_payment_busy(false);
    state.set_membership_payment_busy(false);
    if payment_kind.as_str() == PaymentOrderKind::Membership.state_value() {
        state.set_membership_payment_message(
            "支付窗口已隐藏，未完成订单仍会保留；再次购买时将先恢复该订单".into(),
        );
    } else {
        state.set_credit_payment_message(
            "支付窗口已隐藏，未完成订单仍会保留；再次充值时将先恢复该订单".into(),
        );
    }
}

fn recover_existing_payment_before_new_order(
    app: &AppWindow,
    context: AppContext,
    session_scope: &SessionScope,
    requested_kind: PaymentOrderKind,
) -> bool {
    let state = app.global::<AppState>();
    let records = match load_pending_orders_checked() {
        Ok(records) => records,
        Err(error) => {
            let message = format!(
                "订单恢复文件无法读取，原文件已保留；为避免重复扣款，请勿再次下单并联系客服：{error}"
            );
            state.set_payment_status_message(message.clone().into());
            set_payment_kind_status(&state, requested_kind, false, &message);
            return true;
        }
    };
    match pending_order_gate(&records, session_scope) {
        PendingOrderGate::None => false,
        PendingOrderGate::ManualReview => {
            let message =
                "检测到无法自动恢复的历史订单记录；为避免重复扣款，请勿再次下单并联系客服";
            state.set_payment_status_message(message.into());
            set_payment_kind_status(&state, requested_kind, false, message);
            true
        }
        PendingOrderGate::Recoverable => {
            let message = "检测到未完成订单，正在优先恢复；确认关闭或到账后才能新建订单";
            state.set_payment_status_message(message.into());
            set_payment_kind_status(&state, requested_kind, true, message);
            recover_pending_orders(app, context);
            true
        }
    }
}

fn pending_order_gate(
    records: &[PendingOrderRecord],
    session_scope: &SessionScope,
) -> PendingOrderGate {
    let relevant = records.iter().filter(|record| {
        record.owner_user_id == session_scope.owner_user_id
            || (record.owner_user_id.is_empty() && !record.order_id.trim().is_empty())
    });
    let mut found = false;
    for record in relevant {
        found = true;
        if !valid_pending_order(record) {
            return PendingOrderGate::ManualReview;
        }
    }
    if found {
        PendingOrderGate::Recoverable
    } else {
        PendingOrderGate::None
    }
}

fn clear_payment_session(
    state: &AppState,
    context: &AppContext,
    expected: Option<(&str, &SessionScope)>,
) -> bool {
    let mut active = context.active_payment.borrow_mut();
    let should_clear = active.as_ref().is_some_and(|session| {
        expected.is_none_or(|(client_request_id, scope)| {
            session.client_request_id == client_request_id && session.session_scope == *scope
        })
    });
    if !should_clear {
        return false;
    }
    active.take();
    state.set_payment_active(false);
    state.set_payment_browser_ready(false);
    true
}

fn reopen_payment_checkout(app: &AppWindow, context: &AppContext, trusted_api_base: &reqwest::Url) {
    let state = app.global::<AppState>();
    let active = context.active_payment.borrow().clone();
    let Some(active) = active else {
        state.set_payment_active(false);
        state.set_payment_dialog_open(false);
        state.set_payment_browser_ready(false);
        return;
    };
    if !payment_scope_matches_context(context, &active.session_scope) {
        clear_payment_account_state(app, context);
        return;
    }
    state.set_payment_dialog_open(true);
    let Some(checkout_url) = active.checkout_url else {
        state.set_payment_status_message("支付地址尚未准备好，请稍候".into());
        return;
    };
    match open_payment_checkout(&checkout_url, trusted_api_base) {
        Ok(()) => state.set_payment_status_message(state.get_payment_waiting_message()),
        Err(_) => {
            state.set_payment_status_message("无法打开系统浏览器，请检查系统设置后重试".into())
        }
    }
}

fn set_payment_kind_status(state: &AppState, kind: PaymentOrderKind, busy: bool, message: &str) {
    match kind {
        PaymentOrderKind::Credit => {
            state.set_credit_payment_busy(busy);
            state.set_credit_payment_message(message.into());
        }
        PaymentOrderKind::Membership => {
            state.set_membership_payment_busy(busy);
            state.set_membership_payment_message(message.into());
        }
    }
}

fn finish_fulfilled_payment(
    app: &AppWindow,
    context: AppContext,
    client_request_id: &str,
    kind: PaymentOrderKind,
    session_scope: &SessionScope,
) {
    if !payment_scope_matches_context(&context, session_scope) {
        return;
    }
    let _ = remove_pending_order(
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
        client_request_id,
    );
    remove_recovering_order(&context, session_scope, client_request_id);
    let state = app.global::<AppState>();
    if clear_payment_session(&state, &context, Some((client_request_id, session_scope))) {
        state.set_payment_dialog_mode("success".into());
        state.set_payment_dialog_open(true);
        state.set_payment_status_message("支付成功".into());
    }
    let message = match kind {
        PaymentOrderKind::Credit => "支付成功，积分已到账",
        PaymentOrderKind::Membership => {
            state.set_membership_open(false);
            "支付成功，会员权益已更新"
        }
    };
    set_payment_kind_status(&state, kind, false, message);
    refresh_backend_snapshot(app, context.clone());
    refresh_server_notifications(app, context);
}

fn close_payment_success(state: &AppState) {
    state.set_payment_dialog_open(false);
    state.set_payment_dialog_mode("waiting".into());
    state.set_payment_status_message("".into());
    state.set_payment_success_message("".into());
    state.set_payment_success_detail("".into());
}

fn finish_closed_payment(
    state: &AppState,
    context: &AppContext,
    client_request_id: &str,
    kind: PaymentOrderKind,
    session_scope: &SessionScope,
) {
    if !payment_scope_matches_context(context, session_scope) {
        return;
    }
    let _ = remove_pending_order(
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
        client_request_id,
    );
    remove_recovering_order(context, session_scope, client_request_id);
    if clear_payment_session(state, context, Some((client_request_id, session_scope))) {
        state.set_payment_status_message("订单已关闭或过期".into());
        set_payment_kind_status(state, kind, false, "订单已关闭或过期，请重新发起支付");
    }
}

fn finish_unavailable_payment(
    state: &AppState,
    context: &AppContext,
    client_request_id: &str,
    kind: PaymentOrderKind,
    session_scope: &SessionScope,
) {
    if !payment_scope_matches_context(context, session_scope) {
        return;
    }
    remove_recovering_order(context, session_scope, client_request_id);
    if clear_payment_session(state, context, Some((client_request_id, session_scope))) {
        state.set_payment_status_message(PAYMENT_STATUS_UNAVAILABLE.into());
        set_payment_kind_status(state, kind, false, PAYMENT_STATUS_UNAVAILABLE);
    }
}

pub(super) fn clear_payment_account_state(app: &AppWindow, context: &AppContext) {
    context.active_payment.borrow_mut().take();
    context.recovering_orders.borrow_mut().clear();
    let state = app.global::<AppState>();
    state.set_payment_active(false);
    state.set_payment_dialog_open(false);
    state.set_payment_dialog_mode("waiting".into());
    state.set_payment_browser_ready(false);
    state.set_payment_kind("".into());
    state.set_payment_status_message("".into());
    state.set_payment_waiting_message("".into());
    state.set_payment_success_message("".into());
    state.set_payment_success_detail("".into());
    state.set_credit_payment_busy(false);
    state.set_credit_payment_message("".into());
    state.set_membership_payment_busy(false);
    state.set_membership_payment_message("".into());
}

fn apply_agreements_from_payment_error(app: &AppWindow, error: &ApiError) {
    let ApiError::Http {
        code,
        details: Some(details),
        ..
    } = error
    else {
        return;
    };
    if code != "agreement_acceptance_required" {
        return;
    }
    let Some(agreements) = details.get("agreements").cloned() else {
        return;
    };
    if let Ok(items) = serde_json::from_value::<Vec<AgreementItem>>(agreements) {
        apply_agreements(app, &items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(status: &str, fulfillment_status: &str) -> OrderDetail {
        OrderDetail {
            id: "order-1".to_string(),
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".to_string(),
            status: status.to_string(),
            fulfillment_status: fulfillment_status.to_string(),
            payable_amount_cents: "100".to_string(),
            payment: None,
        }
    }

    fn pending(owner_user_id: &str, product_code: &str) -> PendingOrderRecord {
        PendingOrderRecord {
            schema_version: 2,
            kind: "credit".to_string(),
            client_request_id: "request-1".to_string(),
            owner_user_id: owner_user_id.to_string(),
            billing_account_group_id: "22222222-2222-4222-8222-222222222222".to_owned(),
            auth_epoch: 4,
            order_id: "order-1".to_string(),
            product_code: product_code.to_string(),
            upgrade_quote_id: String::new(),
            created_at: "2026-08-10T00:00:00+08:00".to_string(),
        }
    }

    #[test]
    fn paid_order_is_not_downgraded_while_fulfillment_retries() {
        assert_eq!(
            payment_order_phase(&order("paid", "retry_pending")),
            PaymentOrderPhase::PaidFulfilling
        );
        assert_eq!(
            payment_order_phase(&order("paid", "fulfilled")),
            PaymentOrderPhase::Fulfilled
        );
    }

    #[test]
    fn pending_expired_and_closed_orders_have_distinct_phases() {
        assert_eq!(
            payment_order_phase(&order("pending_payment", "pending")),
            PaymentOrderPhase::PendingPayment
        );
        assert_eq!(
            payment_order_phase(&order("expired", "pending")),
            PaymentOrderPhase::Closed
        );
        assert_eq!(
            payment_order_phase(&order("closed", "pending")),
            PaymentOrderPhase::Closed
        );
    }

    #[test]
    fn payment_presentations_keep_credit_and_membership_copy_distinct() {
        let credit = PaymentPresentation::credit("1000");
        assert_eq!(credit.success_message, "1000 积分已到账");
        assert_eq!(credit.success_detail, "积分余额已更新");
        assert!(credit.waiting_message.contains("积分充值"));

        let membership = PaymentPresentation::membership("专业版");
        assert_eq!(membership.success_message, "专业版会员已生效");
        assert_eq!(membership.success_detail, "会员权益与有效期已更新");
        assert!(membership.waiting_message.contains("会员权益"));

        let named_membership = PaymentPresentation::membership("年度会员");
        assert_eq!(named_membership.success_message, "年度会员已生效");
    }

    #[test]
    fn uncertain_payment_errors_preserve_order_recovery() {
        let errors = [
            ApiError::Network {
                message: "connection reset".to_string(),
                timeout: false,
            },
            ApiError::Protocol {
                message: "truncated response".to_string(),
                request_id: None,
            },
            ApiError::Http {
                status: 503,
                code: "service_unavailable".to_string(),
                message: "later".to_string(),
                request_id: None,
                details: None,
            },
            ApiError::Http {
                status: 401,
                code: "access_token_invalid".to_string(),
                message: "expired".to_string(),
                request_id: None,
                details: None,
            },
            ApiError::Http {
                status: 409,
                code: "idempotency_key_conflict".to_string(),
                message: "unknown outcome".to_string(),
                request_id: None,
                details: None,
            },
            ApiError::AuthenticationRequired,
            ApiError::LocalState {
                message: "disk unavailable".to_string(),
            },
        ];

        assert!(errors.iter().all(payment_error_preserves_order_recovery));
    }

    #[test]
    fn deterministic_payment_rejection_can_discard_uncreated_order_recovery() {
        let error = ApiError::Http {
            status: 400,
            code: "credit_pack_unavailable".to_string(),
            message: "removed".to_string(),
            request_id: None,
            details: None,
        };

        assert!(!payment_error_preserves_order_recovery(&error));
    }

    #[test]
    fn unfinished_owned_order_blocks_a_new_purchase_until_recovered() {
        let scope = SessionScope {
            owner_user_id: "user-a".to_string(),
            auth_epoch: 9,
        };
        assert_eq!(
            pending_order_gate(&[pending("user-a", "pack-1")], &scope),
            PendingOrderGate::Recoverable
        );
        assert_eq!(
            pending_order_gate(&[pending("user-b", "pack-1")], &scope),
            PendingOrderGate::None
        );
    }

    #[test]
    fn malformed_owned_order_fails_closed_instead_of_allowing_duplicate_purchase() {
        let scope = SessionScope {
            owner_user_id: "user-a".to_string(),
            auth_epoch: 9,
        };
        assert_eq!(
            pending_order_gate(&[pending("user-a", "")], &scope),
            PendingOrderGate::ManualReview
        );
    }
}

pub(super) fn recover_pending_orders(app: &AppWindow, _context: AppContext) {
    // TEMP(team-accounts): no replay before saved-payer admission.
    app.global::<AppState>()
        .set_credit_payment_message("订单恢复暂不可用，请稍后重试".into());
}

fn valid_pending_order(record: &PendingOrderRecord) -> bool {
    record.schema_version == 2
        && !record.client_request_id.trim().is_empty()
        && !record.product_code.trim().is_empty()
        && matches!(
            record.kind.as_str(),
            "credit" | "membership" | "membership_upgrade"
        )
}

fn pending_order_kind(record: &PendingOrderRecord) -> PaymentOrderKind {
    if record.kind == "credit" {
        PaymentOrderKind::Credit
    } else {
        PaymentOrderKind::Membership
    }
}

fn recover_pending_order_worker(
    _backend: Arc<BackendRuntime>,
    _record: PendingOrderRecord,
    _kind: PaymentOrderKind,
    _presentation: PaymentPresentation,
    _session_scope: SessionScope,
    _legacy_probe: bool,
) -> std::result::Result<PaymentStarted, ApiError> {
    // TEMP(team-accounts): persisted strings cannot authorize billing or legacy assignment.
    Err(ApiError::LocalState {
        message: "订单恢复暂不可用，请稍后重试".to_owned(),
    })
}

fn payment_presentation_for_product(
    state: &AppState,
    kind: PaymentOrderKind,
    product_code: &str,
) -> PaymentPresentation {
    match kind {
        PaymentOrderKind::Credit => {
            let credits = state
                .get_credit_packs()
                .iter()
                .find(|pack| pack.code.as_str() == product_code)
                .map(|pack| pack.credits.to_string())
                .unwrap_or_default();
            PaymentPresentation::credit(&credits)
        }
        PaymentOrderKind::Membership => {
            let name = state
                .get_membership_plans()
                .iter()
                .find(|plan| plan.code.as_str() == product_code)
                .map(|plan| plan.name.to_string())
                .unwrap_or_default();
            PaymentPresentation::membership(&name)
        }
    }
}

fn poll_recovered_order(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    client_request_id: String,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<PaymentStarted, ApiError>>>>>,
    kind: PaymentOrderKind,
    session_scope: SessionScope,
    legacy_probe: bool,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(value) => {
                    slot.take();
                    Some(value)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err(ApiError::Protocol {
                        message: "支付订单恢复任务已中断".to_string(),
                        request_id: None,
                    }))
                }
            }
        };
        let Some(result) = result else {
            poll_recovered_order(
                app_weak,
                context,
                backend,
                client_request_id,
                receiver,
                kind,
                session_scope,
                legacy_probe,
            );
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match payment_scope_disposition(&context, &session_scope) {
            PaymentScopeDisposition::CapturedTerminal => {
                sign_out_locally(&app, &context, true, Some(session_scope.auth_epoch));
                return;
            }
            PaymentScopeDisposition::Stale => return,
            PaymentScopeDisposition::Current => {}
        }
        match result {
            Ok(started) => {
                continue_payment_order(&app, context, backend, started, false);
            }
            Err(error) => {
                remove_recovering_order(&context, &session_scope, &client_request_id);
                if legacy_probe {
                    return;
                }
                if !payment_error_preserves_order_recovery(&error) {
                    let _ = remove_pending_order(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &client_request_id,
                    );
                }
                if context.active_payment.borrow().is_some() {
                    return;
                }
                let state = app.global::<AppState>();
                let message = format!("未完成订单暂时无法恢复：{}", error.user_message());
                match kind {
                    PaymentOrderKind::Credit => state.set_credit_payment_message(message.into()),
                    PaymentOrderKind::Membership => {
                        state.set_membership_payment_message(message.into())
                    }
                }
            }
        }
    });
}

fn require_order_recovery_update(updated: bool) -> Result<()> {
    if updated {
        Ok(())
    } else {
        Err(RecoveryError::IdentityChanged.into())
    }
}

fn create_upgrade_order_with_saved_quote(
    api: &MembershipApi,
    backend: &BackendRuntime,
    authority: &NamespaceStorageAuthority,
    identity: &RecoveryRecordIdentity,
    billing_scope: &BillingScope,
    plan_code: &str,
    request_id: &str,
) -> std::result::Result<OrderDetail, ApiError> {
    let quote = api.create_upgrade_quote_billing(plan_code, billing_scope)?;
    ensure_payment_scope_active(backend, &billing_scope.request.session)?;
    update_pending_order_quote_id_for_namespace(authority, identity, &quote.id)
        .and_then(require_order_recovery_update)
        .map_err(|error| ApiError::LocalState {
            message: format!("无法保存会员升级报价：{error}"),
        })?;
    api.create_upgrade_order_billing(&quote.id, request_id, billing_scope)
}

fn unfinished_namespace_order_blocks_new_purchase(
    app: &AppWindow,
    authority: &NamespaceStorageAuthority,
    requested_kind: PaymentOrderKind,
) -> bool {
    // Old-epoch rows still represent unfinished purchases. Do not claim, remove, or
    // replay them here: admission must stop before a fresh idempotency key exists.
    let message = match load_pending_orders_for_namespace(authority) {
        Ok(records) if records.is_empty() => return false,
        Ok(_) => "检测到未完成订单，记录已保留；为避免重复扣款，请勿再次下单并联系客服".to_owned(),
        Err(error) => format!(
            "订单恢复文件无法读取，原文件已保留；为避免重复扣款，请勿再次下单并联系客服：{error}"
        ),
    };
    let state = app.global::<AppState>();
    state.set_payment_status_message(message.clone().into());
    set_payment_kind_status(&state, requested_kind, false, &message);
    true
}

fn start_credit_order_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: &BillingScope,
    pack_code: String,
) {
    let billing_scope =
        match capture_billing_scope_for_submission(Some(&backend), &authority, billing_scope) {
            Ok(scope) => scope,
            Err(error) => {
                app.global::<AppState>()
                    .set_credit_payment_message(error.user_message().into());
                return;
            }
        };
    let session_scope = billing_scope.request.session.clone();

    let state = app.global::<AppState>();
    if !require_online_operation(&app, "充值积分") {
        return;
    }
    if state.get_payment_active() {
        state.set_credit_payment_message("当前订单正在等待付款，可继续前往支付宝".into());
        reopen_payment_checkout(&app, &context, backend.api.base_url());
        return;
    }
    if state.get_credit_payment_busy() {
        return;
    }
    if unfinished_namespace_order_blocks_new_purchase(app, &authority, PaymentOrderKind::Credit) {
        return;
    }
    let acceptances = match required_purchase_acceptances(&app) {
        Ok(value) => value,
        Err(message) => {
            state.set_credit_payment_message(message.into());
            return;
        }
    };
    let pack_code = pack_code.trim().to_string();
    if pack_code.is_empty() {
        state.set_credit_payment_message("请选择可用积分包".into());
        return;
    }
    let api = PaymentApi::new(backend.api.clone());
    let agreements_api = AuthApi::new(backend.api.clone());
    let request_id = Uuid::new_v4().simple().to_string();
    let payment_request_id = request_id.clone();
    let presentation = PaymentPresentation::credit(state.get_selected_credit_amount().as_str());
    begin_payment_session(
        &state,
        &context,
        &request_id,
        PaymentOrderKind::Credit,
        &presentation,
        session_scope.clone(),
        "正在创建积分充值订单...",
    );
    state.set_credit_payment_busy(true);
    state.set_credit_payment_message("正在创建积分充值订单...".into());
    let (sender, receiver) = mpsc::channel();
    let worker_scope = session_scope.clone();
    let worker_backend = backend.clone();
    std::thread::spawn(move || {
        let result = (|| {
            ensure_payment_scope_active(&worker_backend, &worker_scope)?;
            let record = PendingOrderRecord {
                schema_version: 2,
                kind: "credit".to_string(),
                client_request_id: request_id.clone(),
                owner_user_id: worker_scope.owner_user_id.clone(),
                billing_account_group_id: billing_scope.request.account_group_id.clone(),
                auth_epoch: worker_scope.auth_epoch,
                order_id: String::new(),
                product_code: pack_code.clone(),
                upgrade_quote_id: String::new(),
                created_at: Local::now().to_rfc3339(),
            };
            let recovery_identity = record.identity();
            upsert_pending_order_for_namespace(&authority, &billing_scope, record).map_err(
                |error| ApiError::LocalState {
                    message: format!("无法保存订单恢复记录：{error}"),
                },
            )?;
            agreements_api.accept_agreements_scoped(&acceptances, &worker_scope)?;
            ensure_payment_scope_active(&worker_backend, &worker_scope)?;
            let order = api.create_credit_order_billing(&pack_code, &request_id, &billing_scope)?;
            ensure_payment_scope_active(&worker_backend, &worker_scope)?;
            update_pending_order_id_for_namespace(&authority, &recovery_identity, &order.id)
                .and_then(require_order_recovery_update)
                .map_err(|error| ApiError::LocalState {
                    message: format!("无法保存服务端订单编号：{error}"),
                })?;
            Ok::<_, ApiError>(PaymentStarted {
                order,
                client_request_id: request_id,
                kind: PaymentOrderKind::Credit,
                presentation,
                session_scope: worker_scope,
            })
        })();
        let _ = sender.send(result);
    });
    poll_payment_started(
        app.as_weak(),
        context.clone(),
        backend.clone(),
        Rc::new(RefCell::new(Some(receiver))),
        payment_request_id,
        PaymentOrderKind::Credit,
        session_scope,
    );
}

fn start_membership_order_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: &BillingScope,
    plan_code: String,
) {
    let billing_scope =
        match capture_billing_scope_for_submission(Some(&backend), &authority, billing_scope) {
            Ok(scope) => scope,
            Err(error) => {
                app.global::<AppState>()
                    .set_membership_payment_message(error.user_message().into());
                return;
            }
        };
    let session_scope = billing_scope.request.session.clone();

    let state = app.global::<AppState>();
    if !require_online_operation(&app, "购买会员") || state.get_membership_payment_busy() {
        return;
    }
    if state.get_payment_active() {
        state.set_payment_dialog_open(true);
        state.set_membership_payment_message("请先完成当前支付订单".into());
        return;
    }
    if unfinished_namespace_order_blocks_new_purchase(app, &authority, PaymentOrderKind::Membership) {
        return;
    }
    let acceptances = match required_purchase_acceptances(&app) {
        Ok(value) => value,
        Err(message) => {
            state.set_membership_payment_message(message.into());
            return;
        }
    };
    let plan_code = plan_code.trim().to_string();
    let Some(target) = state
        .get_membership_plans()
        .iter()
        .find(|plan| plan.code.as_str() == plan_code)
    else {
        state.set_membership_payment_message("所选会员套餐已下线，请刷新后重试".into());
        return;
    };
    let is_upgrade =
        state.get_membership_tier_rank() > 0 && target.tier_rank > state.get_membership_tier_rank();
    let kind = if is_upgrade {
        "membership_upgrade"
    } else {
        "membership"
    }
    .to_string();
    let presentation = PaymentPresentation::membership(target.name.as_str());
    let request_id = Uuid::new_v4().simple().to_string();
    let payment_request_id = request_id.clone();
    state.set_membership_payment_busy(true);
    state.set_membership_payment_message(if is_upgrade {
        "正在获取服务端升级报价...".into()
    } else {
        "正在创建会员订单...".into()
    });
    begin_payment_session(
        &state,
        &context,
        &request_id,
        PaymentOrderKind::Membership,
        &presentation,
        session_scope.clone(),
        if is_upgrade {
            "正在获取服务端升级报价..."
        } else {
            "正在创建会员订单..."
        },
    );
    let api = MembershipApi::new(backend.api.clone());
    let agreements_api = AuthApi::new(backend.api.clone());
    let (sender, receiver) = mpsc::channel();
    let worker_scope = session_scope.clone();
    let worker_backend = backend.clone();
    std::thread::spawn(move || {
        let result = (|| {
            ensure_payment_scope_active(&worker_backend, &worker_scope)?;
            let record = PendingOrderRecord {
                schema_version: 2,
                kind,
                client_request_id: request_id.clone(),
                owner_user_id: worker_scope.owner_user_id.clone(),
                billing_account_group_id: billing_scope.request.account_group_id.clone(),
                auth_epoch: worker_scope.auth_epoch,
                order_id: String::new(),
                product_code: plan_code.clone(),
                upgrade_quote_id: String::new(),
                created_at: Local::now().to_rfc3339(),
            };
            let recovery_identity = record.identity();
            upsert_pending_order_for_namespace(&authority, &billing_scope, record).map_err(
                |error| ApiError::LocalState {
                    message: format!("无法保存订单恢复记录：{error}"),
                },
            )?;
            agreements_api.accept_agreements_scoped(&acceptances, &worker_scope)?;
            ensure_payment_scope_active(&worker_backend, &worker_scope)?;
            let order = if is_upgrade {
                create_upgrade_order_with_saved_quote(
                    &api,
                    &worker_backend,
                    &authority,
                    &recovery_identity,
                    &billing_scope,
                    &plan_code,
                    &request_id,
                )?
            } else {
                api.create_order_billing(&plan_code, &request_id, &billing_scope)?
            };
            ensure_payment_scope_active(&worker_backend, &worker_scope)?;
            update_pending_order_id_for_namespace(&authority, &recovery_identity, &order.id)
                .and_then(require_order_recovery_update)
                .map_err(|error| ApiError::LocalState {
                    message: format!("无法保存服务端订单编号：{error}"),
                })?;
            Ok::<_, ApiError>(PaymentStarted {
                order,
                client_request_id: request_id,
                kind: PaymentOrderKind::Membership,
                presentation,
                session_scope: worker_scope,
            })
        })();
        let _ = sender.send(result);
    });
    poll_payment_started(
        app.as_weak(),
        context.clone(),
        backend.clone(),
        Rc::new(RefCell::new(Some(receiver))),
        payment_request_id,
        PaymentOrderKind::Membership,
        session_scope,
    );
}

#[cfg(test)]
mod billing_capture_tests {
    use super::*;
    use backend_generation::billing_capture_test_support::*;
    fn assert_unfinished_order_blocks_actual_start(membership: bool, old_epoch: bool) {
        let app = app();
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let saved_epoch = if old_epoch {
            fixture.scope.request.session.auth_epoch.saturating_sub(1)
        } else {
            fixture.scope.request.session.auth_epoch
        };
        assert!(!old_epoch || saved_epoch != fixture.scope.request.session.auth_epoch);
        let saved_authority = NamespaceStorageAuthority::open(
            Arc::new(NamespaceFs::open_data_root(fixture.root.path()).unwrap()),
            &NamespaceLease {
                namespace: UserNamespace::new(fixture.root.path(), OWNER).unwrap(),
                auth_epoch: saved_epoch,
                namespace_epoch: 1,
            },
        )
        .unwrap();
        let mut saved_scope = fixture.scope.clone();
        saved_scope.request.session.auth_epoch = saved_epoch;
        let record = PendingOrderRecord {
            schema_version: 2,
            kind: "credit".into(),
            client_request_id: "unfinished-original-request".into(),
            owner_user_id: OWNER.into(),
            billing_account_group_id: PAYER.into(),
            auth_epoch: saved_epoch,
            order_id: "unfinished-original-order".into(),
            product_code: "original-pack".into(),
            upgrade_quote_id: String::new(),
            created_at: "fixture".into(),
        };
        upsert_pending_order_for_namespace(&saved_authority, &saved_scope, record).unwrap();
        let read_bytes = || {
            let key =
                ManagedFileKey::new(ManagedUserArea::Recovery, "pending-orders.json").unwrap();
            let mut file = fixture.authority.open_existing_regular(&key).unwrap();
            let mut bytes = Vec::new();
            fixture
                .authority
                .read_regular_to(&mut file, &mut bytes)
                .unwrap();
            bytes
        };
        let before = read_bytes();
        if membership {
            start_membership_order_with_billing_scope(
                &app,
                fixture.context.clone(),
                fixture.backend.clone(),
                fixture.authority.clone(),
                &fixture.scope,
                "fixture-plan".into(),
            );
        } else {
            start_credit_order_with_billing_scope(
                &app,
                fixture.context.clone(),
                fixture.backend.clone(),
                fixture.authority.clone(),
                &fixture.scope,
                "fixture-pack".into(),
            );
        }
        assert_no_request(&listener);
        assert_eq!(
            read_bytes(),
            before,
            "unfinished recovery bytes must remain unchanged"
        );
        assert!(
            fixture.context.active_payment.borrow().is_none(),
            "no new payment may be started"
        );
        let message = if membership {
            app.global::<AppState>().get_membership_payment_message()
        } else {
            app.global::<AppState>().get_credit_payment_message()
        };
        assert!(
            message.contains("未完成订单"),
            "expected local unfinished-order refusal: {message}"
        );
    }
    #[test]
    fn billing_capture_credit_start_blocks_current_epoch_unfinished_order() {
        assert_unfinished_order_blocks_actual_start(false, false);
    }
    #[test]
    fn billing_capture_credit_start_blocks_old_epoch_unfinished_order() {
        assert_unfinished_order_blocks_actual_start(false, true);
    }
    #[test]
    fn billing_capture_membership_start_blocks_current_epoch_unfinished_order() {
        assert_unfinished_order_blocks_actual_start(true, false);
    }
    #[test]
    fn billing_capture_membership_start_blocks_old_epoch_unfinished_order() {
        assert_unfinished_order_blocks_actual_start(true, true);
    }
    #[test]
    fn billing_capture_order_update_requires_an_exact_persisted_record() {
        let fixture = fixture("http://127.0.0.1:9/");
        let record = PendingOrderRecord {
            schema_version: 2,
            kind: "credit".into(),
            client_request_id: "order-fixture".into(),
            owner_user_id: OWNER.into(),
            billing_account_group_id: PAYER.into(),
            auth_epoch: fixture.scope.request.session.auth_epoch,
            order_id: String::new(),
            product_code: "fixture-pack".into(),
            upgrade_quote_id: String::new(),
            created_at: "fixture".into(),
        };
        let missing =
            update_pending_order_id_for_namespace(&fixture.authority, &record.identity(), "order")
                .unwrap();
        assert!(require_order_recovery_update(missing).is_err());
        upsert_pending_order_for_namespace(&fixture.authority, &fixture.scope, record.clone())
            .unwrap();
        let updated =
            update_pending_order_id_for_namespace(&fixture.authority, &record.identity(), "order")
                .unwrap();
        require_order_recovery_update(updated).unwrap();
        assert_eq!(
            load_pending_orders_for_namespace(&fixture.authority).unwrap()[0].order_id,
            "order"
        );
    }
    #[test]
    fn billing_capture_missing_quote_record_prevents_upgrade_order_dispatch() {
        use std::io::Write;
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let record = PendingOrderRecord {
            schema_version: 2,
            kind: "membership_upgrade".into(),
            client_request_id: "upgrade-fixture".into(),
            owner_user_id: OWNER.into(),
            billing_account_group_id: PAYER.into(),
            auth_epoch: fixture.scope.request.session.auth_epoch,
            order_id: String::new(),
            product_code: "fixture-plan".into(),
            upgrade_quote_id: String::new(),
            created_at: "fixture".into(),
        };
        upsert_pending_order_for_namespace(&fixture.authority, &fixture.scope, record.clone())
            .unwrap();
        let authority = fixture.authority.clone();
        let identity = record.identity();
        let transport = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("quote dispatch was not observed: {error}"),
                }
            };
            let request = read_request(&mut stream);
            assert!(request.starts_with("POST /v1/membership/upgrade-quotes "));
            assert!(request
                .to_lowercase()
                .contains(&format!("x-account-group-id: {PAYER}")));
            let persisted = load_pending_orders_for_namespace(&authority).unwrap();
            assert_eq!(persisted[0].identity(), identity);
            // A separate contender removes the exact persisted row while the real quote request
            // is paused at the transport boundary. The next billable order must never dispatch.
            assert!(remove_pending_order_for_namespace(&authority, &identity).unwrap());
            let body = serde_json::json!({
                "request_id": "quote-fixture", "error": null, "meta": null,
                "data": {"id": OTHER, "target_plan_code": "fixture-plan",
                    "payable_amount_cents": "100", "credit_delta": "10",
                    "expires_at": "2099-01-01T00:00:00Z"}
            })
            .to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            listener
        });
        let error = create_upgrade_order_with_saved_quote(
            &MembershipApi::new(fixture.backend.api.clone()),
            &fixture.backend,
            &fixture.authority,
            &record.identity(),
            &fixture.scope,
            "fixture-plan",
            &record.client_request_id,
        )
        .unwrap_err();
        assert!(matches!(error, ApiError::LocalState { .. }));
        assert_no_request(&transport.join().unwrap());
        assert!(load_pending_orders_for_namespace(&fixture.authority)
            .unwrap()
            .is_empty());
    }
    fn app() -> AppWindow {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_session_state("online".into());
        state.set_purchase_membership_required(false);
        state.set_purchase_credit_rules_required(false);
        state.set_membership_plans(ModelRc::new(VecModel::from(vec![MembershipPlanView {
            code: "fixture-plan".into(),
            name: "Fixture".into(),
            price: "100".into(),
            grant_credits: "10".into(),
            period_days: 30,
            tier_rank: 1,
        }])));
        app
    }
    #[test]
    fn billing_capture_credit_start_persists_before_real_dispatch() {
        let app = app();
        let (listener, url) = listener();
        let mut fixture = fixture(&url);
        let (release, transport) =
            capture(listener, fixture.authority.clone(), "pending-orders.json");
        start_credit_order_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.backend.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            "fixture-pack".into(),
        );
        fixture.scope.request.account_group_id = OTHER.into();
        fixture.scope.context_epoch += 1;
        release.send(()).unwrap();
        let observed = transport.join().unwrap();
        assert_capture(&observed, "orders");
        assert!(observed.request.starts_with("POST /v1/credits/orders "));
    }
    #[test]
    fn billing_capture_membership_start_persists_owned_group_before_real_dispatch() {
        let app = app();
        let (listener, url) = listener();
        let mut fixture = fixture(&url);
        let (release, transport) =
            capture(listener, fixture.authority.clone(), "pending-orders.json");
        start_membership_order_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.backend.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            "fixture-plan".into(),
        );
        fixture.scope.request.account_group_id = OTHER.into();
        fixture.scope.context_epoch += 1;
        release.send(()).unwrap();
        let observed = transport.join().unwrap();
        assert_capture(&observed, "orders");
        assert!(observed.request.contains("\"plan_code\":\"fixture-plan\""));
    }
    #[test]
    fn billing_capture_payment_storage_and_scope_failure_prevent_dispatch() {
        let app = app();
        let (listener, url) = listener();
        let fixture = fixture(&url);
        corrupt(&fixture.authority, "pending-orders.json");
        start_credit_order_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.backend.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            "fixture-pack".into(),
        );
        let mut wrong = fixture.scope.clone();
        wrong.request.session.auth_epoch += 1;
        start_membership_order_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.backend.clone(),
            fixture.authority.clone(),
            &wrong,
            "fixture-plan".into(),
        );
        assert_no_request(&listener);
        let key = ManagedFileKey::new(ManagedUserArea::Recovery, "pending-orders.json").unwrap();
        let mut file = fixture.authority.open_existing_regular(&key).unwrap();
        let mut bytes = Vec::new();
        fixture
            .authority
            .read_regular_to(&mut file, &mut bytes)
            .unwrap();
        assert_eq!(bytes, b"invalid-owned-fixture");
    }
}
