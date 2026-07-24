use super::*;

const PAYMENT_STATUS_UNAVAILABLE: &str = "暂时无法确认支付结果，请稍后查看订单状态";

struct PaymentStarted {
    order: OrderDetail,
    client_request_id: String,
    kind: PaymentOrderKind,
    presentation: PaymentPresentation,
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
    let Some(backend) = context.backend.clone() else { return; };
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
            let Some(app) = app_weak.upgrade() else { return; };
            dismiss_payment_session(&app.global::<AppState>(), &context);
        });
    }
    {
        let app_weak = app.as_weak();
        state.on_confirm_payment_success(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            close_payment_success(&app.global::<AppState>());
        });
    }
    let app_weak = app.as_weak();
    let credit_context = context.clone();
    state.on_recharge_credits(move |pack_code| {
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if !require_online_operation(&app, "充值积分") {
            return;
        }
        if state.get_payment_active() {
            state.set_credit_payment_message("当前订单正在等待付款，可继续前往支付宝".into());
            reopen_payment_checkout(&app, &credit_context, backend.api.base_url());
            return;
        }
        if state.get_credit_payment_busy() {
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
        let presentation = PaymentPresentation::credit(
            state.get_selected_credit_amount().as_str(),
        );
        begin_payment_session(
            &state,
            &credit_context,
            &request_id,
            PaymentOrderKind::Credit,
            &presentation,
            "正在创建积分充值订单...",
        );
        state.set_credit_payment_busy(true);
        state.set_credit_payment_message("正在创建积分充值订单...".into());
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                agreements_api.accept_agreements(&acceptances)?;
                upsert_pending_order(PendingOrderRecord {
                    schema_version: 1,
                    kind: "credit".to_string(),
                    client_request_id: request_id.clone(),
                    order_id: String::new(),
                    product_code: pack_code.clone(),
                    created_at: Local::now().to_rfc3339(),
                })
                .map_err(|error| ApiError::LocalState {
                    message: format!("无法保存订单恢复记录：{error}"),
                })?;
                let order = api.create_credit_order(&pack_code, &request_id)?;
                update_pending_order_id(&request_id, &order.id).map_err(|error| ApiError::LocalState {
                    message: format!("无法保存服务端订单编号：{error}"),
                })?;
                Ok::<_, ApiError>(PaymentStarted {
                    order,
                    client_request_id: request_id,
                    kind: PaymentOrderKind::Credit,
                    presentation,
                })
            })();
            let _ = sender.send(result);
        });
        poll_payment_started(
            app.as_weak(),
            credit_context.clone(),
            backend.clone(),
            Rc::new(RefCell::new(Some(receiver))),
            payment_request_id,
            PaymentOrderKind::Credit,
        );
    });

    let app_weak = app.as_weak();
    let backend = context.backend.clone().unwrap();
    state.on_purchase_membership(move |plan_code| {
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if !require_online_operation(&app, "购买会员") || state.get_membership_payment_busy() {
            return;
        }
        if state.get_payment_active() {
            state.set_payment_dialog_open(true);
            state.set_membership_payment_message("请先完成当前支付订单".into());
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
        let Some(target) = state.get_membership_plans().iter().find(|plan| plan.code.as_str() == plan_code) else {
            state.set_membership_payment_message("所选会员套餐已下线，请刷新后重试".into());
            return;
        };
        let is_upgrade = state.get_membership_tier_rank() > 0
            && target.tier_rank > state.get_membership_tier_rank();
        let kind = if is_upgrade { "membership_upgrade" } else { "membership" }.to_string();
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
            if is_upgrade {
                "正在获取服务端升级报价..."
            } else {
                "正在创建会员订单..."
            },
        );
        let api = MembershipApi::new(backend.api.clone());
        let agreements_api = AuthApi::new(backend.api.clone());
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                agreements_api.accept_agreements(&acceptances)?;
                upsert_pending_order(PendingOrderRecord {
                    schema_version: 1,
                    kind,
                    client_request_id: request_id.clone(),
                    order_id: String::new(),
                    product_code: plan_code.clone(),
                    created_at: Local::now().to_rfc3339(),
                })
                .map_err(|error| ApiError::LocalState {
                    message: format!("无法保存订单恢复记录：{error}"),
                })?;
                let order = if is_upgrade {
                    let quote = api.create_upgrade_quote(&plan_code)?;
                    api.create_upgrade_order(&quote.id, &request_id)?
                } else {
                    api.create_order(&plan_code, &request_id)?
                };
                update_pending_order_id(&request_id, &order.id).map_err(|error| ApiError::LocalState {
                    message: format!("无法保存服务端订单编号：{error}"),
                })?;
                Ok::<_, ApiError>(PaymentStarted {
                    order,
                    client_request_id: request_id,
                    kind: PaymentOrderKind::Membership,
                    presentation,
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
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else { return; };
            match rx.try_recv() {
                Ok(value) => { slot.take(); Some(value) }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err(ApiError::Protocol { message: "支付任务已中断".to_string(), request_id: None }))
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
            );
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        if !payment_session_is_current(&context, &client_request_id) {
            if let Ok(started) = result {
                let _ = remove_pending_order(&started.client_request_id);
                context
                    .recovering_orders
                    .borrow_mut()
                    .remove(&started.client_request_id);
            }
            return;
        }
        let state = app.global::<AppState>();
        match result {
            Ok(started) => {
                continue_payment_order(&app, context, backend, started, true);
            }
            Err(error) => {
                let failed_request_id = context
                    .active_payment
                    .borrow()
                    .as_ref()
                    .map(|session| session.client_request_id.clone());
                if let Some(client_request_id) = failed_request_id {
                    let _ = remove_pending_order(&client_request_id);
                    context.recovering_orders.borrow_mut().remove(&client_request_id);
                }
                clear_payment_session(&state, &context, None);
                apply_agreements_from_payment_error(&app, &error);
                state.set_payment_status_message(error.user_message().into());
                match kind {
                    PaymentOrderKind::Credit => {
                        state.set_credit_payment_busy(false);
                        state.set_credit_payment_message(error.user_message().into());
                    }
                    PaymentOrderKind::Membership => {
                        state.set_membership_payment_busy(false);
                        state.set_membership_payment_message(error.user_message().into());
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
) {
    slint::Timer::single_shot(Duration::from_secs(3), move || {
        let (sender, receiver) = mpsc::channel();
        let api = PaymentApi::new(backend.api.clone());
        let id = order_id.clone();
        std::thread::spawn(move || { let _ = sender.send(api.sync_order(&id)); });
        poll_payment_sync_result(app_weak, context, backend, order_id, client_request_id, kind, attempt, Rc::new(RefCell::new(Some(receiver))));
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
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<OrderDetail, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else { return; };
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
            poll_payment_sync_result(app_weak, context, backend, order_id, client_request_id, kind, attempt, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        if !payment_session_is_current(&context, &client_request_id) {
            return;
        }
        let state = app.global::<AppState>();
        match result {
            Ok(order) if payment_order_phase(&order) == PaymentOrderPhase::Fulfilled => {
                finish_fulfilled_payment(
                    &app,
                    context,
                    &client_request_id,
                    kind,
                );
            }
            Ok(order) if payment_order_phase(&order) == PaymentOrderPhase::Closed => {
                finish_closed_payment(&state, &context, &client_request_id, kind);
            }
            Ok(_) if attempt < 200 => poll_payment_order(app.as_weak(), context, backend, order_id, client_request_id, kind, attempt + 1),
            Ok(_) => {
                finish_unavailable_payment(&state, &context, &client_request_id, kind);
            }
            Err(_) if attempt < 200 => {
                poll_payment_order(app.as_weak(), context, backend, order_id, client_request_id, kind, attempt + 1);
            }
            Err(_) => {
                finish_unavailable_payment(&state, &context, &client_request_id, kind);
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
    let state = app.global::<AppState>();
    apply_payment_presentation(&state, started.kind, &started.presentation);
    if context.active_payment.borrow().is_none() {
        *context.active_payment.borrow_mut() = Some(ActivePaymentSession {
            client_request_id: started.client_request_id.clone(),
            checkout_url: None,
        });
    }
    let phase = payment_order_phase(&started.order);
    if phase == PaymentOrderPhase::Fulfilled {
        finish_fulfilled_payment(
            app,
            context,
            &started.client_request_id,
            started.kind,
        );
        return;
    }
    if phase == PaymentOrderPhase::Closed {
        finish_closed_payment(
            &state,
            &context,
            &started.client_request_id,
            started.kind,
        );
        return;
    }

    let checkout_url = started
        .order
        .payment
        .as_ref()
        .and_then(|payment| payment.checkout_url.clone());
    if phase == PaymentOrderPhase::PendingPayment && checkout_url.is_none() {
        let message = "暂时无法获取支付宝支付地址，请重新发起支付";
        let _ = remove_pending_order(&started.client_request_id);
        context.recovering_orders.borrow_mut().remove(&started.client_request_id);
        clear_payment_session(&state, &context, Some(&started.client_request_id));
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
            let trusted_api_base = backend.api.base_url().clone();
            let app_weak = app.as_weak();
            slint::Timer::single_shot(Duration::from_millis(16), move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                if !payment_session_is_current(&context_for_launch, &request_id) {
                    return;
                }
                let state = app.global::<AppState>();
                match open_payment_checkout(&checkout_url, &trusted_api_base) {
                    Ok(()) => state.set_payment_status_message(
                        state.get_payment_waiting_message(),
                    ),
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
    );
}

fn begin_payment_session(
    state: &AppState,
    context: &AppContext,
    client_request_id: &str,
    kind: PaymentOrderKind,
    presentation: &PaymentPresentation,
    message: &str,
) {
    *context.active_payment.borrow_mut() = Some(ActivePaymentSession {
        client_request_id: client_request_id.to_string(),
        checkout_url: None,
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

fn payment_session_is_current(context: &AppContext, client_request_id: &str) -> bool {
    context
        .active_payment
        .borrow()
        .as_ref()
        .is_some_and(|session| session.client_request_id == client_request_id)
}

fn dismiss_payment_session(state: &AppState, context: &AppContext) {
    let payment_kind = state.get_payment_kind();
    let client_request_id = context
        .active_payment
        .borrow()
        .as_ref()
        .map(|session| session.client_request_id.clone());
    if let Some(client_request_id) = client_request_id {
        let _ = remove_pending_order(&client_request_id);
        context
            .recovering_orders
            .borrow_mut()
            .remove(&client_request_id);
    }
    context.active_payment.borrow_mut().take();
    state.set_payment_active(false);
    state.set_payment_dialog_open(false);
    state.set_payment_browser_ready(false);
    state.set_credit_payment_busy(false);
    state.set_membership_payment_busy(false);
    if payment_kind.as_str() == PaymentOrderKind::Membership.state_value() {
        state.set_membership_payment_message("支付窗口已关闭，可重新发起会员购买".into());
    } else {
        state.set_credit_payment_message("支付窗口已关闭，可重新发起积分充值".into());
    }
}

fn clear_payment_session(
    state: &AppState,
    context: &AppContext,
    client_request_id: Option<&str>,
) -> bool {
    let mut active = context.active_payment.borrow_mut();
    let should_clear = active.as_ref().is_some_and(|session| {
        client_request_id.is_none_or(|value| session.client_request_id == value)
    });
    if !should_clear {
        return false;
    }
    active.take();
    state.set_payment_active(false);
    state.set_payment_browser_ready(false);
    true
}

fn reopen_payment_checkout(
    app: &AppWindow,
    context: &AppContext,
    trusted_api_base: &reqwest::Url,
) {
    let checkout_url = context
        .active_payment
        .borrow()
        .as_ref()
        .and_then(|session| session.checkout_url.clone());
    let state = app.global::<AppState>();
    state.set_payment_dialog_open(true);
    let Some(checkout_url) = checkout_url else {
        state.set_payment_status_message("支付地址尚未准备好，请稍候".into());
        return;
    };
    match open_payment_checkout(&checkout_url, trusted_api_base) {
        Ok(()) => state.set_payment_status_message(state.get_payment_waiting_message()),
        Err(_) => state.set_payment_status_message(
            "无法打开系统浏览器，请检查系统设置后重试".into(),
        ),
    }
}

fn set_payment_kind_status(
    state: &AppState,
    kind: PaymentOrderKind,
    busy: bool,
    message: &str,
) {
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
) {
    let _ = remove_pending_order(client_request_id);
    context.recovering_orders.borrow_mut().remove(client_request_id);
    let state = app.global::<AppState>();
    if clear_payment_session(&state, &context, Some(client_request_id)) {
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
) {
    let _ = remove_pending_order(client_request_id);
    context.recovering_orders.borrow_mut().remove(client_request_id);
    if clear_payment_session(state, context, Some(client_request_id)) {
        state.set_payment_status_message("订单已关闭或过期".into());
        set_payment_kind_status(
            state,
            kind,
            false,
            "订单已关闭或过期，请重新发起支付",
        );
    }
}

fn finish_unavailable_payment(
    state: &AppState,
    context: &AppContext,
    client_request_id: &str,
    kind: PaymentOrderKind,
) {
    context.recovering_orders.borrow_mut().remove(client_request_id);
    if clear_payment_session(state, context, Some(client_request_id)) {
        state.set_payment_status_message(PAYMENT_STATUS_UNAVAILABLE.into());
        set_payment_kind_status(state, kind, false, PAYMENT_STATUS_UNAVAILABLE);
    }
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
            status: status.to_string(),
            fulfillment_status: fulfillment_status.to_string(),
            payable_amount_cents: "100".to_string(),
            payment: None,
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
}

pub(super) fn recover_pending_orders(app: &AppWindow, context: AppContext) {
    if app.global::<AppState>().get_session_state().as_str() != "online" {
        return;
    }
    let Some(backend) = context.backend.clone() else { return; };
    for record in load_pending_orders() {
        if record.schema_version != 1 || record.client_request_id.is_empty() {
            continue;
        }
        if !context.recovering_orders.borrow_mut().insert(record.client_request_id.clone()) {
            continue;
        }
        let api = PaymentApi::new(backend.api.clone());
        let backend_for_worker = backend.clone();
        let request_id = record.client_request_id.clone();
        let request_id_for_worker = request_id.clone();
        let kind = if record.kind == "credit" {
            PaymentOrderKind::Credit
        } else {
            PaymentOrderKind::Membership
        };
        let presentation = payment_presentation_for_product(
            &app.global::<AppState>(),
            kind,
            &record.product_code,
        );
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = if record.order_id.is_empty() {
                match record.kind.as_str() {
                    "credit" => api.create_credit_order(&record.product_code, &record.client_request_id),
                    "membership" => MembershipApi::new(backend_for_worker.api.clone())
                        .create_order(&record.product_code, &record.client_request_id),
                    "membership_upgrade" => {
                        let membership = MembershipApi::new(backend_for_worker.api.clone());
                        membership.create_upgrade_quote(&record.product_code)
                            .and_then(|quote| membership.create_upgrade_order(&quote.id, &record.client_request_id))
                    }
                    _ => Err(ApiError::LocalState { message: "未知的待恢复订单类型".to_string() }),
                }
            } else {
                api.order(&record.order_id).or_else(|_| api.sync_order(&record.order_id))
            }.and_then(|order| {
                update_pending_order_id(&record.client_request_id, &order.id).map_err(|error| {
                    ApiError::LocalState { message: error.to_string() }
                })?;
                Ok(PaymentStarted {
                    order,
                    client_request_id: record.client_request_id,
                    kind,
                    presentation,
                })
            });
            let _ = sender.send(result);
        });
        poll_recovered_order(
            app.as_weak(),
            context.clone(),
            backend.clone(),
            request_id_for_worker,
            Rc::new(RefCell::new(Some(receiver))),
            kind,
        );
    }
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
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = receiver.borrow().as_ref().and_then(|rx| rx.try_recv().ok());
        let Some(result) = result else {
            poll_recovered_order(app_weak, context, backend, client_request_id, receiver, kind);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        match result {
            Ok(started) => {
                let state = app.global::<AppState>();
                match kind {
                    PaymentOrderKind::Credit => state.set_credit_payment_message("已恢复未完成支付订单".into()),
                    PaymentOrderKind::Membership => state.set_membership_payment_message("已恢复未完成支付订单".into()),
                }
                continue_payment_order(&app, context, backend, started, false);
            }
            Err(error) => {
                context.recovering_orders.borrow_mut().remove(&client_request_id);
                let state = app.global::<AppState>();
                let message = format!("未完成订单暂时无法恢复：{}", error.user_message());
                match kind {
                    PaymentOrderKind::Credit => state.set_credit_payment_message(message.into()),
                    PaymentOrderKind::Membership => state.set_membership_payment_message(message.into()),
                }
            }
        }
    });
}
