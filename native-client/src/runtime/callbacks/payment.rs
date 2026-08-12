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
    let credit_context = context.clone();
    state.on_recharge_credits(move |pack_code| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
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
        let Some(session_scope) = current_payment_session_scope(&credit_context) else {
            state.set_credit_payment_message("账号信息尚未同步，请稍后重试".into());
            return;
        };
        if recover_existing_payment_before_new_order(
            &app,
            credit_context.clone(),
            &session_scope,
            PaymentOrderKind::Credit,
        ) {
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
            &credit_context,
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
                upsert_pending_order(PendingOrderRecord {
                    schema_version: 1,
                    kind: "credit".to_string(),
                    client_request_id: request_id.clone(),
                    owner_user_id: worker_scope.owner_user_id.clone(),
                    auth_epoch: worker_scope.auth_epoch,
                    order_id: String::new(),
                    product_code: pack_code.clone(),
                    upgrade_quote_id: String::new(),
                    created_at: Local::now().to_rfc3339(),
                })
                .map_err(|error| ApiError::LocalState {
                    message: format!("无法保存订单恢复记录：{error}"),
                })?;
                agreements_api.accept_agreements_scoped(&acceptances, &worker_scope)?;
                ensure_payment_scope_active(&worker_backend, &worker_scope)?;
                let order =
                    api.create_credit_order_scoped(&pack_code, &request_id, &worker_scope)?;
                ensure_payment_scope_active(&worker_backend, &worker_scope)?;
                update_pending_order_id(
                    &worker_scope.owner_user_id,
                    worker_scope.auth_epoch,
                    &request_id,
                    &order.id,
                )
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
            credit_context.clone(),
            backend.clone(),
            Rc::new(RefCell::new(Some(receiver))),
            payment_request_id,
            PaymentOrderKind::Credit,
            session_scope,
        );
    });

    let app_weak = app.as_weak();
    let backend = context.backend.clone().unwrap();
    state.on_purchase_membership(move |plan_code| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !require_online_operation(&app, "购买会员") || state.get_membership_payment_busy() {
            return;
        }
        if state.get_payment_active() {
            state.set_payment_dialog_open(true);
            state.set_membership_payment_message("请先完成当前支付订单".into());
            return;
        }
        let Some(session_scope) = current_payment_session_scope(&context) else {
            state.set_membership_payment_message("账号信息尚未同步，请稍后重试".into());
            return;
        };
        if recover_existing_payment_before_new_order(
            &app,
            context.clone(),
            &session_scope,
            PaymentOrderKind::Membership,
        ) {
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
        let is_upgrade = state.get_membership_tier_rank() > 0
            && target.tier_rank > state.get_membership_tier_rank();
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
                upsert_pending_order(PendingOrderRecord {
                    schema_version: 1,
                    kind,
                    client_request_id: request_id.clone(),
                    owner_user_id: worker_scope.owner_user_id.clone(),
                    auth_epoch: worker_scope.auth_epoch,
                    order_id: String::new(),
                    product_code: plan_code.clone(),
                    upgrade_quote_id: String::new(),
                    created_at: Local::now().to_rfc3339(),
                })
                .map_err(|error| ApiError::LocalState {
                    message: format!("无法保存订单恢复记录：{error}"),
                })?;
                agreements_api.accept_agreements_scoped(&acceptances, &worker_scope)?;
                ensure_payment_scope_active(&worker_backend, &worker_scope)?;
                let order = if is_upgrade {
                    let quote = api.create_upgrade_quote_scoped(&plan_code, &worker_scope)?;
                    ensure_payment_scope_active(&worker_backend, &worker_scope)?;
                    update_pending_order_quote_id(
                        &worker_scope.owner_user_id,
                        worker_scope.auth_epoch,
                        &request_id,
                        &quote.id,
                    )
                    .map_err(|error| ApiError::LocalState {
                        message: format!("无法保存会员升级报价：{error}"),
                    })?;
                    api.create_upgrade_order_scoped(&quote.id, &request_id, &worker_scope)?
                } else {
                    api.create_order_scoped(&plan_code, &request_id, &worker_scope)?
                };
                ensure_payment_scope_active(&worker_backend, &worker_scope)?;
                update_pending_order_id(
                    &worker_scope.owner_user_id,
                    worker_scope.auth_epoch,
                    &request_id,
                    &order.id,
                )
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
                sign_out_locally(
                    &app,
                    &context,
                    true,
                    Some(session_scope.auth_epoch),
                );
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
                sign_out_locally(
                    &app,
                    &context,
                    true,
                    Some(session_scope.auth_epoch),
                );
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
            sign_out_locally(
                app,
                &context,
                true,
                Some(session_scope.auth_epoch),
            );
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
    };
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
            status: status.to_string(),
            fulfillment_status: fulfillment_status.to_string(),
            payable_amount_cents: "100".to_string(),
            payment: None,
        }
    }

    fn pending(owner_user_id: &str, product_code: &str) -> PendingOrderRecord {
        PendingOrderRecord {
            schema_version: 1,
            kind: "credit".to_string(),
            client_request_id: "request-1".to_string(),
            owner_user_id: owner_user_id.to_string(),
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

pub(super) fn recover_pending_orders(app: &AppWindow, context: AppContext) {
    if app.global::<AppState>().get_session_state().as_str() != "online" {
        return;
    }
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let Some(session_scope) = current_payment_session_scope(&context) else {
        return;
    };
    if context.active_payment.borrow().is_some() {
        return;
    }

    let mut records = match load_pending_orders_checked() {
        Ok(records) => records,
        Err(error) => {
            let message = format!(
                "订单恢复文件无法读取，原文件已保留；为避免重复扣款，请勿再次下单并联系客服：{error}"
            );
            let state = app.global::<AppState>();
            state.set_payment_status_message(message.clone().into());
            state.set_credit_payment_message(message.clone().into());
            state.set_membership_payment_message(message.into());
            return;
        }
    };
    records.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    let owned_index = records.iter().position(|record| {
        valid_pending_order(record) && record.owner_user_id == session_scope.owner_user_id
    });
    let legacy_index = records.iter().position(|record| {
        valid_pending_order(record)
            && record.owner_user_id.is_empty()
            && !record.order_id.trim().is_empty()
    });
    let Some(index) = owned_index.or(legacy_index) else {
        if records
            .iter()
            .any(|record| record.owner_user_id == session_scope.owner_user_id)
        {
            let message = "检测到无法自动恢复的历史订单记录；记录已保留，请联系客服处理";
            let state = app.global::<AppState>();
            state.set_payment_status_message(message.into());
            state.set_credit_payment_message(message.into());
            state.set_membership_payment_message(message.into());
        }
        // Legacy records without an order ID cannot be attributed safely. Do not issue any
        // network request and leave them untouched for an explicit migration/discard flow.
        return;
    };
    let mut record = records.swap_remove(index);
    let legacy_probe = record.owner_user_id.is_empty();
    if !payment_scope_matches_context(&context, &session_scope) {
        return;
    }
    if !legacy_probe && record.auth_epoch != session_scope.auth_epoch {
        let previous_epoch = record.auth_epoch;
        if claim_pending_order_epoch(
            &record.owner_user_id,
            previous_epoch,
            session_scope.auth_epoch,
            &record.client_request_id,
        )
        .is_err()
        {
            return;
        }
        record.auth_epoch = session_scope.auth_epoch;
    }
    if !payment_scope_matches_context(&context, &session_scope) {
        return;
    }

    let recovery_key = recovering_order_key(&session_scope, &record.client_request_id);
    if !context.recovering_orders.borrow_mut().insert(recovery_key) {
        return;
    }
    let kind = pending_order_kind(&record);
    let presentation =
        payment_presentation_for_product(&app.global::<AppState>(), kind, &record.product_code);
    let request_id = record.client_request_id.clone();
    let request_id_for_poll = request_id.clone();
    let worker_scope = session_scope.clone();
    let worker_backend = backend.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = recover_pending_order_worker(
            worker_backend,
            record,
            kind,
            presentation,
            worker_scope,
            legacy_probe,
        );
        let _ = sender.send(result);
    });
    poll_recovered_order(
        app.as_weak(),
        context,
        backend,
        request_id_for_poll,
        Rc::new(RefCell::new(Some(receiver))),
        kind,
        session_scope,
        legacy_probe,
    );
}

fn valid_pending_order(record: &PendingOrderRecord) -> bool {
    record.schema_version == 1
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
    backend: Arc<BackendRuntime>,
    mut record: PendingOrderRecord,
    kind: PaymentOrderKind,
    presentation: PaymentPresentation,
    session_scope: SessionScope,
    legacy_probe: bool,
) -> std::result::Result<PaymentStarted, ApiError> {
    let payment = PaymentApi::new(backend.api.clone());
    if legacy_probe {
        let order = payment.order_scoped(&record.order_id, &session_scope)?;
        ensure_payment_scope_active(&backend, &session_scope)?;
        if order.id != record.order_id {
            return Err(ApiError::Protocol {
                message: "服务端返回了不匹配的支付订单编号".to_string(),
                request_id: None,
            });
        }
        claim_legacy_pending_order(
            &session_scope.owner_user_id,
            session_scope.auth_epoch,
            &record.client_request_id,
            &record.order_id,
        )
        .map_err(|error| ApiError::LocalState {
            message: format!("无法认领旧版订单恢复记录：{error}"),
        })?;
        record.owner_user_id = session_scope.owner_user_id.clone();
        record.auth_epoch = session_scope.auth_epoch;
        return Ok(PaymentStarted {
            order,
            client_request_id: record.client_request_id,
            kind,
            presentation,
            session_scope,
        });
    }

    let order = if record.order_id.is_empty() {
        match record.kind.as_str() {
            "credit" => payment.create_credit_order_scoped(
                &record.product_code,
                &record.client_request_id,
                &session_scope,
            )?,
            "membership" => MembershipApi::new(backend.api.clone()).create_order_scoped(
                &record.product_code,
                &record.client_request_id,
                &session_scope,
            )?,
            "membership_upgrade" => {
                let membership = MembershipApi::new(backend.api.clone());
                let quote_id = if record.upgrade_quote_id.trim().is_empty() {
                    let quote = membership
                        .create_upgrade_quote_scoped(&record.product_code, &session_scope)?;
                    ensure_payment_scope_active(&backend, &session_scope)?;
                    update_pending_order_quote_id(
                        &record.owner_user_id,
                        record.auth_epoch,
                        &record.client_request_id,
                        &quote.id,
                    )
                    .map_err(|error| ApiError::LocalState {
                        message: format!("无法保存会员升级报价：{error}"),
                    })?;
                    record.upgrade_quote_id = quote.id;
                    record.upgrade_quote_id.clone()
                } else {
                    record.upgrade_quote_id.clone()
                };
                membership.create_upgrade_order_scoped(
                    &quote_id,
                    &record.client_request_id,
                    &session_scope,
                )?
            }
            _ => {
                return Err(ApiError::LocalState {
                    message: "未知的待恢复订单类型".to_string(),
                })
            }
        }
    } else {
        payment.order_scoped(&record.order_id, &session_scope)?
    };
    ensure_payment_scope_active(&backend, &session_scope)?;
    if !record.order_id.is_empty() && order.id != record.order_id {
        return Err(ApiError::Protocol {
            message: "服务端返回了不匹配的支付订单编号".to_string(),
            request_id: None,
        });
    }
    update_pending_order_id(
        &record.owner_user_id,
        record.auth_epoch,
        &record.client_request_id,
        &order.id,
    )
    .map_err(|error| ApiError::LocalState {
        message: format!("无法保存服务端订单编号：{error}"),
    })?;
    Ok(PaymentStarted {
        order,
        client_request_id: record.client_request_id,
        kind,
        presentation,
        session_scope,
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
                sign_out_locally(
                    &app,
                    &context,
                    true,
                    Some(session_scope.auth_epoch),
                );
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
