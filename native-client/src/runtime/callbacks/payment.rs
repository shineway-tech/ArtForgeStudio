use super::*;

const PAYMENT_STATUS_UNAVAILABLE: &str = "暂时无法确认支付结果，请稍后查看订单状态";

struct PaymentStarted {
    order: OrderDetail,
    client_request_id: String,
    kind: PaymentOrderKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaymentOrderKind {
    Credit,
    Membership,
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
        state.on_close_payment_window(move || {
            close_payment_window();
            if let Some(app) = app_weak.upgrade() {
                cancel_active_payment(&app, &context);
            }
        });
    }
    let app_weak = app.as_weak();
    let credit_context = context.clone();
    state.on_recharge_credits(move |pack_code| {
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if !require_online_operation(&app, "充值积分") || state.get_credit_payment_busy() {
            return;
        }
        let acceptances = match required_purchase_acceptances(&app) {
            Ok(value) => value,
            Err(message) => {
                state.set_credit_payment_message(message.into());
                return;
            }
        };
        state.set_credit_payment_busy(true);
        state.set_credit_payment_message("正在创建支付订单...".into());
        let pack_code = pack_code.trim().to_string();
        if pack_code.is_empty() {
            state.set_credit_payment_message("请选择可用积分包".into());
            state.set_credit_payment_busy(false);
            return;
        }
        let api = PaymentApi::new(backend.api.clone());
        let agreements_api = AuthApi::new(backend.api.clone());
        let request_id = Uuid::new_v4().simple().to_string();
        *credit_context.active_payment_request.borrow_mut() = Some(request_id.clone());
        credit_context
            .cancelled_payment_requests
            .borrow_mut()
            .remove(&request_id);
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
                })
            })();
            let _ = sender.send(result);
        });
        poll_payment_started(
            app.as_weak(),
            credit_context.clone(),
            backend.clone(),
            Rc::new(RefCell::new(Some(receiver))),
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
        let request_id = Uuid::new_v4().simple().to_string();
        state.set_membership_payment_busy(true);
        state.set_membership_payment_message(if is_upgrade {
            "正在获取服务端升级报价...".into()
        } else {
            "正在创建会员订单...".into()
        });
        let api = MembershipApi::new(backend.api.clone());
        let agreements_api = AuthApi::new(backend.api.clone());
        *context.active_payment_request.borrow_mut() = Some(request_id.clone());
        context
            .cancelled_payment_requests
            .borrow_mut()
            .remove(&request_id);
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
                })
            })();
            let _ = sender.send(result);
        });
        poll_payment_started(
            app.as_weak(),
            context.clone(),
            backend.clone(),
            Rc::new(RefCell::new(Some(receiver))),
            PaymentOrderKind::Membership,
        );
    });
}

fn poll_payment_started(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<PaymentStarted, ApiError>>>>>,
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
            poll_payment_started(app_weak, context, backend, receiver, kind);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        match result {
            Ok(started) => {
                continue_payment_order(&app, context, backend, started, true);
            }
            Err(error) => {
                context.active_payment_request.borrow_mut().take();
                apply_agreements_from_payment_error(&app, &error);
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
        if is_payment_cancelled(&context, &client_request_id) {
            finish_cancelled_payment(&context, &client_request_id);
            return;
        }
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
        let result = receiver.borrow().as_ref().and_then(|rx| rx.try_recv().ok());
        let Some(result) = result else {
            poll_payment_sync_result(app_weak, context, backend, order_id, client_request_id, kind, attempt, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        if is_payment_cancelled(&context, &client_request_id) {
            finish_cancelled_payment(&context, &client_request_id);
            return;
        }
        let state = app.global::<AppState>();
        match result {
            Ok(order) if payment_order_phase(&order) == PaymentOrderPhase::Fulfilled => {
                close_payment_window();
                let _ = remove_pending_order(&client_request_id);
                context.recovering_orders.borrow_mut().remove(&client_request_id);
                clear_active_payment(&context, &client_request_id);
                state.set_payment_qr_open(false);
                state.set_payment_qr_message("".into());
                match kind {
                    PaymentOrderKind::Credit => {
                        state.set_credit_payment_busy(false);
                        state.set_credit_payment_message("支付成功，积分已到账".into());
                    }
                    PaymentOrderKind::Membership => {
                        state.set_membership_payment_busy(false);
                        state.set_membership_payment_message("支付成功，会员权益已更新".into());
                    }
                }
                refresh_backend_snapshot(&app, context.clone());
                refresh_server_notifications(&app, context);
            }
            Ok(order) if payment_order_phase(&order) == PaymentOrderPhase::Closed => {
                close_payment_window();
                let _ = remove_pending_order(&client_request_id);
                context.recovering_orders.borrow_mut().remove(&client_request_id);
                clear_active_payment(&context, &client_request_id);
                state.set_payment_qr_open(false);
                state.set_payment_qr_message("订单已关闭或过期".into());
                match kind {
                    PaymentOrderKind::Credit => {
                        state.set_credit_payment_busy(false);
                        state.set_credit_payment_message("订单已关闭或过期，请重新发起支付".into());
                    }
                    PaymentOrderKind::Membership => {
                        state.set_membership_payment_busy(false);
                        state.set_membership_payment_message("订单已关闭或过期，请重新发起支付".into());
                    }
                }
            }
            Ok(_) if attempt < 200 => poll_payment_order(app.as_weak(), context, backend, order_id, client_request_id, kind, attempt + 1),
            Ok(_) => {
                state.set_payment_qr_message(PAYMENT_STATUS_UNAVAILABLE.into());
                match kind {
                    PaymentOrderKind::Credit => {
                        state.set_credit_payment_busy(false);
                        state.set_credit_payment_message(PAYMENT_STATUS_UNAVAILABLE.into());
                    }
                    PaymentOrderKind::Membership => {
                        state.set_membership_payment_busy(false);
                        state.set_membership_payment_message(PAYMENT_STATUS_UNAVAILABLE.into());
                    }
                }
                context.recovering_orders.borrow_mut().remove(&client_request_id);
                clear_active_payment(&context, &client_request_id);
            }
            Err(_) if attempt < 200 => {
                poll_payment_order(app.as_weak(), context, backend, order_id, client_request_id, kind, attempt + 1);
            }
            Err(_) => {
                state.set_payment_qr_message(PAYMENT_STATUS_UNAVAILABLE.into());
                match kind {
                    PaymentOrderKind::Credit => {
                        state.set_credit_payment_busy(false);
                        state.set_credit_payment_message(PAYMENT_STATUS_UNAVAILABLE.into());
                    }
                    PaymentOrderKind::Membership => {
                        state.set_membership_payment_busy(false);
                        state.set_membership_payment_message(PAYMENT_STATUS_UNAVAILABLE.into());
                    }
                }
                context.recovering_orders.borrow_mut().remove(&client_request_id);
                clear_active_payment(&context, &client_request_id);
            }
        }
    });
}

fn continue_payment_order(
    app: &AppWindow,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    started: PaymentStarted,
    open_checkout: bool,
) {
    let state = app.global::<AppState>();
    if is_payment_cancelled(&context, &started.client_request_id) {
        let _ = remove_pending_order(&started.client_request_id);
        finish_cancelled_payment(&context, &started.client_request_id);
        clear_active_payment(&context, &started.client_request_id);
        return;
    }
    if payment_order_phase(&started.order) == PaymentOrderPhase::Fulfilled {
        close_payment_window();
        let _ = remove_pending_order(&started.client_request_id);
        context.recovering_orders.borrow_mut().remove(&started.client_request_id);
        clear_active_payment(&context, &started.client_request_id);
        state.set_payment_qr_open(false);
        state.set_payment_qr_message("".into());
        match started.kind {
            PaymentOrderKind::Credit => {
                state.set_credit_payment_busy(false);
                state.set_credit_payment_message("支付成功，积分已到账".into());
            }
            PaymentOrderKind::Membership => {
                state.set_membership_payment_busy(false);
                state.set_membership_open(false);
                state.set_membership_payment_message("支付成功，会员权益已更新".into());
            }
        }
        refresh_backend_snapshot(app, context.clone());
        refresh_server_notifications(app, context);
        return;
    }
    if payment_order_phase(&started.order) == PaymentOrderPhase::Closed {
        close_payment_window();
        let _ = remove_pending_order(&started.client_request_id);
        context.recovering_orders.borrow_mut().remove(&started.client_request_id);
        clear_active_payment(&context, &started.client_request_id);
        state.set_payment_qr_open(false);
        state.set_payment_qr_message("订单已关闭或过期".into());
        match started.kind {
            PaymentOrderKind::Credit => {
                state.set_credit_payment_busy(false);
                state.set_credit_payment_message("订单已关闭或过期，请重新发起支付".into());
            }
            PaymentOrderKind::Membership => {
                state.set_membership_payment_busy(false);
                state.set_membership_payment_message("订单已关闭或过期，请重新发起支付".into());
            }
        }
        return;
    }
    let checkout_result = if open_checkout {
        started
            .order
            .payment
            .as_ref()
            .and_then(|payment| payment.checkout_url.as_deref())
            .map(|url| open_payment_window(app, url))
    } else {
        None
    };
    let checkout_opened = matches!(checkout_result, Some(Ok(())));
    let checkout_error = checkout_result
        .and_then(|result| result.err())
        .map(|_| "支付二维码加载失败，请关闭后重试。");
    let message = checkout_error.unwrap_or(if started.order.status == "paid" {
        match started.kind {
            PaymentOrderKind::Credit => "付款已确认，正在等待权益到账...",
            PaymentOrderKind::Membership => "付款已确认，正在等待会员权益生效...",
        }
    } else {
        "正在等待支付宝付款结果..."
    });
    if checkout_opened {
        state.set_payment_qr_message(message.into());
        state.set_payment_qr_open(true);
    }
    *context.active_payment_request.borrow_mut() = Some(started.client_request_id.clone());
    context
        .cancelled_payment_requests
        .borrow_mut()
        .remove(&started.client_request_id);
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

fn cancel_active_payment(app: &AppWindow, context: &AppContext) {
    let state = app.global::<AppState>();
    state.set_payment_qr_open(false);
    state.set_payment_qr_message("支付已取消，可重新发起支付".into());
    if state.get_credit_payment_busy() {
        state.set_credit_payment_busy(false);
        state.set_credit_payment_message("支付已取消，可重新发起支付".into());
    }
    if state.get_membership_payment_busy() {
        state.set_membership_payment_busy(false);
        state.set_membership_payment_message("支付已取消，可重新发起支付".into());
    }
    if let Some(client_request_id) = context.active_payment_request.borrow_mut().take() {
        context
            .cancelled_payment_requests
            .borrow_mut()
            .insert(client_request_id.clone());
        context.recovering_orders.borrow_mut().remove(&client_request_id);
        let _ = remove_pending_order(&client_request_id);
    }
}

fn is_payment_cancelled(context: &AppContext, client_request_id: &str) -> bool {
    context
        .cancelled_payment_requests
        .borrow()
        .contains(client_request_id)
}

fn finish_cancelled_payment(context: &AppContext, client_request_id: &str) {
    context
        .cancelled_payment_requests
        .borrow_mut()
        .remove(client_request_id);
    context.recovering_orders.borrow_mut().remove(client_request_id);
}

fn clear_active_payment(context: &AppContext, client_request_id: &str) {
    let mut active = context.active_payment_request.borrow_mut();
    if active.as_deref() == Some(client_request_id) {
        active.take();
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
                continue_payment_order(&app, context, backend, started, true);
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
