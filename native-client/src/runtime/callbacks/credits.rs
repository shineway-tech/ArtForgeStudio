use super::*;

enum CreditRedemptionOutcome {
    Redeemed(CreditRedemptionResult),
    Failed(ApiError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreditRedemptionSettlement {
    Succeeded,
    AmbiguousFailure,
    DefinitiveFailure,
}

fn credit_redemption_request_id(
    pending_by_owner: &mut BTreeMap<String, PendingCreditRedemption>,
    owner_user_id: &str,
    code: &str,
    create_request_id: impl FnOnce() -> String,
) -> String {
    if let Some(pending) = pending_by_owner.get(owner_user_id) {
        if pending.code == code {
            return pending.client_request_id.clone();
        }
    }

    let client_request_id = create_request_id();
    pending_by_owner.insert(
        owner_user_id.to_string(),
        PendingCreditRedemption {
            code: code.to_string(),
            client_request_id: client_request_id.clone(),
        },
    );
    client_request_id
}

fn settle_credit_redemption(
    pending_by_owner: &mut BTreeMap<String, PendingCreditRedemption>,
    owner_user_id: &str,
    code: &str,
    client_request_id: &str,
    settlement: CreditRedemptionSettlement,
) {
    let matches_completed_request = pending_by_owner.get(owner_user_id).is_some_and(|pending| {
        pending.code == code && pending.client_request_id == client_request_id
    });
    if settlement != CreditRedemptionSettlement::AmbiguousFailure && matches_completed_request {
        pending_by_owner.remove(owner_user_id);
    }
}

fn canonical_credit_account_version(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(value.trim_start_matches('0')).map(|value| if value.is_empty() { "0" } else { value })
}

fn credit_account_version_is_fresh(
    current: Option<&str>,
    candidate: &str,
    request_is_current: bool,
) -> bool {
    if !request_is_current {
        return false;
    }
    if candidate.trim().is_empty() {
        // Older servers did not return a credit-account version. The shared request epoch is the
        // ordering guarantee for that compatibility path, so a latest response may still apply
        // even after this client has previously observed a versioned response.
        return true;
    }
    let Some(candidate) = canonical_credit_account_version(candidate) else {
        return false;
    };
    let Some(current) = current.and_then(canonical_credit_account_version) else {
        return true;
    };
    candidate.len() > current.len() || (candidate.len() == current.len() && candidate >= current)
}

pub(super) fn apply_credit_account_balance_if_fresh(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    available: &str,
    reserved: &str,
    version: &str,
    credit_sync_epoch: u64,
) -> bool {
    let mut store = store.borrow_mut();
    let request_is_current = credit_sync_epoch_is_current(&store, credit_sync_epoch);
    if !credit_account_version_is_fresh(
        store.credit_account_version.as_deref(),
        version,
        request_is_current,
    ) {
        return false;
    }
    store.credit_account_version =
        canonical_credit_account_version(version).map(ToString::to_string);
    drop(store);

    let state = app.global::<AppState>();
    state.set_credit_balance(available.into());
    state.set_credit_reserved(reserved.into());
    true
}

pub(super) fn invalidate_credit_account_view(store: &Rc<RefCell<Store>>) {
    let mut store = store.borrow_mut();
    store.credit_account_version = None;
    store.credit_account_refresh_epoch = store.credit_account_refresh_epoch.wrapping_add(1);
}

fn begin_credit_account_refresh(store: &mut Store) -> u64 {
    store.credit_account_refresh_epoch = store.credit_account_refresh_epoch.wrapping_add(1);
    store.credit_account_refresh_epoch
}

fn credit_account_refresh_is_current(store: &Store, request_epoch: u64) -> bool {
    store.credit_account_refresh_epoch == request_epoch
}

#[derive(Clone, Default)]
pub(super) struct CreditLedgerPagination {
    page_cursors: Vec<Option<String>>,
    page_index: usize,
    next_cursor: Option<String>,
    request_epoch: u64,
}

impl CreditLedgerPagination {
    fn reset(&mut self, next_cursor: Option<String>) {
        self.request_epoch = self.request_epoch.wrapping_add(1);
        self.page_cursors = vec![None];
        self.page_index = 0;
        self.next_cursor = next_cursor;
    }

    fn begin_request(&mut self) -> u64 {
        self.request_epoch = self.request_epoch.wrapping_add(1);
        self.request_epoch
    }

    fn request_is_current(&self, request_epoch: u64) -> bool {
        self.request_epoch == request_epoch
    }

    fn page_number(&self) -> usize {
        self.page_index + 1
    }

    fn previous_target(&self) -> Option<(usize, Option<String>)> {
        self.page_index.checked_sub(1).map(|target_index| {
            let cursor = self.page_cursors.get(target_index).cloned().flatten();
            (target_index, cursor)
        })
    }

    fn next_target(&self) -> Option<(usize, Option<String>)> {
        self.next_cursor
            .clone()
            .map(|cursor| (self.page_index + 1, Some(cursor)))
    }

    fn apply_page(
        &mut self,
        target_index: usize,
        start_cursor: Option<String>,
        next_cursor: Option<String>,
    ) {
        self.page_cursors.truncate(target_index + 1);
        if self.page_cursors.len() <= target_index {
            self.page_cursors.resize(target_index + 1, None);
        }
        self.page_cursors[target_index] = start_cursor;
        self.page_index = target_index;
        self.next_cursor = next_cursor;
    }

    fn has_previous(&self) -> bool {
        self.page_index > 0
    }

    fn has_next(&self) -> bool {
        self.next_cursor.is_some()
    }
}

pub(super) fn wire_credit_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_redeem_credits(move |submitted_code| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_credit_redemption_busy() {
                return;
            }

            state.set_credit_redemption_success(false);
            let code = match validate_credit_redemption_code(submitted_code.as_str()) {
                Ok(code) => code,
                Err(message) => {
                    state.set_credit_redemption_message(
                        local_redemption_validation_message(message, state.get_en()).into(),
                    );
                    return;
                }
            };
            state.set_credit_redemption_code(code.clone().into());

            let Some(session_scope) = context.current_account_session_scope() else {
                state.set_credit_redemption_message(if state.get_en() {
                    "Your session has expired. Please sign in again.".into()
                } else {
                    "登录状态已失效，请重新登录".into()
                });
                return;
            };

            let client_request_id = {
                let mut store = context.store.borrow_mut();
                credit_redemption_request_id(
                    &mut store.pending_credit_redemptions_by_owner,
                    &session_scope.owner_user_id,
                    &code,
                    || Uuid::new_v4().to_string(),
                )
            };
            state.set_credit_redemption_busy(true);
            state.set_credit_redemption_message(if state.get_en() {
                "Redeeming...".into()
            } else {
                "正在兑换…".into()
            });

            let api = AccountApi::new(backend.api.clone());
            let submitted_code = code.clone();
            let submitted_request_id = client_request_id.clone();
            let worker_scope = session_scope.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let outcome =
                    match api.redeem_credit_code_scoped(&code, &client_request_id, &worker_scope) {
                        Ok(result) => CreditRedemptionOutcome::Redeemed(result),
                        Err(error) => CreditRedemptionOutcome::Failed(error),
                    };
                let _ = sender.send(outcome);
            });
            poll_credit_redemption(
                app.as_weak(),
                context.clone(),
                session_scope,
                submitted_code,
                submitted_request_id,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = context.store.clone();
        let context = context.clone();
        state.on_credit_ledger_previous_page(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_credit_ledger_loading() {
                return;
            }
            let target = store.borrow().credit_ledger_pagination.previous_target();
            if let Some((target_index, cursor)) = target {
                request_credit_ledger_page(
                    &app,
                    store.clone(),
                    context.clone(),
                    target_index,
                    cursor,
                    None,
                );
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = context.store.clone();
        let context = context.clone();
        state.on_credit_ledger_next_page(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_credit_ledger_loading() {
                return;
            }
            let target = store.borrow().credit_ledger_pagination.next_target();
            if let Some((target_index, cursor)) = target {
                request_credit_ledger_page(
                    &app,
                    store.clone(),
                    context.clone(),
                    target_index,
                    cursor,
                    None,
                );
            }
        });
    }
}

fn poll_credit_redemption(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    submitted_code: String,
    submitted_request_id: String,
    receiver: Rc<RefCell<Option<mpsc::Receiver<CreditRedemptionOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !redemption_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &submitted_code,
            &submitted_request_id,
            &receiver,
        ) {
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
                    Some(CreditRedemptionOutcome::Failed(ApiError::LocalState {
                        message: "兑换任务意外中断".to_string(),
                    }))
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_credit_redemption(
                app_weak,
                context,
                session_scope,
                submitted_code,
                submitted_request_id,
                receiver,
            );
            return;
        };
        if !redemption_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &submitted_code,
            &submitted_request_id,
            &receiver,
        ) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_credit_redemption_busy(false);
        match outcome {
            CreditRedemptionOutcome::Redeemed(result) => {
                settle_credit_redemption(
                    &mut context
                        .store
                        .borrow_mut()
                        .pending_credit_redemptions_by_owner,
                    &session_scope.owner_user_id,
                    &submitted_code,
                    &submitted_request_id,
                    CreditRedemptionSettlement::Succeeded,
                );
                state.set_credit_redemption_code("".into());
                state.set_credit_redemption_success(true);
                state.set_credit_redemption_message(
                    credit_redemption_success_message(&result.credits_granted, state.get_en())
                        .into(),
                );
                let credit_sync_epoch = {
                    let mut store = context.store.borrow_mut();
                    begin_credit_sync_epoch(&mut store)
                };
                // An HTTP idempotency replay can contain the balance snapshot from the original
                // response. Apply it only if its server version is not older, then fetch the
                // authoritative account so the fresh snapshot wins shortly afterwards.
                apply_credit_account_balance_if_fresh(
                    &app,
                    &context.store,
                    &result.account.available,
                    &result.account.reserved,
                    &result.account.version,
                    credit_sync_epoch,
                );
                request_authoritative_credit_account(
                    &app,
                    context.clone(),
                    session_scope.clone(),
                    credit_sync_epoch,
                );
                request_credit_ledger_page(
                    &app,
                    context.store.clone(),
                    context.clone(),
                    0,
                    None,
                    Some(credit_sync_epoch),
                );
            }
            CreditRedemptionOutcome::Failed(error) => {
                let settlement = if error.should_preserve_redemption_retry() {
                    CreditRedemptionSettlement::AmbiguousFailure
                } else {
                    CreditRedemptionSettlement::DefinitiveFailure
                };
                settle_credit_redemption(
                    &mut context
                        .store
                        .borrow_mut()
                        .pending_credit_redemptions_by_owner,
                    &session_scope.owner_user_id,
                    &submitted_code,
                    &submitted_request_id,
                    settlement,
                );
                state.set_credit_redemption_success(false);
                state
                    .set_credit_redemption_message(error.redemption_message(state.get_en()).into());
            }
        }
    });
}

fn request_authoritative_credit_account(
    app: &AppWindow,
    context: AppContext,
    session_scope: SessionScope,
    credit_sync_epoch: u64,
) {
    if context.account_scope_disposition(&session_scope) != AccountScopeDisposition::Current {
        return;
    }
    if !credit_sync_epoch_is_current(&context.store.borrow(), credit_sync_epoch) {
        return;
    }
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let request_epoch = begin_credit_account_refresh(&mut context.store.borrow_mut());
    let worker_scope = session_scope.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = AccountApi::new(backend.api.clone()).credit_account_scoped(&worker_scope);
        let _ = sender.send(result);
    });
    poll_authoritative_credit_account(
        app.as_weak(),
        context,
        session_scope,
        credit_sync_epoch,
        request_epoch,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_authoritative_credit_account(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    credit_sync_epoch: u64,
    request_epoch: u64,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<CreditAccount, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !credit_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        if !credit_sync_epoch_is_current(&context.store.borrow(), credit_sync_epoch)
            || !credit_account_refresh_is_current(&context.store.borrow(), request_epoch)
        {
            receiver.borrow_mut().take();
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
                    return;
                }
            }
        };
        let Some(result) = result else {
            poll_authoritative_credit_account(
                app_weak,
                context,
                session_scope,
                credit_sync_epoch,
                request_epoch,
                receiver,
            );
            return;
        };
        if !credit_poll_is_current(&app_weak, &context, &session_scope, &receiver)
            || !credit_sync_epoch_is_current(&context.store.borrow(), credit_sync_epoch)
            || !credit_account_refresh_is_current(&context.store.borrow(), request_epoch)
        {
            return;
        }
        let (Some(app), Ok(account)) = (app_weak.upgrade(), result) else {
            return;
        };
        apply_credit_account_balance_if_fresh(
            &app,
            &context.store,
            &account.available,
            &account.reserved,
            &account.version,
            credit_sync_epoch,
        );
    });
}

fn redemption_poll_is_current<T>(
    app_weak: &Weak<AppWindow>,
    context: &AppContext,
    session_scope: &SessionScope,
    submitted_code: &str,
    submitted_request_id: &str,
    receiver: &Rc<RefCell<Option<mpsc::Receiver<T>>>>,
) -> bool {
    if !credit_poll_is_current(app_weak, context, session_scope, receiver) {
        return false;
    }
    let request_is_current = context
        .store
        .borrow()
        .pending_credit_redemptions_by_owner
        .get(&session_scope.owner_user_id)
        .is_some_and(|pending| {
            pending.code == submitted_code && pending.client_request_id == submitted_request_id
        });
    if !request_is_current {
        receiver.borrow_mut().take();
    }
    request_is_current
}

fn local_redemption_validation_message(message: &str, english: bool) -> &str {
    if !english {
        return message;
    }
    match message {
        "请输入兑换码" => "Enter a redemption code",
        "兑换码长度不能超过 64 个字符" => {
            "The redemption code cannot exceed 64 characters"
        }
        _ => "Check the redemption code and try again",
    }
}

fn credit_redemption_success_message(credits_granted: &str, english: bool) -> String {
    if english {
        format!("Redeemed successfully. {credits_granted} credits have been added to this account.")
    } else {
        format!("兑换成功，{credits_granted} 积分已到账当前账号。")
    }
}

fn validate_credit_redemption_code(
    submitted_code: &str,
) -> std::result::Result<String, &'static str> {
    let code = submitted_code.trim();
    if code.is_empty() {
        return Err("请输入兑换码");
    }
    if code.chars().count() > 64 {
        return Err("兑换码长度不能超过 64 个字符");
    }
    Ok(code.to_string())
}

pub(super) fn reset_credit_ledger(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    items: &[CreditLedgerItem],
    next_cursor: Option<String>,
) {
    let records = items.iter().map(credit_record).collect::<Vec<_>>();
    let orders = invoice_orders(app, items);
    let (page, has_previous, has_next) = {
        let mut store = store.borrow_mut();
        store.credit_ledger_pagination.reset(next_cursor);
        pagination_view(&store.credit_ledger_pagination)
    };
    apply_credit_ledger_view(app, records, orders, page, has_previous, has_next);
}

fn request_credit_ledger_page(
    app: &AppWindow,
    store: Rc<RefCell<Store>>,
    context: AppContext,
    target_index: usize,
    cursor: Option<String>,
    credit_sync_epoch: Option<u64>,
) {
    let state = app.global::<AppState>();
    let Some(session_scope) = context.current_account_session_scope() else {
        state.set_credit_ledger_message("登录状态已失效，请重新登录".into());
        return;
    };
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let (credit_sync_epoch, request_epoch) = {
        let mut store = store.borrow_mut();
        let credit_sync_epoch =
            credit_sync_epoch.unwrap_or_else(|| begin_credit_sync_epoch(&mut store));
        if !credit_sync_epoch_is_current(&store, credit_sync_epoch) {
            return;
        }
        let request_epoch = store.credit_ledger_pagination.begin_request();
        (credit_sync_epoch, request_epoch)
    };
    state.set_credit_ledger_loading(true);
    state.set_credit_ledger_message("".into());

    let request_cursor = cursor.clone();
    let worker_scope = session_scope.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = AccountApi::new(backend.api.clone()).ledger_page_scoped(
            request_cursor.as_deref(),
            CREDIT_LEDGER_PAGE_SIZE,
            &worker_scope,
        );
        let _ = sender.send(result);
    });
    poll_credit_ledger_page(
        app.as_weak(),
        context,
        store,
        session_scope,
        credit_sync_epoch,
        request_epoch,
        target_index,
        cursor,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_credit_ledger_page(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    store: Rc<RefCell<Store>>,
    session_scope: SessionScope,
    credit_sync_epoch: u64,
    request_epoch: u64,
    target_index: usize,
    start_cursor: Option<String>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<CreditLedgerPage, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !credit_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        if !credit_sync_epoch_is_current(&store.borrow(), credit_sync_epoch)
            || !store
                .borrow()
                .credit_ledger_pagination
                .request_is_current(request_epoch)
        {
            receiver.borrow_mut().take();
            return;
        }
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
                    Some(Err(ApiError::LocalState {
                        message: "积分明细加载任务意外中断".to_string(),
                    }))
                }
            }
        };
        let Some(result) = result else {
            poll_credit_ledger_page(
                app_weak,
                context,
                store,
                session_scope,
                credit_sync_epoch,
                request_epoch,
                target_index,
                start_cursor,
                receiver,
            );
            return;
        };
        if !credit_poll_is_current(&app_weak, &context, &session_scope, &receiver)
            || !credit_sync_epoch_is_current(&store.borrow(), credit_sync_epoch)
            || !store
                .borrow()
                .credit_ledger_pagination
                .request_is_current(request_epoch)
        {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_credit_ledger_loading(false);
        match result {
            Ok(page) => {
                let records = page.items.iter().map(credit_record).collect::<Vec<_>>();
                let orders = invoice_orders(&app, &page.items);
                let (page_number, has_previous, has_next) = {
                    let mut store = store.borrow_mut();
                    store.credit_ledger_pagination.apply_page(
                        target_index,
                        start_cursor,
                        page.next_cursor,
                    );
                    pagination_view(&store.credit_ledger_pagination)
                };
                apply_credit_ledger_view(
                    &app,
                    records,
                    orders,
                    page_number,
                    has_previous,
                    has_next,
                );
            }
            Err(error) => state.set_credit_ledger_message(
                format!("积分明细加载失败：{}", error.user_message()).into(),
            ),
        }
    });
}

fn credit_poll_is_current<T>(
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
                sign_out_locally(&app, context, true, Some(session_scope.auth_epoch));
            }
            false
        }
        AccountScopeDisposition::Stale => {
            receiver.borrow_mut().take();
            false
        }
    }
}

fn pagination_view(pagination: &CreditLedgerPagination) -> (i32, bool, bool) {
    (
        pagination.page_number().min(i32::MAX as usize) as i32,
        pagination.has_previous(),
        pagination.has_next(),
    )
}

fn apply_credit_ledger_view(
    app: &AppWindow,
    records: Vec<CreditRecord>,
    orders: Vec<InvoiceOrderView>,
    page: i32,
    has_previous: bool,
    has_next: bool,
) {
    let state = app.global::<AppState>();
    state.set_credit_records(ModelRc::new(VecModel::from(records)));
    state.set_invoice_orders(ModelRc::new(VecModel::from(orders)));
    state.set_credit_ledger_page(page);
    state.set_credit_ledger_has_previous(has_previous);
    state.set_credit_ledger_has_next(has_next);
    state.set_credit_ledger_loading(false);
    state.set_credit_ledger_message("".into());
}

fn invoice_orders(app: &AppWindow, items: &[CreditLedgerItem]) -> Vec<InvoiceOrderView> {
    let packs = app
        .global::<AppState>()
        .get_credit_packs()
        .iter()
        .collect::<Vec<_>>();
    items
        .iter()
        .filter_map(|item| invoice_order(item, &packs))
        .collect()
}

fn invoice_order(item: &CreditLedgerItem, packs: &[CreditPackView]) -> Option<InvoiceOrderView> {
    if item.entry_type != "grant" || item.business_type != "order" {
        return None;
    }

    let credits = absolute_credit_amount(&item.available_delta);
    let pack = packs.iter().find(|pack| pack.credits.as_str() == credits);
    let (amount, amount_cents, eligible, status) = match pack {
        Some(pack) => {
            let eligible = decimal_at_least(pack.price_cents.as_str(), "10000");
            (
                pack.price.to_string(),
                pack.price_cents.to_string(),
                eligible,
                if eligible {
                    "可申请开票".to_string()
                } else {
                    "单次充值未满 ¥100.00".to_string()
                },
            )
        }
        None => (
            "金额待确认".to_string(),
            String::new(),
            false,
            "暂无法确认订单金额".to_string(),
        ),
    };

    Some(InvoiceOrderView {
        id: item.id.clone().into(),
        title: format!("充值 {credits} 积分").into(),
        amount: amount.into(),
        amount_cents: amount_cents.into(),
        time: format_ledger_time(&item.created_at).into(),
        eligible,
        status: status.into(),
    })
}

fn decimal_at_least(value: &str, minimum: &str) -> bool {
    let value = value.trim().trim_start_matches('0');
    let minimum = minimum.trim().trim_start_matches('0');
    let value = if value.is_empty() { "0" } else { value };
    let minimum = if minimum.is_empty() { "0" } else { minimum };
    value.len() > minimum.len() || (value.len() == minimum.len() && value >= minimum)
}

fn credit_record(item: &CreditLedgerItem) -> CreditRecord {
    let business = business_type_label(&item.business_type);
    let balance = format!("可用积分余额 {}", item.available_after);
    let (title, amount, note, tone) = match item.entry_type.as_str() {
        "reserve" => (
            "AI 创作积分暂时冻结".to_string(),
            format!(
                "冻结 {}",
                preferred_absolute(&item.reserved_delta, &item.available_delta)
            ),
            format!("{business}任务处理中暂时冻结，失败或未使用部分会自动退回 · {balance}"),
            "neutral",
        ),
        "commit" => (
            "AI 创作积分已扣除".to_string(),
            format!(
                "扣除 {}",
                preferred_absolute(&item.reserved_delta, &item.available_delta)
            ),
            format!("{business}任务完成，已从冻结积分中结算 · {balance}"),
            "negative",
        ),
        "release" => (
            "未使用积分已退回".to_string(),
            format!(
                "退回 {}",
                preferred_absolute(&item.available_delta, &item.reserved_delta)
            ),
            format!("{business}未消耗的冻结积分已返还 · {balance}"),
            "positive",
        ),
        "grant" => (
            non_empty_description(item, "积分已到账"),
            signed_credit_amount(&item.available_delta),
            format!("{business} · {balance}"),
            "positive",
        ),
        "expire" => (
            "积分已过期".to_string(),
            negative_credit_amount(&item.available_delta),
            format!("{business} · {balance}"),
            "negative",
        ),
        _ => {
            let tone = credit_tone(&item.available_delta);
            (
                non_empty_description(item, "积分变动"),
                signed_credit_amount(&item.available_delta),
                format!("{business} · {balance}"),
                tone,
            )
        }
    };

    CreditRecord {
        title: title.into(),
        amount: amount.into(),
        time: format_ledger_time(&item.created_at).into(),
        note: note.into(),
        tone: tone.into(),
    }
}

fn business_type_label(value: &str) -> &'static str {
    match value {
        "generation_task" => "AI 创作",
        "generation_retry" => "任务重试",
        "membership" => "会员赠送",
        "membership_upgrade" => "会员升级",
        "order" => "积分充值",
        "redemption_code" => "兑换码兑换",
        "registration" => "注册赠送",
        "user" => "人工调整",
        "outbox_event" => "系统补偿",
        _ => "积分变动",
    }
}

fn non_empty_description(item: &CreditLedgerItem, fallback: &str) -> String {
    let description = item.description.trim();
    if description.is_empty() {
        fallback.to_string()
    } else {
        description.to_string()
    }
}

fn preferred_absolute(primary: &str, fallback: &str) -> String {
    let primary = absolute_credit_amount(primary);
    if primary == "0" {
        absolute_credit_amount(fallback)
    } else {
        primary
    }
}

fn absolute_credit_amount(value: &str) -> String {
    let normalized = value
        .trim()
        .trim_start_matches(['-', '+'])
        .trim_start_matches('0');
    if normalized.is_empty() {
        "0".to_string()
    } else {
        normalized.to_string()
    }
}

fn signed_credit_amount(value: &str) -> String {
    let absolute = absolute_credit_amount(value);
    if absolute == "0" {
        "0".to_string()
    } else if value.trim().starts_with('-') {
        format!("-{absolute}")
    } else {
        format!("+{absolute}")
    }
}

fn negative_credit_amount(value: &str) -> String {
    let absolute = absolute_credit_amount(value);
    if absolute == "0" {
        "0".to_string()
    } else {
        format!("-{absolute}")
    }
}

fn credit_tone(value: &str) -> &'static str {
    let absolute = absolute_credit_amount(value);
    if absolute == "0" {
        "neutral"
    } else if value.trim().starts_with('-') {
        "negative"
    } else {
        "positive"
    }
}

fn format_ledger_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| {
            time.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_item(
        entry_type: &str,
        available_delta: &str,
        reserved_delta: &str,
        business_type: &str,
    ) -> CreditLedgerItem {
        CreditLedgerItem {
            id: "100".to_string(),
            entry_type: entry_type.to_string(),
            available_delta: available_delta.to_string(),
            reserved_delta: reserved_delta.to_string(),
            available_after: "850".to_string(),
            reserved_after: "0".to_string(),
            business_type: business_type.to_string(),
            description: "服务端技术描述".to_string(),
            created_at: "2026-07-15T12:44:40.734Z".to_string(),
        }
    }

    fn invoice_pack(credits: &str, price: &str, price_cents: &str) -> CreditPackView {
        CreditPackView {
            code: format!("pack_{credits}").into(),
            name: format!("{credits} 积分").into(),
            credits: credits.into(),
            price: price.into(),
            price_cents: price_cents.into(),
            note: "".into(),
        }
    }

    #[test]
    fn invoice_order_is_enabled_at_exactly_one_hundred_yuan() {
        let item = ledger_item("grant", "10000", "0", "order");
        let order = invoice_order(&item, &[invoice_pack("10000", "¥ 100.00", "10000")])
            .expect("credit recharge order");

        assert!(order.eligible);
        assert_eq!(order.amount.as_str(), "¥ 100.00");
        assert_eq!(order.amount_cents.as_str(), "10000");
        assert_eq!(order.status.as_str(), "可申请开票");
    }

    #[test]
    fn invoice_order_below_one_hundred_yuan_is_disabled() {
        let item = ledger_item("grant", "5000", "0", "order");
        let order = invoice_order(&item, &[invoice_pack("5000", "¥ 99.99", "9999")])
            .expect("credit recharge order");

        assert!(!order.eligible);
        assert_eq!(order.status.as_str(), "单次充值未满 ¥100.00");
    }

    #[test]
    fn non_recharge_ledger_entries_are_not_invoice_orders() {
        let item = ledger_item("grant", "10000", "0", "membership");
        assert!(invoice_order(&item, &[invoice_pack("10000", "¥ 100.00", "10000")],).is_none());
    }

    #[test]
    fn reserve_is_explained_as_a_temporary_freeze() {
        let record = credit_record(&ledger_item("reserve", "-50", "50", "generation_task"));

        assert_eq!(record.title.as_str(), "AI 创作积分暂时冻结");
        assert_eq!(record.amount.as_str(), "冻结 50");
        assert_eq!(record.tone.as_str(), "neutral");
        assert!(record.note.as_str().contains("失败或未使用部分会自动退回"));
        assert!(record.note.as_str().contains("可用积分余额 850"));
        assert!(!record.note.as_str().contains("generation_task"));
    }

    #[test]
    fn commit_uses_reserved_delta_instead_of_zero_available_delta() {
        let record = credit_record(&ledger_item("commit", "0", "-50", "generation_task"));

        assert_eq!(record.title.as_str(), "AI 创作积分已扣除");
        assert_eq!(record.amount.as_str(), "扣除 50");
        assert_eq!(record.tone.as_str(), "negative");
        assert!(record.note.as_str().contains("已从冻结积分中结算"));
        assert!(!record.note.as_str().contains("generation_task"));
    }

    #[test]
    fn release_is_explained_as_returned_credit() {
        let record = credit_record(&ledger_item("release", "50", "-50", "generation_task"));

        assert_eq!(record.title.as_str(), "未使用积分已退回");
        assert_eq!(record.amount.as_str(), "退回 50");
        assert_eq!(record.tone.as_str(), "positive");
        assert!(record.note.as_str().contains("冻结积分已返还"));
    }

    #[test]
    fn fallback_never_exposes_an_unknown_business_code() {
        let record = credit_record(&ledger_item("adjust", "12", "0", "internal_code"));

        assert_eq!(record.amount.as_str(), "+12");
        assert_eq!(record.tone.as_str(), "positive");
        assert!(record.note.as_str().contains("积分变动"));
        assert!(!record.note.as_str().contains("internal_code"));
        assert!(!record.time.as_str().contains('T'));
        assert!(!record.time.as_str().contains('Z'));
    }

    #[test]
    fn redemption_ledger_entries_use_a_user_facing_business_label() {
        let record = credit_record(&ledger_item("grant", "500", "0", "redemption_code"));

        assert_eq!(record.amount.as_str(), "+500");
        assert!(record.note.as_str().contains("兑换码兑换"));
        assert!(!record.note.as_str().contains("redemption_code"));
    }

    #[test]
    fn redemption_success_message_uses_the_server_grant_amount() {
        assert_eq!(
            credit_redemption_success_message("875", false),
            "兑换成功，875 积分已到账当前账号。"
        );
        assert_eq!(
            credit_redemption_success_message("875", true),
            "Redeemed successfully. 875 credits have been added to this account."
        );
    }

    #[test]
    fn redemption_code_validation_only_trims_and_limits_length() {
        assert_eq!(
            validate_credit_redemption_code("  elun-abcd-2345  "),
            Ok("elun-abcd-2345".to_string())
        );
        assert_eq!(validate_credit_redemption_code("  "), Err("请输入兑换码"));
        assert_eq!(
            validate_credit_redemption_code(&"A".repeat(65)),
            Err("兑换码长度不能超过 64 个字符")
        );
        assert_eq!(
            local_redemption_validation_message("请输入兑换码", true),
            "Enter a redemption code"
        );
    }

    #[test]
    fn redemption_retry_identity_reuses_only_ambiguous_same_account_requests() {
        let mut pending = BTreeMap::new();
        let first = credit_redemption_request_id(&mut pending, "user-a", "ELUN-CODE-A", || {
            "request-a-1".to_string()
        });
        settle_credit_redemption(
            &mut pending,
            "user-a",
            "ELUN-CODE-A",
            "request-a-1",
            CreditRedemptionSettlement::AmbiguousFailure,
        );
        let replay = credit_redemption_request_id(&mut pending, "user-a", "ELUN-CODE-A", || {
            panic!("ambiguous same-code retry must reuse the original request id")
        });
        let other_account =
            credit_redemption_request_id(&mut pending, "user-b", "ELUN-CODE-A", || {
                "request-b-1".to_string()
            });
        let first_account_after_other =
            credit_redemption_request_id(&mut pending, "user-a", "ELUN-CODE-A", || {
                panic!("another account must not replace the first account's request id")
            });

        assert_eq!(first, "request-a-1");
        assert_eq!(replay, first);
        assert_eq!(other_account, "request-b-1");
        assert_ne!(other_account, first);
        assert_eq!(first_account_after_other, first);
    }

    #[test]
    fn definitive_failure_and_success_clear_redemption_retry_identity() {
        let mut pending = BTreeMap::new();
        assert_eq!(
            credit_redemption_request_id(&mut pending, "user-a", "ELUN-CODE-A", || "request-a-1"
                .to_string(),),
            "request-a-1"
        );
        settle_credit_redemption(
            &mut pending,
            "user-a",
            "ELUN-CODE-A",
            "request-a-1",
            CreditRedemptionSettlement::DefinitiveFailure,
        );
        assert_eq!(
            credit_redemption_request_id(&mut pending, "user-a", "ELUN-CODE-A", || "request-a-2"
                .to_string(),),
            "request-a-2"
        );
        settle_credit_redemption(
            &mut pending,
            "user-a",
            "ELUN-CODE-A",
            "request-a-2",
            CreditRedemptionSettlement::Succeeded,
        );
        assert_eq!(
            credit_redemption_request_id(&mut pending, "user-a", "ELUN-CODE-A", || "request-a-3"
                .to_string(),),
            "request-a-3"
        );
    }

    #[test]
    fn stale_completion_does_not_clear_a_newer_redemption_retry_identity() {
        let mut pending = BTreeMap::new();
        credit_redemption_request_id(&mut pending, "user-a", "ELUN-CODE-A", || {
            "request-a-1".to_string()
        });
        assert_eq!(
            credit_redemption_request_id(&mut pending, "user-a", "ELUN-CODE-B", || "request-b-1"
                .to_string(),),
            "request-b-1"
        );

        settle_credit_redemption(
            &mut pending,
            "user-a",
            "ELUN-CODE-A",
            "request-a-1",
            CreditRedemptionSettlement::Succeeded,
        );

        assert_eq!(
            credit_redemption_request_id(&mut pending, "user-a", "ELUN-CODE-B", || panic!(
                "a stale completion must not remove the newer request"
            ),),
            "request-b-1"
        );
    }

    #[test]
    fn redemption_stale_idempotency_snapshot_cannot_replace_a_newer_credit_account_version() {
        assert!(credit_account_version_is_fresh(None, "8", true));
        assert!(credit_account_version_is_fresh(Some("8"), "8", true));
        assert!(credit_account_version_is_fresh(Some("8"), "10", true));
        assert!(!credit_account_version_is_fresh(Some("8"), "7", true));
        assert!(!credit_account_version_is_fresh(Some("10"), "0009", true));
        assert!(!credit_account_version_is_fresh(
            Some("10"),
            "invalid",
            true
        ));
    }

    #[test]
    fn unversioned_credit_fallback_requires_the_latest_shared_request_epoch() {
        assert!(credit_account_version_is_fresh(Some("10"), "", true));
        assert!(!credit_account_version_is_fresh(Some("10"), "", false));
        assert!(!credit_account_version_is_fresh(None, "12", false));
    }

    #[test]
    fn newer_credit_sync_invalidates_older_full_and_targeted_snapshots() {
        let mut store = Store::default();
        let full_snapshot = begin_credit_sync_epoch(&mut store);
        let redemption_reconciliation = begin_credit_sync_epoch(&mut store);

        assert!(!credit_sync_epoch_is_current(&store, full_snapshot));
        assert!(credit_sync_epoch_is_current(
            &store,
            redemption_reconciliation
        ));

        invalidate_credit_sync_epoch(&mut store);

        assert!(!credit_sync_epoch_is_current(
            &store,
            redemption_reconciliation
        ));
    }

    #[test]
    fn redemption_only_the_latest_authoritative_credit_refresh_may_apply() {
        let mut store = Store::default();
        let replay_refresh = begin_credit_account_refresh(&mut store);
        let newer_refresh = begin_credit_account_refresh(&mut store);

        assert!(!credit_account_refresh_is_current(&store, replay_refresh));
        assert!(credit_account_refresh_is_current(&store, newer_refresh));
    }

    #[test]
    fn pagination_remembers_page_start_cursors_for_back_navigation() {
        let mut pagination = CreditLedgerPagination::default();
        pagination.reset(Some("80".to_string()));

        assert_eq!(pagination.page_number(), 1);
        assert_eq!(pagination.previous_target(), None);
        assert_eq!(pagination.next_target(), Some((1, Some("80".to_string()))));

        pagination.apply_page(1, Some("80".to_string()), Some("72".to_string()));

        assert_eq!(pagination.page_number(), 2);
        assert_eq!(pagination.previous_target(), Some((0, None)));
        assert_eq!(pagination.next_target(), Some((2, Some("72".to_string()))));
    }

    #[test]
    fn ledger_reset_invalidates_an_in_flight_page_request() {
        let mut pagination = CreditLedgerPagination::default();
        pagination.reset(Some("80".to_string()));
        let request_epoch = pagination.begin_request();
        assert!(pagination.request_is_current(request_epoch));

        pagination.reset(Some("40".to_string()));

        assert!(!pagination.request_is_current(request_epoch));
    }

    #[test]
    fn applying_previous_page_restores_first_page_state() {
        let mut pagination = CreditLedgerPagination::default();
        pagination.reset(Some("80".to_string()));
        pagination.apply_page(1, Some("80".to_string()), Some("72".to_string()));

        let (target_index, cursor) = pagination.previous_target().unwrap();
        pagination.apply_page(target_index, cursor, Some("80".to_string()));

        assert_eq!(pagination.page_number(), 1);
        assert_eq!(pagination.previous_target(), None);
        assert_eq!(pagination.next_target(), Some((1, Some("80".to_string()))));
    }
}
