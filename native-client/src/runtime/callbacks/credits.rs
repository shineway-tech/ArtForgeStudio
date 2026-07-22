use super::*;

#[derive(Clone, Default)]
pub(super) struct CreditLedgerPagination {
    page_cursors: Vec<Option<String>>,
    page_index: usize,
    next_cursor: Option<String>,
}

impl CreditLedgerPagination {
    fn reset(&mut self, next_cursor: Option<String>) {
        self.page_cursors = vec![None];
        self.page_index = 0;
        self.next_cursor = next_cursor;
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
        let store = context.store.clone();
        let backend = backend.clone();
        state.on_credit_ledger_previous_page(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_credit_ledger_loading() {
                return;
            }
            let target = store
                .borrow()
                .credit_ledger_pagination
                .previous_target();
            if let Some((target_index, cursor)) = target {
                request_credit_ledger_page(
                    &app,
                    store.clone(),
                    backend.clone(),
                    target_index,
                    cursor,
                );
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = context.store.clone();
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
                    backend.clone(),
                    target_index,
                    cursor,
                );
            }
        });
    }
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
    backend: Arc<BackendRuntime>,
    target_index: usize,
    cursor: Option<String>,
) {
    let state = app.global::<AppState>();
    state.set_credit_ledger_loading(true);
    state.set_credit_ledger_message("".into());

    let request_cursor = cursor.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = AccountApi::new(backend.api.clone()).ledger_page(
            request_cursor.as_deref(),
            CREDIT_LEDGER_PAGE_SIZE,
        );
        let _ = sender.send(result);
    });
    poll_credit_ledger_page(
        app.as_weak(),
        store,
        target_index,
        cursor,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_credit_ledger_page(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    target_index: usize,
    start_cursor: Option<String>,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<CreditLedgerPage, ApiError>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
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
                store,
                target_index,
                start_cursor,
                receiver,
            );
            return;
        };
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

fn invoice_order(
    item: &CreditLedgerItem,
    packs: &[CreditPackView],
) -> Option<InvoiceOrderView> {
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
            format!("冻结 {}", preferred_absolute(&item.reserved_delta, &item.available_delta)),
            format!("{business}任务处理中暂时冻结，失败或未使用部分会自动退回 · {balance}"),
            "neutral",
        ),
        "commit" => (
            "AI 创作积分已扣除".to_string(),
            format!("扣除 {}", preferred_absolute(&item.reserved_delta, &item.available_delta)),
            format!("{business}任务完成，已从冻结积分中结算 · {balance}"),
            "negative",
        ),
        "release" => (
            "未使用积分已退回".to_string(),
            format!("退回 {}", preferred_absolute(&item.available_delta, &item.reserved_delta)),
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
    let normalized = value.trim().trim_start_matches(['-', '+']).trim_start_matches('0');
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
        let order = invoice_order(
            &item,
            &[invoice_pack("10000", "¥ 100.00", "10000")],
        )
        .expect("credit recharge order");

        assert!(order.eligible);
        assert_eq!(order.amount.as_str(), "¥ 100.00");
        assert_eq!(order.amount_cents.as_str(), "10000");
        assert_eq!(order.status.as_str(), "可申请开票");
    }

    #[test]
    fn invoice_order_below_one_hundred_yuan_is_disabled() {
        let item = ledger_item("grant", "5000", "0", "order");
        let order = invoice_order(
            &item,
            &[invoice_pack("5000", "¥ 99.99", "9999")],
        )
        .expect("credit recharge order");

        assert!(!order.eligible);
        assert_eq!(order.status.as_str(), "单次充值未满 ¥100.00");
    }

    #[test]
    fn non_recharge_ledger_entries_are_not_invoice_orders() {
        let item = ledger_item("grant", "10000", "0", "membership");
        assert!(invoice_order(
            &item,
            &[invoice_pack("10000", "¥ 100.00", "10000")],
        )
        .is_none());
    }

    #[test]
    fn reserve_is_explained_as_a_temporary_freeze() {
        let record = credit_record(&ledger_item(
            "reserve",
            "-50",
            "50",
            "generation_task",
        ));

        assert_eq!(record.title.as_str(), "AI 创作积分暂时冻结");
        assert_eq!(record.amount.as_str(), "冻结 50");
        assert_eq!(record.tone.as_str(), "neutral");
        assert!(record.note.as_str().contains("失败或未使用部分会自动退回"));
        assert!(record.note.as_str().contains("可用积分余额 850"));
        assert!(!record.note.as_str().contains("generation_task"));
    }

    #[test]
    fn commit_uses_reserved_delta_instead_of_zero_available_delta() {
        let record = credit_record(&ledger_item(
            "commit",
            "0",
            "-50",
            "generation_task",
        ));

        assert_eq!(record.title.as_str(), "AI 创作积分已扣除");
        assert_eq!(record.amount.as_str(), "扣除 50");
        assert_eq!(record.tone.as_str(), "negative");
        assert!(record.note.as_str().contains("已从冻结积分中结算"));
        assert!(!record.note.as_str().contains("generation_task"));
    }

    #[test]
    fn release_is_explained_as_returned_credit() {
        let record = credit_record(&ledger_item(
            "release",
            "50",
            "-50",
            "generation_task",
        ));

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
    fn pagination_remembers_page_start_cursors_for_back_navigation() {
        let mut pagination = CreditLedgerPagination::default();
        pagination.reset(Some("80".to_string()));

        assert_eq!(pagination.page_number(), 1);
        assert_eq!(pagination.previous_target(), None);
        assert_eq!(
            pagination.next_target(),
            Some((1, Some("80".to_string())))
        );

        pagination.apply_page(
            1,
            Some("80".to_string()),
            Some("72".to_string()),
        );

        assert_eq!(pagination.page_number(), 2);
        assert_eq!(pagination.previous_target(), Some((0, None)));
        assert_eq!(
            pagination.next_target(),
            Some((2, Some("72".to_string())))
        );
    }

    #[test]
    fn applying_previous_page_restores_first_page_state() {
        let mut pagination = CreditLedgerPagination::default();
        pagination.reset(Some("80".to_string()));
        pagination.apply_page(
            1,
            Some("80".to_string()),
            Some("72".to_string()),
        );

        let (target_index, cursor) = pagination.previous_target().unwrap();
        pagination.apply_page(target_index, cursor, Some("80".to_string()));

        assert_eq!(pagination.page_number(), 1);
        assert_eq!(pagination.previous_target(), None);
        assert_eq!(
            pagination.next_target(),
            Some((1, Some("80".to_string())))
        );
    }
}
