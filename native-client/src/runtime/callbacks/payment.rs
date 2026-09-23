use super::*;

const PAYMENT_STATUS_UNAVAILABLE: &str = "暂时无法确认支付结果，请稍后查看订单状态";
const MISSING_ORIGINAL_UPGRADE_QUOTE: &str = "缺少原始升级报价，记录已保留；不会重新报价或更改付款方";

struct PaymentStarted {
    order: OrderDetail,
    client_request_id: String,
    kind: PaymentOrderKind,
    presentation: PaymentPresentation,
    session_scope: SessionScope,
    record: PendingOrderRecord,
    settled: bool,
}

#[derive(Clone, Debug)]
struct PaymentPresentation {
    waiting_message: String,
    success_message: String,
    success_detail: String,
    credit_fallback_total: String,
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
        Self::credit_with_total(credits)
    }
    fn credit_with_recharge(fallback_total: &str, recharge: Option<&CreditRechargeSummary>) -> Self {
        let total = recharge.and_then(|r| r.total_credits.as_deref()).filter(|v| !v.trim().is_empty()).unwrap_or(fallback_total);
        Self::credit_with_total(total)
    }
    fn credit_with_total(credits: &str) -> Self {
        let credits = credits.trim();
        Self {
            waiting_message: "已在浏览器中打开支付宝，客户端正在等待积分充值结果".to_string(),
            success_message: if credits.is_empty() {
                "积分已到账".to_string()
            } else {
                format!("{credits} 积分已到账")
            },
            success_detail: "积分余额已更新".to_string(),
            credit_fallback_total: credits.to_string(),
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
            credit_fallback_total: String::new(),
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


#[derive(Clone)]
struct PaymentCapture {
    persistence: PrivatePersistence,
    authority: Arc<NamespaceStorageAuthority>,
    session: SessionScope,
    backend: Arc<BackendRuntime>,
    active_namespace: Arc<Mutex<Option<NamespaceLease>>>,
}
impl PaymentCapture {
    fn new(context: &AppContext) -> std::result::Result<Self, ApiError> {
        let persistence=context.store.borrow().private_persistence.clone().ok_or(ApiError::AuthenticationRequired)?;
        let backend=context.backend.clone().ok_or(ApiError::AuthenticationRequired)?;
        if let Some(required)=backend.api.upgrade_latch().snapshot(){return Err(required.as_error());}
        let session=backend.api.session().scope_for_user(persistence.lease().namespace.user_public_id())
            .filter(|scope|scope.auth_epoch==persistence.lease().auth_epoch).ok_or(ApiError::AuthenticationRequired)?;
        let authority=persistence.storage_authority().map_err(transition_error)?;
        let captured=Self{persistence,authority,session,backend,active_namespace:context.active_namespace.clone()};
        if !captured.is_current(context){return Err(ApiError::AuthenticationRequired);}
        Ok(captured)
    }
    fn binding_matches(&self,context:&AppContext)->bool {
        context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.lease()==self.persistence.lease())
    }
    fn namespace_is_current(&self,context:&AppContext)->bool {
        !PAYMENT_SHUTDOWN.with(|closed|closed.get()) && self.binding_matches(context)
            && self.active_namespace.lock().ok().is_some_and(|active|active.as_ref()==Some(self.persistence.lease()))
            && self.persistence.is_current()
    }
    fn is_current(&self,context:&AppContext)->bool {
        self.namespace_is_current(context) && self.backend.api.session().is_scope_current(&self.session)
    }
    fn apply<R>(&self,context:&AppContext,apply:impl FnOnce()->R)->Option<R> {
        if !self.is_current(context){return None;}
        context.apply_user_completion(self.persistence.lease(),|| {
            self.binding_matches(context).then(apply)
        }).ok().flatten()
    }
}
fn apply_payment_ui<R>(
    app:&AppWindow,context:&AppContext,capture:&PaymentCapture,payer:&str,apply:impl FnOnce(&AppState)->R,
)->Option<R> {
    let visible=context.billing_context.current_scope(KnownCapability::ReadGroupFinance).ok()?;
    if visible.request.session!=capture.session || visible.request.account_group_id!=payer {return None;}
    capture.apply(context,|| {
        if context.billing_context.confirmed_scope().as_ref()!=Some(&visible){return None;}
        Some(apply(&app.global::<AppState>()))
    }).flatten()
}

struct PaymentThread {
    lease:NamespaceLease,key:String,cancelled:Arc<std::sync::atomic::AtomicBool>,handle:std::thread::JoinHandle<()>,
}
thread_local! {
    static PAYMENT_THREADS:RefCell<Vec<PaymentThread>>=const{RefCell::new(Vec::new())};
    static PAYMENT_JOIN_FAILED:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
    static PAYMENT_SHUTDOWN:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
    #[cfg(test)]
    static PAYMENT_BEFORE_CREATE_RECORD:RefCell<Option<Box<dyn FnOnce()+Send>>>=const{RefCell::new(None)};
}
fn reap_payment_workers(){
    let ready=PAYMENT_THREADS.with(|threads|{
        let mut threads=threads.borrow_mut();let mut ready=Vec::new();let mut index=0;
        while index<threads.len(){if threads[index].handle.is_finished(){ready.push(threads.remove(index));}else{index+=1;}}
        ready
    });
    for worker in ready {
        if worker.handle.join().is_err(){PAYMENT_JOIN_FAILED.with(|failed|failed.set(true));}
    }
}
fn payment_worker_pending(capture:&PaymentCapture,key:&str)->bool {
    PAYMENT_THREADS.with(|threads|threads.borrow().iter().any(|worker|
        worker.lease==*capture.persistence.lease() && worker.key==key))
}
fn new_payment_work_pending(context:&AppContext,capture:&PaymentCapture)->bool {
    // UI busy/active state is presentation only. A worker can own a NEW key
    // before its first durable row exists, and billing-only clear hides that UI.
    let prefix=format!("{}:{}:",capture.session.owner_user_id,capture.session.auth_epoch);
    context.recovering_orders.borrow().iter().any(|key|key.starts_with(&prefix))
        || PAYMENT_THREADS.with(|threads|threads.borrow().iter().any(|worker|worker.lease==*capture.persistence.lease()))
}
fn join_payment_workers()->std::result::Result<(),String>{
    let workers=PAYMENT_THREADS.with(|threads|std::mem::take(&mut *threads.borrow_mut()));
    for worker in workers {if worker.handle.join().is_err(){PAYMENT_JOIN_FAILED.with(|failed|failed.set(true));}}
    if PAYMENT_JOIN_FAILED.with(|failed|failed.get()){Err("payment worker panicked".into())}else{Ok(())}
}
pub(super) fn cancel_payment_workers_for_retirement(lease:&NamespaceLease){
    PAYMENT_THREADS.with(|threads|{
        for worker in threads.borrow().iter().filter(|worker|&worker.lease==lease){
            worker.cancelled.store(true,std::sync::atomic::Ordering::Release);
        }
    });
}
/// Owning UI thread, after its event loop ends, outside every completion/latch.
/// Request/refresh I/O has finite ApiClient timeouts. Never discard these handles.
pub(super) fn shutdown_payment_workers()->std::result::Result<(),String>{
    PAYMENT_SHUTDOWN.with(|closed|closed.set(true));
    PAYMENT_THREADS.with(|threads|{
        for worker in threads.borrow().iter(){worker.cancelled.store(true,std::sync::atomic::Ordering::Release);}
    });
    join_payment_workers()
}
struct PaymentWorker {
    capture:PaymentCapture,cancelled:Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    before_create_record:Option<Box<dyn FnOnce()+Send>>,
}
impl PaymentWorker {
    fn ensure(&self)->std::result::Result<(),ApiError>{
        if self.cancelled.load(std::sync::atomic::Ordering::Acquire)
            || self.capture.active_namespace.lock().ok().is_none_or(|active|active.as_ref()!=Some(self.capture.persistence.lease()))
            || !self.capture.persistence.is_current()
        {return Err(ApiError::AuthenticationRequired);}
        ensure_payment_scope_active(&self.capture.backend,&self.capture.session)
    }
}
fn spawn_payment_thread(
    context:&AppContext,capture:&PaymentCapture,key:&str,
    work:impl FnOnce(PaymentWorker)->std::result::Result<PaymentStarted,ApiError>+Send+'static,
)->std::result::Result<mpsc::Receiver<std::result::Result<PaymentStarted,ApiError>>,ApiError>{
    if !capture.is_current(context) || payment_worker_pending(capture,key){return Err(ApiError::AuthenticationRequired);}
    let activity=capture.persistence.begin_activity().map_err(transition_error)?;
    let cancelled=Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker=PaymentWorker{capture:capture.clone(),cancelled:cancelled.clone(),
        #[cfg(test)]
        before_create_record:PAYMENT_BEFORE_CREATE_RECORD.with(|hook|hook.borrow_mut().take()),
    };
    let(sender,receiver)=mpsc::channel();
    let handle=std::thread::Builder::new().name("payment-request".into()).spawn(move||{
        let result=worker.ensure().and_then(|()|work(worker));
        drop(activity);
        let _=sender.send(result);
    }).map_err(|_|ApiError::LocalState{message:"支付任务无法启动，恢复记录已保留".into()})?;
    PAYMENT_THREADS.with(|threads|threads.borrow_mut().push(PaymentThread{
        lease:capture.persistence.lease().clone(),key:key.into(),cancelled,handle,
    }));
    Ok(receiver)
}

fn recovering_order_key(session:&SessionScope,key:&str)->String {
    format!("{}:{}:{}",session.owner_user_id,session.auth_epoch,key)
}
fn remove_recovering_order(context:&AppContext,session:&SessionScope,key:&str){
    context.recovering_orders.borrow_mut().remove(&recovering_order_key(session,key));
}
fn payment_matches(context:&AppContext,session:&SessionScope,key:&str)->bool {
    context.active_payment.borrow().as_ref().is_some_and(|active|
        active.client_request_id==key && active.session_scope==*session)
}
fn ensure_payment_scope_active(backend:&BackendRuntime,session:&SessionScope)->std::result::Result<(),ApiError>{
    if backend.api.user_work_is_current(session){Ok(())}else{Err(ApiError::AuthenticationRequired)}
}
fn payment_error_preserves_order_recovery(error:&ApiError)->bool{
    // Only an explicit definitive refusal for a NEW, still-uncreated request can
    // be abandoned. Admission denial/conflict/frozen/unknown never implies that.
    !matches!(error,ApiError::Http{status:400,code,..} if code=="credit_pack_unavailable" || code=="membership_plan_unavailable")
}
fn payment_error_is_transient(error:&ApiError)->bool {
    error.is_network_error() || matches!(error,ApiError::Protocol{..}|ApiError::Http{status:408|429|500..=599,..})
}
fn release_payment_tracking(context:&AppContext,capture:&PaymentCapture,key:&str){
    capture.apply(context,||{
        remove_recovering_order(context,&capture.session,key);
        if payment_matches(context,&capture.session,key){context.active_payment.borrow_mut().take();}
    });
}
fn report_payment_error(
    app:&AppWindow,context:&AppContext,capture:&PaymentCapture,key:&str,payer:&str,kind:PaymentOrderKind,error:ApiError,
){
    // Logout is control work, not an ordinary completion. Do not call it while
    // holding any activity/effect/latch guard that its quiescence must drain.
    if error.is_terminal_session_error() || matches!(error,ApiError::AuthenticationRequired){
        if capture.namespace_is_current(context) && terminal_auth_scope_matches_context(context,&capture.session){
            sign_out_locally(app,context,true,Some(capture.session.auth_epoch));
        }
    }
    let matched=payment_matches(context,&capture.session,key);
    release_payment_tracking(context,capture,key);
    apply_payment_ui(app,context,capture,payer,|state|{
        apply_agreements_from_payment_error(app,&error);
        // Only this locally constructed, fixed disclosure is public. Preserve
        // general LocalState sanitization for storage paths and internal errors.
        let detail=match &error {
            ApiError::LocalState{message} if message==MISSING_ORIGINAL_UPGRADE_QUOTE => MISSING_ORIGINAL_UPGRADE_QUOTE.to_owned(),
            _=>error.user_message(),
        };
        let message=format!("付款操作暂未完成：{}",detail);
        if matched {state.set_payment_active(false);state.set_payment_browser_ready(false);}
        state.set_payment_status_message(message.clone().into());
        set_payment_kind_status(state,kind,false,&message);
    });
}
fn find_saved_payment(capture:&PaymentCapture,key:&str)->std::result::Result<PendingOrderRecord,ApiError>{
    load_pending_orders_for_namespace(&capture.authority).map_err(transition_error)?
        .into_iter().find(|row|row.owner_user_id==capture.session.owner_user_id && row.client_request_id==key)
        .ok_or_else(||ApiError::LocalState{message:"原始订单恢复记录不可用，未更改付款方".into()})
}

/// Validate creator/payer and preserve the accepted order binding before any
/// terminal row removal. All namespace I/O occurs outside the Slint/latch lock.
fn bind_and_settle_order(
    worker:&PaymentWorker,mut record:PendingOrderRecord,order:OrderDetail,
    kind:PaymentOrderKind,presentation:PaymentPresentation,
)->std::result::Result<PaymentStarted,ApiError>{
    worker.ensure()?;
    crate::runtime::api::require_saved_group(&record.billing_account_group_id,&order.billing_account_group_id)?;
    if order.id.trim().is_empty() || (!record.order_id.is_empty() && record.order_id!=order.id){
        return Err(ApiError::LocalState{message:"服务端订单身份不一致，原始记录已保留".into()});
    }
    if record.auth_epoch!=worker.capture.session.auth_epoch {
        rebind_pending_order_epoch_for_namespace(&worker.capture.authority,&record.identity(),worker.capture.session.auth_epoch)
            .and_then(require_order_recovery_update).map_err(transition_error)?;
        record.auth_epoch=worker.capture.session.auth_epoch;
    }
    worker.ensure()?;
    update_pending_order_id_for_namespace(&worker.capture.authority,&record.identity(),&order.id)
        .and_then(require_order_recovery_update).map_err(transition_error)?;
    record.order_id=order.id.clone();
    let settled=matches!(payment_order_phase(&order),PaymentOrderPhase::Fulfilled|PaymentOrderPhase::Closed);
    if settled {
        worker.ensure()?;
        remove_pending_order_for_namespace(&worker.capture.authority,&record.identity())
            .and_then(require_order_recovery_update).map_err(transition_error)?;
    }
    Ok(PaymentStarted{order,client_request_id:record.client_request_id.clone(),kind,presentation,
        session_scope:worker.capture.session.clone(),record,settled})
}
fn recover_pending_order_worker(
    worker:PaymentWorker,record:PendingOrderRecord,kind:PaymentOrderKind,presentation:PaymentPresentation,
)->std::result::Result<PaymentStarted,ApiError>{
    worker.ensure()?;
    if record.owner_user_id!=worker.capture.session.owner_user_id{return Err(ApiError::AuthenticationRequired);}
    let order=if record.order_id.is_empty(){
        if record.kind=="membership_upgrade" && record.upgrade_quote_id.is_empty(){
            return Err(ApiError::LocalState{message:MISSING_ORIGINAL_UPGRADE_QUOTE.into()});
        }
        let replay=SavedReplayRequest::order(worker.capture.authority.clone(),&worker.capture.session,&record.client_request_id)
            .map_err(transition_error)?;
        worker.capture.backend.api.replay_saved(&replay)?.data
    }else{
        PaymentApi::new(worker.capture.backend.api.clone()).order_scoped(&record.order_id,&worker.capture.session)?
    };
    bind_and_settle_order(&worker,record,order,kind,presentation)
}
fn sync_payment_worker(worker:PaymentWorker,key:String,order_id:String,kind:PaymentOrderKind,presentation:PaymentPresentation)
    ->std::result::Result<PaymentStarted,ApiError>{
    worker.ensure()?;
    let record=find_saved_payment(&worker.capture,&key)?;
    if record.order_id!=order_id{return Err(ApiError::LocalState{message:"原始订单编号已变化，记录已保留".into()});}
    worker.ensure()?;
    let order=PaymentApi::new(worker.capture.backend.api.clone()).sync_order_scoped(&order_id,&worker.capture.session)?;
    bind_and_settle_order(&worker,record,order,kind,presentation)
}

#[derive(Clone,Copy)]
enum PaymentPoll { Initial{launch:bool}, Sync{attempt:u32} }
fn poll_payment_result(
    weak:Weak<AppWindow>,context:AppContext,capture:PaymentCapture,key:String,payer:String,
    kind:PaymentOrderKind,presentation:PaymentPresentation,poll:PaymentPoll,
    receiver:mpsc::Receiver<std::result::Result<PaymentStarted,ApiError>>,
){
    slint::Timer::single_shot(Duration::from_millis(100),move||{
        reap_payment_workers();
        if payment_worker_pending(&capture,&key){
            poll_payment_result(weak,context,capture,key,payer,kind,presentation,poll,receiver);return;
        }
        let result=match receiver.try_recv(){
            Ok(result)=>result,
            Err(TryRecvError::Empty)=>{
                poll_payment_result(weak,context,capture,key,payer,kind,presentation,poll,receiver);return;
            }
            Err(TryRecvError::Disconnected)=>Err(ApiError::Protocol{message:"支付任务已中断，原始订单记录已保留".into(),request_id:None}),
        };
        let Some(app)=weak.upgrade()else{return;};
        match result {
            Ok(started)=>continue_payment_order(&app,context,capture,started,poll),
            Err(error)=>{
                if let PaymentPoll::Sync{attempt}=poll {
                    if payment_error_is_transient(&error) && attempt<200 && capture.is_current(&context){
                        if let Ok(saved)=find_saved_payment(&capture,&key){
                            poll_payment_order(app.as_weak(),context,capture,saved,kind,presentation,attempt+1);return;
                        }
                    }
                }
                report_payment_error(&app,&context,&capture,&key,&payer,kind,error);
            }
        }
    });
}
fn poll_payment_order(
    weak:Weak<AppWindow>,context:AppContext,capture:PaymentCapture,record:PendingOrderRecord,
    kind:PaymentOrderKind,presentation:PaymentPresentation,attempt:u32,
){
    slint::Timer::single_shot(Duration::from_secs(3),move||{
        reap_payment_workers();
        if !capture.is_current(&context) || !context.recovering_orders.borrow().contains(&recovering_order_key(&capture.session,&record.client_request_id)){return;}
        let Some(app)=weak.upgrade()else{return;};
        let key=record.client_request_id.clone();let order_id=record.order_id.clone();
        let worker_key=key.clone();let worker_presentation=presentation.clone();
        match spawn_payment_thread(&context,&capture,&key,move|worker|sync_payment_worker(worker,worker_key,order_id,kind,worker_presentation)){
            Ok(receiver)=>poll_payment_result(app.as_weak(),context,capture,key,record.billing_account_group_id,kind,presentation,PaymentPoll::Sync{attempt},receiver),
            Err(error)=>report_payment_error(&app,&context,&capture,&key,&record.billing_account_group_id,kind,error),
        }
    });
}
fn continue_payment_order(app:&AppWindow,context:AppContext,capture:PaymentCapture,started:PaymentStarted,poll:PaymentPoll){
    if !capture.is_current(&context) || started.session_scope!=capture.session{return;}
    let key=started.client_request_id.clone();let payer=started.record.billing_account_group_id.clone();
    let phase=payment_order_phase(&started.order);
    if started.settled {
        let matched=payment_matches(&context,&capture.session,&key);
        release_payment_tracking(&context,&capture,&key);
        let visible=apply_payment_ui(app,&context,&capture,&payer,|state|{
            let settled_presentation = if started.kind == PaymentOrderKind::Credit { PaymentPresentation::credit_with_recharge(&started.presentation.credit_fallback_total, started.order.credit_recharge.as_ref()) } else { started.presentation.clone() };
            apply_payment_presentation(state,started.kind,&settled_presentation);
            if matched {
                state.set_payment_active(false);state.set_payment_browser_ready(false);
                if phase==PaymentOrderPhase::Fulfilled {
                    state.set_payment_dialog_mode("success".into());state.set_payment_dialog_open(true);
                    state.set_payment_status_message("支付成功".into());
                }else{state.set_payment_status_message("订单已关闭或过期".into());}
            }
            let message=if phase==PaymentOrderPhase::Fulfilled{
                if started.kind==PaymentOrderKind::Credit{"支付成功，积分已到账"}else{state.set_membership_open(false);"支付成功，会员权益已更新"}
            }else{"订单已关闭或过期，请重新发起支付"};
            set_payment_kind_status(state,started.kind,false,message);
        }).is_some();
        if visible && phase==PaymentOrderPhase::Fulfilled {
            // These follow-ups acquire their own current captured authority.
            refresh_backend_snapshot(app,context.clone());refresh_server_notifications(app,context);
        }
        return;
    }
    let checkout=started.order.payment.as_ref().and_then(|payment|payment.checkout_url.clone());
    if phase==PaymentOrderPhase::PendingPayment && checkout.is_none(){
        report_payment_error(app,&context,&capture,&key,&payer,started.kind,ApiError::LocalState{
            message:"暂时无法获取支付宝支付地址".into()});return;
    }
    let present=payment_matches(&context,&capture.session,&key);
    if present {
        apply_payment_ui(app,&context,&capture,&payer,|state|{
            if let Some(active)=context.active_payment.borrow_mut().as_mut(){
                if active.client_request_id==key && active.session_scope==capture.session {active.checkout_url=checkout.clone();}
            }
            apply_payment_presentation(state,started.kind,&started.presentation);
            state.set_payment_active(true);state.set_payment_browser_ready(checkout.is_some());
            let message=if phase==PaymentOrderPhase::PaidFulfilling{"付款已确认，正在等待权益生效..."}else{"已恢复未完成订单，可重新打开支付宝继续支付"};
            state.set_payment_status_message(message.into());set_payment_kind_status(state,started.kind,true,message);
        });
        if matches!(poll,PaymentPoll::Initial{launch:true}) && checkout.is_some(){
            schedule_payment_checkout(app.as_weak(),context.clone(),capture.clone(),key.clone(),payer.clone());
        }
    }
    let attempt=match poll{PaymentPoll::Sync{attempt}=>attempt,_=>0};
    if attempt>=200{
        report_payment_error(app,&context,&capture,&key,&payer,started.kind,ApiError::LocalState{message:PAYMENT_STATUS_UNAVAILABLE.into()});
    }else{
        poll_payment_order(app.as_weak(),context,capture,started.record,started.kind,started.presentation,attempt);
    }
}

pub(super) fn wire_payment_callbacks(app:&AppWindow,context:AppContext){
    if context.backend.is_none(){return;}
    let state=app.global::<AppState>();
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_retry_payment_browser(move||{
            let Some(app)=weak.upgrade()else{return;};let Ok(capture)=PaymentCapture::new(&context)else{return;};
            let active=context.active_payment.borrow().clone();
            if let Some(active)=active {
                if active.session_scope==capture.session {launch_payment_checkout(&app,&context,&capture,&active.client_request_id,&active.billing_account_group_id);}
            }else{recover_pending_orders(&app,context.clone());}
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_dismiss_payment(move||{
            let Some(app)=weak.upgrade()else{return;};let Ok(capture)=PaymentCapture::new(&context)else{return;};
            let active=context.active_payment.borrow().clone();
            if let Some(active)=active {
                apply_payment_ui(&app,&context,&capture,&active.billing_account_group_id,|state|{
                    if payment_matches(&context,&capture.session,&active.client_request_id){
                        // Hide presentation only; background order confirmation and
                        // its retained intent remain owned by the original lease.
                        context.active_payment.borrow_mut().take();
                        state.set_payment_active(false);state.set_payment_dialog_open(false);state.set_payment_browser_ready(false);
                        state.set_credit_payment_busy(false);state.set_membership_payment_busy(false);
                        state.set_payment_status_message("支付窗口已隐藏，原始订单仍会保留并确认".into());
                    }
                });
            }
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_confirm_payment_success(move||{
            let Some(app)=weak.upgrade()else{return;};let Ok(capture)=PaymentCapture::new(&context)else{return;};
            capture.apply(&context,||close_payment_success(&app.global::<AppState>()));
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_recharge_credits(move|code|{
            let Some(app)=weak.upgrade()else{return;};let Ok(capture)=PaymentCapture::new(&context)else{return;};
            if recover_before_new_purchase(&app,&context,&capture,PaymentOrderKind::Credit){return;}
            match context.capture_billing_action(KnownCapability::Purchase){
                Ok((scope,authority,activity))=>{
                    drop(activity);
                    start_credit_order_with_billing_scope(&app,context.clone(),capture.backend.clone(),authority,&scope,code.to_string());
                }
                Err(error)=>{capture.apply(&context,||app.global::<AppState>().set_credit_payment_message(error.user_message().into()));}
            }
        });
    }
    {
        let weak=app.as_weak();
        state.on_purchase_membership(move|code|{
            let Some(app)=weak.upgrade()else{return;};let Ok(capture)=PaymentCapture::new(&context)else{return;};
            if recover_before_new_purchase(&app,&context,&capture,PaymentOrderKind::Membership){return;}
            match context.capture_billing_action(KnownCapability::Purchase){
                Ok((scope,authority,activity))=>{
                    drop(activity);
                    start_membership_order_with_billing_scope(&app,context.clone(),capture.backend.clone(),authority,&scope,code.to_string());
                }
                Err(error)=>{capture.apply(&context,||app.global::<AppState>().set_membership_payment_message(error.user_message().into()));}
            }
        });
    }
}
fn recover_before_new_purchase(app:&AppWindow,context:&AppContext,capture:&PaymentCapture,kind:PaymentOrderKind)->bool{
    if new_payment_work_pending(context,capture){
        let payer=context.billing_context.confirmed_scope().map(|scope|scope.request.account_group_id);
        if let Some(payer)=payer {
            apply_payment_ui(app,context,capture,&payer,|state|set_payment_kind_status(state,kind,true,"原始付款请求仍在确认中，请勿重复下单"));
        }
        recover_pending_orders(app,context.clone());
        return true;
    }
    match load_pending_orders_for_namespace(&capture.authority){
        Ok(rows) if rows.is_empty()=>false,
        Ok(_)=>{recover_pending_orders(app,context.clone());true}
        Err(_)=>{
            capture.apply(context,||set_payment_kind_status(&app.global::<AppState>(),kind,false,"未完成订单记录无法读取，原文件已保留，请勿重复下单"));
            true
        }
    }
}
fn show_payment_recovery(app:&AppWindow,context:&AppContext,capture:&PaymentCapture,record:&PendingOrderRecord,kind:PaymentOrderKind,presentation:&PaymentPresentation){
    apply_payment_ui(app,context,capture,&record.billing_account_group_id,|state|{
        if context.active_payment.borrow().as_ref().is_some_and(|active|
            active.client_request_id!=record.client_request_id || active.session_scope!=capture.session){return;}
        if payment_matches(context,&capture.session,&record.client_request_id) && context.active_payment.borrow().as_ref().is_some_and(|active|active.checkout_url.is_some()) {
            // Billing-only clear resets all of these fields but preserves the
            // original active slot/checkout. Restore it without waiting for I/O.
            apply_payment_presentation(state,kind,presentation);
            state.set_payment_dialog_open(true);state.set_payment_dialog_mode("waiting".into());
            state.set_payment_active(true);state.set_payment_browser_ready(true);
            let message="已恢复未完成订单，可重新打开支付宝继续支付";
            state.set_payment_status_message(message.into());set_payment_kind_status(state,kind,true,message);
            return;
        }
        begin_payment_session(state,context,&record.client_request_id,kind,presentation,capture.session.clone(),record.billing_account_group_id.clone(),
            "检测到未完成订单，正在恢复原始付款请求...");
        set_payment_kind_status(state,kind,true,"检测到未完成订单，正在恢复原始付款请求...");
    });
}
pub(super) fn recover_pending_orders(app:&AppWindow,context:AppContext){
    let Ok(capture)=PaymentCapture::new(&context)else{return;};
    let records=match load_pending_orders_for_namespace(&capture.authority){
        Ok(records)=>records,
        Err(_)=>{capture.apply(&context,||app.global::<AppState>().set_credit_payment_message("订单记录已保留，但暂时无法读取".into()));return;}
    };
    for record in records {
        if record.owner_user_id!=capture.session.owner_user_id {continue;}
        let kind=pending_order_kind(&record);
        if !valid_pending_order(&record){
            apply_payment_ui(app,&context,&capture,&record.billing_account_group_id,|state|set_payment_kind_status(state,kind,false,"历史订单记录不完整，已保留，请勿重复下单"));
            continue;
        }
        let presentation=payment_presentation_for_product(&app.global::<AppState>(),kind,&record.product_code);
        show_payment_recovery(app,&context,&capture,&record,kind,&presentation);
        let key=record.client_request_id.clone();
        let inserted=capture.apply(&context,||context.recovering_orders.borrow_mut().insert(recovering_order_key(&capture.session,&key))).unwrap_or(false);
        if !inserted {continue;}
        let payer=record.billing_account_group_id.clone();let worker_presentation=presentation.clone();
        match spawn_payment_thread(&context,&capture,&key,move|worker|recover_pending_order_worker(worker,record,kind,worker_presentation)){
            Ok(receiver)=>poll_payment_result(app.as_weak(),context.clone(),capture.clone(),key,payer,kind,presentation,PaymentPoll::Initial{launch:false},receiver),
            Err(error)=>report_payment_error(app,&context,&capture,&key,&payer,kind,error),
        }
    }
}

fn start_credit_order_with_billing_scope(app:&AppWindow,context:AppContext,backend:Arc<BackendRuntime>,authority:Arc<NamespaceStorageAuthority>,scope:&BillingScope,pack:String){
    start_new_payment(app,context,backend,authority,scope,pack,PaymentOrderKind::Credit);
}
fn start_membership_order_with_billing_scope(app:&AppWindow,context:AppContext,backend:Arc<BackendRuntime>,authority:Arc<NamespaceStorageAuthority>,scope:&BillingScope,plan:String){
    start_new_payment(app,context,backend,authority,scope,plan,PaymentOrderKind::Membership);
}
fn payment_online_or_prompt(app:&AppWindow,context:&AppContext,capture:&PaymentCapture,payer:&str)->bool {
    // This completion contains only inspection and UI setters. The shared
    // require_online_operation helper also dispatches auth callbacks, so it
    // must not be called while this family's completion holds the latch.
    let decision=apply_payment_ui(app,context,capture,payer,|state|{
        if state.get_session_state().as_str()=="online" {return (true,false);}
        if state.get_session_state().as_str()=="offline" {
            state.set_generation_status("离线模式只能浏览本地内容，联网后才能购买服务".into());
            return (false,false);
        }
        state.set_generation_status("请先登录后再购买服务".into());state.set_auth_open(true);
        (false,state.get_auth_method().as_str()=="wechat" && !state.get_auth_wechat_busy() && !state.get_auth_wechat_qr_ready())
    });
    if decision==Some((false,true)) && capture.is_current(context)
        && context.billing_context.current_scope(KnownCapability::ReadGroupFinance).ok().is_some_and(|scope|
            scope.request.session==capture.session && scope.request.account_group_id==payer)
    {
        let state=app.global::<AppState>();
        if !matches!(state.get_session_state().as_str(),"online"|"offline")
            && state.get_auth_method().as_str()=="wechat" && !state.get_auth_wechat_busy() && !state.get_auth_wechat_qr_ready()
        {state.invoke_start_wechat_login();}
    }
    // Auth dispatch can change the session/namespace. Never continue this
    // purchase after prompting; a later user action must capture again.
    decision==Some((true,false))
}
fn start_new_payment(
    app:&AppWindow,context:AppContext,backend:Arc<BackendRuntime>,authority:Arc<NamespaceStorageAuthority>,
    scope:&BillingScope,product:String,kind:PaymentOrderKind,
){
    let Ok(capture)=PaymentCapture::new(&context)else{return;};
    let scoped=capture_billing_scope_for_submission(Some(&backend),&authority,scope);
    let scope=match scoped{
        Ok(scope) if authority.lease()==capture.persistence.lease()
            && scope.request.session==capture.session && context.billing_context.is_current(&scope)=>scope,
        _=>{
            capture.apply(&context,||set_payment_kind_status(&app.global::<AppState>(),kind,false,"付款账号已变化，请重新确认"));
            return;
        }
    };
    let payer=scope.request.account_group_id.clone();
    if new_payment_work_pending(&context,&capture){
        apply_payment_ui(app,&context,&capture,&payer,|state|set_payment_kind_status(state,kind,true,"原始付款请求仍在确认中，请勿重复下单"));
        return;
    }
    // This lower entry never replays: the actual callbacks run the recovery
    // front door first. A direct caller must still never create a second key.
    match load_pending_orders_for_namespace(&authority){
        Ok(rows) if rows.is_empty()=>{},
        _=>{
            apply_payment_ui(app,&context,&capture,&payer,|state|set_payment_kind_status(state,kind,false,"检测到未完成订单，记录已保留，请先恢复原始订单"));
            return;
        }
    }
    if !payment_online_or_prompt(app,&context,&capture,&payer){return;}
    if !capture.is_current(&context) || !context.billing_context.is_current(&scope){return;}
    let state=app.global::<AppState>();
    if state.get_payment_active(){
        apply_payment_ui(app,&context,&capture,&payer,|state|state.set_payment_dialog_open(true));
        return;
    }
    if (kind==PaymentOrderKind::Credit && state.get_credit_payment_busy())
        || (kind==PaymentOrderKind::Membership && state.get_membership_payment_busy()){return;}
    let acceptances=match required_purchase_acceptances(app){
        Ok(items)=>items,
        Err(message)=>{apply_payment_ui(app,&context,&capture,&payer,|state|set_payment_kind_status(state,kind,false,message));return;}
    };
    let product=product.trim().to_owned();
    if product.is_empty(){
        apply_payment_ui(app,&context,&capture,&payer,|state|set_payment_kind_status(state,kind,false,"请选择可用套餐"));return;
    }
    let mut is_upgrade=false;
    let presentation=if kind==PaymentOrderKind::Membership{
        let Some(target)=state.get_membership_plans().iter().find(|plan|plan.code.as_str()==product)else{
            apply_payment_ui(app,&context,&capture,&payer,|state|set_payment_kind_status(state,kind,false,"所选会员套餐已下线，请刷新后重试"));return;
        };
        is_upgrade=state.get_membership_tier_rank()>0 && target.tier_rank>state.get_membership_tier_rank();
        PaymentPresentation::membership(target.name.as_str())
    }else{ let total = state.get_selected_credit_total(); let total = if total.trim().is_empty() { state.get_selected_credit_amount() } else { total }; PaymentPresentation::credit(total.as_str()) };
    let key=Uuid::new_v4().simple().to_string();
    let record=PendingOrderRecord{
        schema_version:2,kind:if kind==PaymentOrderKind::Credit{"credit"}else if is_upgrade{"membership_upgrade"}else{"membership"}.into(),
        client_request_id:key.clone(),owner_user_id:capture.session.owner_user_id.clone(),billing_account_group_id:payer.clone(),
        auth_epoch:capture.session.auth_epoch,order_id:String::new(),product_code:product,upgrade_quote_id:String::new(),created_at:Local::now().to_rfc3339(),
    };
    if apply_payment_ui(app,&context,&capture,&payer,|state|{
        if new_payment_work_pending(&context,&capture){return false;}
        context.recovering_orders.borrow_mut().insert(recovering_order_key(&capture.session,&key));
        begin_payment_session(state,&context,&key,kind,&presentation,capture.session.clone(),payer.clone(),"正在创建原始付款订单...");
        set_payment_kind_status(state,kind,true,"正在创建原始付款订单...");
        true
    })!=Some(true){return;}
    let worker_presentation=presentation.clone();
    let receiver=spawn_payment_thread(&context,&capture,&key,move|worker|{
        create_payment_worker(worker,scope,record,kind,worker_presentation,acceptances)
    });
    match receiver{
        Ok(receiver)=>poll_payment_result(app.as_weak(),context,capture,key,payer,kind,presentation,PaymentPoll::Initial{launch:true},receiver),
        Err(error)=>report_payment_error(app,&context,&capture,&key,&payer,kind,error),
    }
}
fn create_payment_worker(
    worker:PaymentWorker,scope:BillingScope,record:PendingOrderRecord,kind:PaymentOrderKind,
    presentation:PaymentPresentation,acceptances:Vec<AgreementAcceptance>,
)->std::result::Result<PaymentStarted,ApiError>{
    worker.ensure()?;
    #[cfg(test)]
    let worker={let mut worker=worker;if let Some(before)=worker.before_create_record.take(){before();}worker};
    worker.ensure()?;
    upsert_pending_order_for_namespace(&worker.capture.authority,&scope,record.clone()).map_err(transition_error)?;
    let result=(||{
        worker.ensure()?;
        AuthApi::new(worker.capture.backend.api.clone()).accept_agreements_scoped(&acceptances,&worker.capture.session)?;
        worker.ensure()?;
        let order=match record.kind.as_str(){
            "credit"=>PaymentApi::new(worker.capture.backend.api.clone()).create_credit_order_billing(&record.product_code,&record.client_request_id,&scope)?,
            "membership"=>MembershipApi::new(worker.capture.backend.api.clone()).create_order_billing(&record.product_code,&record.client_request_id,&scope)?,
            "membership_upgrade"=>create_upgrade_order_checked(
                &MembershipApi::new(worker.capture.backend.api.clone()),&worker.capture.backend,&worker.capture.authority,
                &record.identity(),&scope,&record.product_code,&record.client_request_id,||worker.ensure())?,
            _=>return Err(ApiError::LocalState{message:"未知订单类型，原始记录已保留".into()}),
        };
        bind_and_settle_order(&worker,record.clone(),order,kind,presentation)
    })();
    if let Err(error)=&result{
        if !payment_error_preserves_order_recovery(error) && worker.ensure().is_ok(){
            let current=find_saved_payment(&worker.capture,&record.client_request_id)?;
            if current.identity()==record.identity() && current.order_id.is_empty(){
                remove_pending_order_for_namespace(&worker.capture.authority,&current.identity())
                    .and_then(require_order_recovery_update).map_err(transition_error)?;
            }
        }
    }
    result
}
fn create_upgrade_order_checked(
    api:&MembershipApi,backend:&BackendRuntime,authority:&NamespaceStorageAuthority,identity:&RecoveryRecordIdentity,
    billing_scope:&BillingScope,plan_code:&str,request_id:&str,ensure:impl Fn()->std::result::Result<(),ApiError>,
)->std::result::Result<OrderDetail,ApiError>{
    ensure()?;
    let quote=api.create_upgrade_quote_billing(plan_code,request_id,billing_scope)?;
    ensure()?;
    ensure_payment_scope_active(backend,&billing_scope.request.session)?;
    update_pending_order_quote_id_for_namespace(authority,identity,&quote.id)
        .and_then(require_order_recovery_update).map_err(transition_error)?;
    ensure()?;
    api.create_upgrade_order_billing(&quote.id,request_id,billing_scope)
}
#[cfg(test)]
fn create_upgrade_order_with_saved_quote(
    api:&MembershipApi,backend:&BackendRuntime,authority:&NamespaceStorageAuthority,identity:&RecoveryRecordIdentity,
    billing_scope:&BillingScope,plan_code:&str,request_id:&str,
)->std::result::Result<OrderDetail,ApiError>{
    create_upgrade_order_checked(api,backend,authority,identity,billing_scope,plan_code,request_id,
        ||ensure_payment_scope_active(backend,&billing_scope.request.session))
}
fn require_order_recovery_update(updated:bool)->Result<()>{
    if updated{Ok(())}else{Err(RecoveryError::IdentityChanged.into())}
}

fn schedule_payment_checkout(weak:Weak<AppWindow>,context:AppContext,capture:PaymentCapture,key:String,payer:String){
    slint::Timer::single_shot(Duration::from_millis(16),move||{
        if let Some(app)=weak.upgrade(){launch_payment_checkout(&app,&context,&capture,&key,&payer);}
    });
}
fn launch_payment_checkout(app:&AppWindow,context:&AppContext,capture:&PaymentCapture,key:&str,payer:&str){
    if !capture.is_current(context){return;}
    let Some(visible)=context.billing_context.current_scope(KnownCapability::ReadGroupFinance).ok()else{return;};
    if visible.request.session!=capture.session || visible.request.account_group_id!=payer{return;}
    let active=context.active_payment.borrow().clone();
    let Some(active)=active.filter(|active|active.client_request_id==key && active.session_scope==capture.session
        && active.billing_account_group_id==payer)else{return;};
    // Re-read the retained original order through the held authority, outside the
    // short UI lock. An arbitrary checkout string is not its own order authority.
    let Ok(record)=find_saved_payment(capture,key)else{return;};
    if record.billing_account_group_id!=payer || record.order_id.is_empty(){return;}
    let Some(checkout)=active.checkout_url else{
        apply_payment_ui(app,context,capture,payer,|state|state.set_payment_status_message("支付地址尚未准备好，请稍候".into()));return;
    };
    if !capture.is_current(context) || !context.billing_context.is_current(&visible){return;}
    let result=open_payment_checkout(&checkout,capture.backend.api.base_url(),&capture.persistence,&record.order_id);
    // Native launch and effect/activity Drop have finished before re-entry.
    if context.billing_context.is_current(&visible) && payment_matches(context,&capture.session,key){
        apply_payment_ui(app,context,capture,payer,|state|{
            state.set_payment_dialog_open(true);
            state.set_payment_status_message(if result.is_ok(){state.get_payment_waiting_message()}else{"无法打开系统浏览器，请点击“重新打开支付宝”重试".into()});
        });
    }
}
fn begin_payment_session(
    state: &AppState,
    context: &AppContext,
    client_request_id: &str,
    kind: PaymentOrderKind,
    presentation: &PaymentPresentation,
    session_scope: SessionScope,
    billing_account_group_id: String,
    message: &str,
) {
    *context.active_payment.borrow_mut() = Some(ActivePaymentSession {
        client_request_id: client_request_id.to_string(),
        billing_account_group_id,
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


fn close_payment_success(state: &AppState) {
    state.set_payment_dialog_open(false);
    state.set_payment_dialog_mode("waiting".into());
    state.set_payment_status_message("".into());
    state.set_payment_success_message("".into());
    state.set_payment_success_detail("".into());
}


pub(super) fn clear_payment_account_state(app: &AppWindow, context: &AppContext) {
    if let Some(persistence)=context.store.borrow().private_persistence.as_ref(){
        cancel_payment_workers_for_retirement(persistence.lease());
    }
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
            credit_recharge: None,
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
    fn payment_success_prefers_server_recharge_total() {
        let recharge = CreditRechargeSummary {
            base_credits: Some("10000".into()), bonus_credits: Some("2000".into()),
            total_credits: Some("12000".into()), pack_code: Some("pack_10000".into()),
            promotion_id: Some("mid-autumn".into()), promotion_ends_at: None,
        };
        let presentation = PaymentPresentation::credit_with_recharge("10000", Some(&recharge));
        assert_eq!(presentation.success_message, "12000 积分已到账");
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

#[cfg(test)]
struct JoinedPaymentFixtureThread<T: Send+'static>(Option<std::thread::JoinHandle<T>>);
#[cfg(test)]
impl<T: Send+'static> JoinedPaymentFixtureThread<T> {
    fn join(mut self)->std::thread::Result<T> {self.0.take().unwrap().join()}
}
#[cfg(test)]
impl<T: Send+'static> Drop for JoinedPaymentFixtureThread<T> {
    fn drop(&mut self) {
        if let Some(handle)=self.0.take() {
            let joined=handle.join();
            if !std::thread::panicking(){assert!(joined.is_ok(),"payment fixture thread panicked");}
        }
    }
}
#[cfg(test)]
fn spawn_payment_fixture_thread<T:Send+'static>(work:impl FnOnce()->T+Send+'static)->JoinedPaymentFixtureThread<T> {
    JoinedPaymentFixtureThread(Some(std::thread::spawn(work)))
}

#[cfg(test)]
mod billing_capture_tests {
    use super::*;
    use backend_generation::billing_capture_test_support::*;
    use super::core_payment_tests::fixture;

    fn accept_recorded_request(listener: &std::net::TcpListener) -> (std::net::TcpStream, String) {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let request = read_request(&mut stream);
                    stream
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    return (stream, request);
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("billing request was not observed: {error}"),
            }
        }
    }

    fn respond_json(
        stream: &mut std::net::TcpStream,
        status: &str,
        data: serde_json::Value,
        error: serde_json::Value,
    ) {
        use std::io::Write;
        let body = serde_json::json!({
            "request_id": "upgrade-quote-fixture",
            "data": data,
            "error": error,
            "meta": null
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    }

    fn request_header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
        request
            .split("\r\n\r\n")
            .next()
            .unwrap()
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
    }

    fn assert_request_scope_and_key(request: &str, request_id: &str) {
        assert_eq!(request_header(request, "x-account-group-id"), Some(PAYER));
        assert_eq!(request_header(request, "x-token"), Some("capture-access"));
        assert_eq!(
            request_header(request, "idempotency-key"),
            Some(request_id)
        );
    }

    fn assert_upgrade_quote_request(request: &str, request_id: &str) {
        assert!(request.starts_with("POST /v1/membership/upgrade-quotes "));
        assert_request_scope_and_key(request, request_id);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                request.split("\r\n\r\n").nth(1).unwrap()
            )
            .unwrap(),
            serde_json::json!({"target_plan_code": "fixture-plan"})
        );
    }

    fn assert_upgrade_order_request(request: &str, request_id: &str, quote_id: &str) {
        assert!(request.starts_with("POST /v1/membership/upgrade-orders "));
        assert_request_scope_and_key(request, request_id);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                request.split("\r\n\r\n").nth(1).unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "quote_id": quote_id,
                "client_request_id": request_id
            })
        );
    }

    fn upgrade_record(scope: &BillingScope, request_id: &str) -> PendingOrderRecord {
        PendingOrderRecord {
            schema_version: 2,
            kind: "membership_upgrade".into(),
            client_request_id: request_id.into(),
            owner_user_id: OWNER.into(),
            billing_account_group_id: PAYER.into(),
            auth_epoch: scope.request.session.auth_epoch,
            order_id: String::new(),
            product_code: "fixture-plan".into(),
            upgrade_quote_id: String::new(),
            created_at: "fixture".into(),
        }
    }

    fn assert_single_upgrade_record(
        authority: &NamespaceStorageAuthority,
        expected: &PendingOrderRecord,
        quote_id: &str,
    ) {
        let records = load_pending_orders_for_namespace(authority).unwrap();
        assert_eq!(records.len(), 1, "purchase retry must not create another record");
        let saved = &records[0];
        assert_eq!(saved.schema_version, 2);
        assert_eq!(saved.kind, "membership_upgrade");
        assert_eq!(saved.client_request_id, expected.client_request_id);
        assert_eq!(saved.owner_user_id, OWNER);
        assert_eq!(saved.billing_account_group_id, PAYER);
        assert_eq!(saved.auth_epoch, expected.auth_epoch);
        assert_eq!(saved.order_id, "");
        assert_eq!(saved.product_code, "fixture-plan");
        assert_eq!(saved.upgrade_quote_id, quote_id);
        assert_eq!(saved.created_at, "fixture");
    }

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
    fn upgrade_quote_identity_missing_quote_record_prevents_order_dispatch() {
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
        let transport = spawn_payment_fixture_thread(move || {
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
            assert_upgrade_quote_request(&request, "upgrade-fixture");
            let persisted = load_pending_orders_for_namespace(&authority).unwrap();
            assert_eq!(persisted.len(), 1);
            assert_eq!(persisted[0].identity(), identity);
            assert_eq!(persisted[0].owner_user_id, OWNER);
            assert_eq!(persisted[0].billing_account_group_id, PAYER);
            assert_eq!(persisted[0].product_code, "fixture-plan");
            assert_eq!(persisted[0].client_request_id, "upgrade-fixture");
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

    #[test]
    fn upgrade_quote_identity_retry_reuses_saved_key_and_payer_before_order() {
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let record = upgrade_record(&fixture.scope, "upgrade-retry-fixture");
        upsert_pending_order_for_namespace(&fixture.authority, &fixture.scope, record.clone())
            .unwrap();
        let authority = fixture.authority.clone();
        let expected_record = record.clone();
        let transport = spawn_payment_fixture_thread(move || {
            let (stream, first_quote_request) = accept_recorded_request(&listener);
            assert_upgrade_quote_request(&first_quote_request, "upgrade-retry-fixture");
            assert_single_upgrade_record(&authority, &expected_record, "");
            drop(stream);

            let (mut stream, retried_quote_request) = accept_recorded_request(&listener);
            assert_upgrade_quote_request(&retried_quote_request, "upgrade-retry-fixture");
            assert_single_upgrade_record(&authority, &expected_record, "");
            respond_json(
                &mut stream,
                "200 OK",
                serde_json::json!({
                    "id": OTHER,
                    "target_plan_code": "fixture-plan",
                    "payable_amount_cents": "100",
                    "credit_delta": "10",
                    "expires_at": "2099-01-01T00:00:00Z"
                }),
                serde_json::Value::Null,
            );

            let (mut stream, order_request) = accept_recorded_request(&listener);
            assert_upgrade_order_request(&order_request, "upgrade-retry-fixture", OTHER);
            assert_single_upgrade_record(&authority, &expected_record, OTHER);
            respond_json(
                &mut stream,
                "200 OK",
                serde_json::json!({
                    "id": "upgrade-order-fixture",
                    "billing_account_group_id": PAYER,
                    "status": "pending",
                    "fulfillment_status": "pending",
                    "payable_amount_cents": "100",
                    "payment": null
                }),
                serde_json::Value::Null,
            );
        });
        let api = MembershipApi::new(fixture.backend.api.clone());

        let first_error = create_upgrade_order_with_saved_quote(
            &api,
            &fixture.backend,
            &fixture.authority,
            &record.identity(),
            &fixture.scope,
            "fixture-plan",
            &record.client_request_id,
        )
        .unwrap_err();
        assert!(first_error.is_network_error());
        assert!(payment_error_preserves_order_recovery(&first_error));
        assert_single_upgrade_record(&fixture.authority, &record, "");

        let order = create_upgrade_order_with_saved_quote(
            &api,
            &fixture.backend,
            &fixture.authority,
            &record.identity(),
            &fixture.scope,
            "fixture-plan",
            &record.client_request_id,
        )
        .unwrap();
        assert_eq!(order.id, "upgrade-order-fixture");
        assert_eq!(order.billing_account_group_id, PAYER);
        transport.join().unwrap();
        assert_single_upgrade_record(&fixture.authority, &record, OTHER);
    }

    #[test]
    fn upgrade_quote_identity_exact_426_preserves_facts_and_sends_no_order() {
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let record = upgrade_record(&fixture.scope, "upgrade-426-fixture");
        upsert_pending_order_for_namespace(&fixture.authority, &fixture.scope, record.clone())
            .unwrap();
        let authority = fixture.authority.clone();
        let expected_record = record.clone();
        let transport = spawn_payment_fixture_thread(move || {
            let (mut stream, request) = accept_recorded_request(&listener);
            assert_upgrade_quote_request(&request, "upgrade-426-fixture");
            assert_single_upgrade_record(&authority, &expected_record, "");
            respond_json(
                &mut stream,
                "426 Upgrade Required",
                serde_json::Value::Null,
                serde_json::json!({
                    "code": "client_upgrade_required",
                    "message": "upgrade required",
                    "details": null
                }),
            );
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
        assert!(error.is_client_update_required());
        assert!(payment_error_preserves_order_recovery(&error));
        let listener = transport.join().unwrap();
        assert_no_request(&listener);
        assert_single_upgrade_record(&fixture.authority, &record, "");
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

#[cfg(test)]
mod core_payment_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};

    const OWNER: &str = "11111111-1111-4111-8111-111111111111";
    const PAYER: &str = "22222222-2222-4222-8222-222222222222";
    const OTHER: &str = "33333333-3333-4333-8333-333333333333";
    const KEY: &str = "44444444-4444-4444-8444-444444444444";
    const ORDER: &str = "55555555-5555-4555-8555-555555555555";

    pub(super) struct FixtureRoot(PathBuf);
    impl FixtureRoot { pub(super) fn path(&self) -> &Path { &self.0 } }
    pub(super) struct Fixture {
        pub(super) authority: Arc<NamespaceStorageAuthority>,
        pub(super) scope: BillingScope,
        pub(super) backend: Arc<BackendRuntime>,
        pub(super) context: AppContext,
        pub(super) root: FixtureRoot,
        writer: client_state::tests::Fixture,
        persistence: PrivatePersistence,
        expected_join_failure:bool,
    }
    pub(super) fn fixture(url: &str) -> Fixture {
        let writer = client_state::tests::Fixture::new(false, false);
        let session = Arc::new(SessionManager::new(Arc::new(
            crate::runtime::test_support::MemoryRefreshTokenStore::default())));
        let session_scope = session.install_tokens_for_user(&TokenSet {
            access_token: "capture-access".into(), access_expires_in_seconds: 1800,
            refresh_token: "capture-refresh".into(), refresh_expires_at: "2099-01-01T00:00:00Z".into(),
            token_type: "X-Token".into(),
        }, OWNER).unwrap();
        let lease = writer.lease(OWNER, session_scope.auth_epoch, 1);
        writer.activate(lease.clone()).unwrap();
        let root = writer.data_root_capability_arc();
        let path = lease.namespace.root().parent().unwrap().parent().unwrap().to_path_buf();
        let authority = Arc::new(NamespaceStorageAuthority::open(root.clone(), &lease).unwrap());
        let index = FileIndex::initialize(path.join("payment-index.sqlite3")).unwrap();
        let backend = Arc::new(BackendRuntime { api: ApiClient::new(ApiClientConfig {
            base_url: reqwest::Url::parse(url).unwrap(), app_version: "999.0.0".into(),
            timeout: Duration::from_secs(3),
        }, DeviceIdentity { id: OTHER.into(), name: "payment-fixture".into(), platform: "macos".into() },
            session).unwrap() });
        let context = AppContext {
            data_root_capability: Some(root.clone()), file_index: Some(index.clone()),
            backend: Some(backend.clone()), current_user_id: Arc::new(Mutex::new(Some(OWNER.into()))),
            account_snapshot_scope: Arc::new(Mutex::new(Some(session_scope.clone()))),
            billing_context: Arc::new(BillingContextManager::with_upgrade_latch(backend.api.upgrade_latch().clone())),
            ..Default::default()
        };
        context.user_activity.activate(lease.clone()).unwrap();
        *context.active_namespace.lock().unwrap() = Some(lease.clone());
        backend.api.bind_user_work(UserWorkAdmission::new(context.active_namespace.clone(), context.user_activity.clone())).unwrap();
        let transition = context.namespace_operations.try_begin_transition().unwrap();
        let phase = transition.begin_prepublication_recovery(&lease).unwrap();
        phase.verify_no_unsupported_imports(&authority).unwrap();
        let proof = phase.finish().unwrap();
        transition.prepare_publication(&lease, proof).unwrap().publish();
        context.billing_context.bind_authenticated_session(session_scope.clone()).unwrap();
        let persistence = PrivatePersistence::for_test_with_storage(
            (*writer).clone(), lease, context.user_activity.clone(), backend.api.upgrade_latch().clone(),
            root, backend.api.clone(), index);
        context.store.borrow_mut().private_persistence = Some(persistence.clone());
        select(&context, &session_scope, PAYER, true);
        let scope = context.billing_context.current_scope(KnownCapability::Purchase).unwrap();
        let authority = persistence.storage_authority().unwrap();
        Fixture { authority, scope, backend, context, root: FixtureRoot(path), writer, persistence, expected_join_failure:false }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let joined = join_payment_workers();
            if !std::thread::panicking() { assert_eq!(joined.is_err(),self.expected_join_failure,"payment worker join outcome"); }
        }
    }
    fn select(context: &AppContext, session: &SessionScope, payer: &str, owner: bool) {
        let caps = if owner { vec!["bill","purchase","read_group_finance"] } else { vec!["bill"] };
        let snapshot: AccountSnapshot = serde_json::from_value(serde_json::json!({
            "user":{"id":OWNER,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
            "read_only":false,"capabilities":caps,"membership":null,"entitlement":{},"credits":null,"quota":null,
            "billing_group":{"group_id":payer,"name":"Fixture","group_status":"active","role":if owner {"owner"}else{"member"},
                "member_id":if owner {None}else{Some("66666666-6666-4666-8666-666666666666")},
                "relationship_status":if owner {None}else{Some("active")},"readable_context":true,"selectable":true,
                "group_version":"1","membership_version":if owner {None}else{Some("1")},"capabilities":caps,"quota":null}
        })).unwrap();
        let ticket = context.billing_context.begin_switch(session, "fixture", payer, PreviousBillingAuthority::StillValid).unwrap();
        let staged = context.billing_context.stage_confirmation(&ticket, snapshot.billing_group.clone(), snapshot).unwrap();
        context.billing_context.publish_persisted(ticket, staged);
    }
    fn app(fixture: &Fixture) -> AppWindow {
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_session_state("online".into());
        state.set_purchase_membership_required(false);
        state.set_purchase_credit_rules_required(false);
        state.set_membership_plans(ModelRc::new(VecModel::from(vec![MembershipPlanView {
            code:"fixture-plan".into(),name:"Fixture".into(),price:"100".into(),grant_credits:"10".into(),
            period_days:30,tier_rank:1,
        }])));
        wire_payment_callbacks(&app, fixture.context.clone());
        app
    }
    fn record(f: &Fixture, kind: &str, order_id: &str) -> PendingOrderRecord {
        PendingOrderRecord { schema_version:2,kind:kind.into(),client_request_id:KEY.into(),owner_user_id:OWNER.into(),
            billing_account_group_id:PAYER.into(),auth_epoch:f.scope.request.session.auth_epoch,order_id:order_id.into(),
            product_code:if kind=="credit" {"fixture-pack".into()}else{"fixture-plan".into()},
            upgrade_quote_id:String::new(),created_at:"fixture".into() }
    }
    fn seed(f: &Fixture, row: PendingOrderRecord) {
        upsert_pending_order_for_namespace(&f.authority, &f.scope, row).unwrap();
    }
    fn rows(f: &Fixture) -> Vec<PendingOrderRecord> { load_pending_orders_for_namespace(&f.authority).unwrap() }
    fn response(status: u16, data: Value, code: &str) -> String {
        let body = serde_json::json!({"request_id":"fixture","data":data,"meta":null,
            "error":if status==200 {Value::Null}else{serde_json::json!({"code":code,"message":"controlled failure","details":null})}}).to_string();
        format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())
    }
    fn order_response(status: &str, fulfillment: &str) -> String {
        response(200, serde_json::json!({"id":ORDER,"billing_account_group_id":PAYER,
            "status":status,"fulfillment_status":fulfillment,"payable_amount_cents":"100","payment":null}), "")
    }
    fn pump_for(duration: Duration) {
        let deadline = Instant::now()+duration;
        while Instant::now()<deadline {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn pump(mut ready: impl FnMut()->bool) {
        let deadline=Instant::now()+Duration::from_secs(5);
        while !ready() && Instant::now()<deadline { pump_for(Duration::from_millis(5)); }
        assert!(ready(),"payment completion was not observed");
    }

    struct Transport {
        url:String, seen:mpsc::Receiver<usize>, replies:Vec<Option<mpsc::Sender<String>>>,
        stop:Arc<AtomicBool>, handle:Option<std::thread::JoinHandle<Vec<String>>>,
    }
    impl Transport {
        fn new(slots:usize)->Self {
            let listener=TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url=format!("http://{}/",listener.local_addr().unwrap());
            let stop=Arc::new(AtomicBool::new(false)); let worker_stop=stop.clone();
            let (seen_tx,seen)=mpsc::channel(); let mut replies=Vec::new(); let mut receivers=Vec::new();
            for _ in 0..slots { let(tx,rx)=mpsc::channel(); replies.push(Some(tx));receivers.push(Some(rx)); }
            let handle=std::thread::spawn(move || {
                let deadline=Instant::now()+Duration::from_secs(12); let mut children=Vec::new();
                while !worker_stop.load(Ordering::Acquire) && Instant::now()<deadline {
                    match listener.accept() {
                        Ok((mut stream,_)) => {
                            let index=children.len();let reply=receivers.get_mut(index).and_then(Option::take);
                            let seen_tx=seen_tx.clone();
                            children.push(std::thread::spawn(move || {
                                stream.set_nonblocking(false).unwrap();
                                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                                stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
                                let mut bytes=Vec::new();let mut byte=[0u8;1];
                                while bytes.len()<16384 && !bytes.ends_with(b"\r\n\r\n") {
                                    if stream.read(&mut byte).unwrap_or(0)==0 {break;} bytes.push(byte[0]);
                                }
                                let header=String::from_utf8(bytes.clone()).unwrap();
                                assert!(header.ends_with("\r\n\r\n"),"bounded payment headers missing");
                                let length=header.lines().filter_map(|line|line.split_once(':'))
                                    .find(|(name,_)|name.eq_ignore_ascii_case("content-length"))
                                    .map(|(_,value)|value.trim().parse::<usize>().unwrap()).unwrap_or(0);
                                assert!(length<=16384,"bounded payment body exceeded");
                                let mut body=vec![0;length];stream.read_exact(&mut body).unwrap();bytes.extend(body);
                                seen_tx.send(index).unwrap();
                                let value=reply.and_then(|rx|rx.recv_timeout(Duration::from_secs(5)).ok())
                                    .unwrap_or_else(||response(500,Value::Null,"fixture_failure"));
                                let _=stream.write_all(value.as_bytes());
                                String::from_utf8(bytes).unwrap()
                            }));
                        }
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
                        Err(_)=>panic!("fixture listener failed"),
                    }
                }
                let mut requests=Vec::new();let mut failed=false;
                for child in children {match child.join(){Ok(request)=>requests.push(request),Err(_)=>failed=true}}
                assert!(!failed,"fixture connection panicked");requests
            });
            Self{url,seen,replies,stop,handle:Some(handle)}
        }
        fn wait(&self){self.seen.recv_timeout(Duration::from_secs(4)).expect("payment request was not dispatched");}
        fn reply(&mut self,index:usize,response:String){self.replies[index].take().unwrap().send(response).unwrap();}
        fn finish(mut self)->Vec<String>{
            self.stop.store(true,Ordering::Release);for reply in &mut self.replies{reply.take();}
            self.handle.take().unwrap().join().expect("fixture transport panicked")
        }
    }
    impl Drop for Transport {
        fn drop(&mut self){
            self.stop.store(true,Ordering::Release);for reply in &mut self.replies{reply.take();}
            if let Some(handle)=self.handle.take(){let joined=handle.join();if !std::thread::panicking(){assert!(joined.is_ok(),"fixture transport panicked");}}
        }
    }
    struct JoinedTrip(Option<std::thread::JoinHandle<()>>);
    impl JoinedTrip {
        fn start(latch:UpgradeLatch)->Self {
            let observed=latch.clone();
            let trip=Self(Some(std::thread::spawn(move||latch.trip(RequiredUpgrade{minimum_version:Some("99.0.0".into())}))));
            let deadline=Instant::now()+Duration::from_secs(3);
            while !observed.is_tripped() && Instant::now()<deadline {std::thread::sleep(Duration::from_millis(1));}
            assert!(observed.is_tripped(),"upgrade admission did not close");trip
        }
        fn join(mut self){self.0.take().unwrap().join().expect("fixture trip panicked");}
    }
    impl Drop for JoinedTrip {
        fn drop(&mut self){if let Some(handle)=self.0.take(){let joined=handle.join();if !std::thread::panicking(){assert!(joined.is_ok(),"fixture trip panicked");}}}
    }

    #[test]
    fn core_payment_saved_a_terminal_while_b_settles_without_b_finance_ui() {
        i_slint_backend_testing::init_no_event_loop();
        for (status,fulfillment) in [("paid","fulfilled"),("closed","pending")] {
            let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
            seed(&f,record(&f,"credit",ORDER));
            recover_pending_orders(&app,f.context.clone());transport.wait();
            select(&f.context,&f.scope.request.session,OTHER,false);
            let state=app.global::<AppState>();
            state.set_credit_balance("B-private".into());state.set_credit_payment_message("B-state".into());
            state.set_payment_status_message("B-status".into());state.set_payment_dialog_open(false);
            transport.reply(0,order_response(status,fulfillment));join_payment_workers().unwrap();
            pump_for(Duration::from_millis(50));
            assert!(rows(&f).is_empty(),"confirmed saved A order must settle while B is selected");
            assert!(!f.context.recovering_orders.borrow().contains(&recovering_order_key(&f.scope.request.session,KEY)));
            assert_eq!(state.get_credit_balance(),"B-private");assert_eq!(state.get_credit_payment_message(),"B-state");
            assert_eq!(state.get_payment_status_message(),"B-status");assert!(!state.get_payment_dialog_open());
            let requests=transport.finish();assert_eq!(requests.len(),1);
            assert!(requests[0].starts_with(&format!("GET /v1/orders/{ORDER} ")));
            assert!(!requests[0].to_ascii_lowercase().contains("x-account-group-id:"));
        }
    }
    #[test]
    fn core_payment_return_to_a_recharge_recovers_original_order_without_new_key() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        seed(&f,record(&f,"credit",ORDER));
        select(&f.context,&f.scope.request.session,OTHER,false);
        select(&f.context,&f.scope.request.session,PAYER,true);
        app.global::<AppState>().invoke_recharge_credits("different-new-pack".into());
        transport.wait();assert_eq!(rows(&f)[0].client_request_id,KEY);
        transport.reply(0,order_response("closed","pending"));join_payment_workers().unwrap();
        pump(||rows(&f).is_empty());
        let requests=transport.finish();assert_eq!(requests.len(),1);
        assert!(requests[0].starts_with(&format!("GET /v1/orders/{ORDER} ")));
    }
    #[test]
    fn core_payment_missing_saved_upgrade_quote_is_blocked_and_retained() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        let saved=record(&f,"membership_upgrade","");seed(&f,saved.clone());
        app.global::<AppState>().invoke_purchase_membership("fixture-plan".into());
        join_payment_workers().unwrap();pump_for(Duration::from_millis(30));
        assert_eq!(rows(&f)[0].identity(),saved.identity());assert!(rows(&f)[0].upgrade_quote_id.is_empty());
        assert!(app.global::<AppState>().get_membership_payment_message().contains("原始升级报价"));
        assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_payment_late_create_after_upgrade_keeps_record_and_ui_boundary() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        app.global::<AppState>().invoke_recharge_credits("fixture-pack".into());transport.wait();
        let saved=rows(&f);assert_eq!(saved.len(),1);
        let trip=JoinedTrip::start(f.backend.api.upgrade_latch().clone());
        app.global::<AppState>().set_credit_payment_message("upgrade boundary".into());
        app.global::<AppState>().set_payment_status_message("upgrade boundary".into());
        transport.reply(0,order_response("closed","pending"));trip.join();join_payment_workers().unwrap();
        pump_for(Duration::from_millis(40));
        assert_eq!(rows(&f)[0].client_request_id,saved[0].client_request_id);
        assert_eq!(app.global::<AppState>().get_credit_payment_message(),"upgrade boundary");
        assert_eq!(app.global::<AppState>().get_payment_status_message(),"upgrade boundary");
        assert_eq!(transport.finish().len(),1);
    }
    #[test]
    fn core_payment_missing_store_authority_rejects_callbacks_before_mutation() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        f.context.store.borrow_mut().private_persistence=None;
        let state=app.global::<AppState>();state.set_payment_dialog_open(true);state.set_payment_dialog_mode("success".into());
        state.set_credit_payment_message("unchanged".into());state.set_membership_payment_message("unchanged".into());
        state.invoke_confirm_payment_success();state.invoke_dismiss_payment();
        state.invoke_recharge_credits("fixture-pack".into());state.invoke_purchase_membership("fixture-plan".into());
        join_payment_workers().unwrap();
        assert!(state.get_payment_dialog_open());assert_eq!(state.get_payment_dialog_mode(),"success");
        assert_eq!(state.get_credit_payment_message(),"unchanged");assert_eq!(state.get_membership_payment_message(),"unchanged");
        assert!(rows(&f).is_empty());assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_payment_saved_denial_keeps_exact_original_row() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        let saved=record(&f,"credit","");seed(&f,saved.clone());
        select(&f.context,&f.scope.request.session,OTHER,false);
        recover_pending_orders(&app,f.context.clone());transport.wait();
        transport.reply(0,response(403,Value::Null,"account_group_frozen"));join_payment_workers().unwrap();
        pump_for(Duration::from_millis(30));
        assert_eq!(rows(&f)[0].identity(),saved.identity());assert!(rows(&f)[0].order_id.is_empty());
        assert!(!f.context.recovering_orders.borrow().contains(&recovering_order_key(&f.scope.request.session,KEY)));
        let requests=transport.finish();assert_eq!(requests.len(),1);
        assert!(requests[0].starts_with("POST /v1/credits/orders "));
        assert!(requests[0].contains(&format!("\"client_request_id\":\"{KEY}\"")));
        assert!(requests[0].contains(&format!("x-account-group-id: {PAYER}")));
    }
    #[test]
    fn core_payment_normal_credit_and_membership_create_preserve_front_doors() {
        i_slint_backend_testing::init_no_event_loop();
        for membership in [false,true] {
            let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
            if membership {app.global::<AppState>().invoke_purchase_membership("fixture-plan".into());}
            else {app.global::<AppState>().invoke_recharge_credits("fixture-pack".into());}
            transport.wait();let saved=rows(&f);assert_eq!(saved.len(),1);
            assert_eq!(saved[0].billing_account_group_id,PAYER);
            transport.reply(0,order_response("closed","pending"));join_payment_workers().unwrap();
            pump(||rows(&f).is_empty());
            let requests=transport.finish();assert_eq!(requests.len(),1);
            assert!(requests[0].starts_with(if membership {"POST /v1/membership/orders "}else{"POST /v1/credits/orders "}));
            assert!(requests[0].contains(&saved[0].client_request_id));
        }
    }

    #[test]
    fn core_payment_browser_late_result_does_not_overwrite_selected_b() {
        i_slint_backend_testing::init_no_event_loop();
        let f=fixture("http://127.0.0.1:9/");let app=app(&f);
        seed(&f,record(&f,"credit",ORDER));
        *f.context.active_payment.borrow_mut()=Some(ActivePaymentSession {
            client_request_id:KEY.into(),billing_account_group_id:PAYER.into(),
            checkout_url:Some(format!("http://127.0.0.1:9/v1/payments/alipay/checkout#order_id={ORDER}&token=fixture-token")),
            session_scope:f.scope.request.session.clone(),
        });
        let context=f.context.clone();let session=f.scope.request.session.clone();let weak=app.as_weak();
        with_payment_checkout_test_launcher(move |_| {
            select(&context,&session,OTHER,false);
            weak.upgrade().unwrap().global::<AppState>().set_payment_status_message("B-after-switch".into());
            Err(anyhow!("controlled browser failure"))
        },||app.global::<AppState>().invoke_retry_payment_browser());
        assert_eq!(app.global::<AppState>().get_payment_status_message(),"B-after-switch");
        assert_eq!(rows(&f)[0].client_request_id,KEY);
    }

    #[test]
    fn core_payment_reaped_worker_panic_stays_failed_at_empty_shutdown() {
        let mut f=fixture("http://127.0.0.1:9/");f.expected_join_failure=true;
        let capture=PaymentCapture::new(&f.context).unwrap();
        let receiver=spawn_payment_thread(&f.context,&capture,KEY,|_|panic!("controlled payment worker panic")).unwrap();
        assert!(receiver.recv_timeout(Duration::from_secs(3)).is_err());
        let deadline=Instant::now()+Duration::from_secs(3);
        while payment_worker_pending(&capture,KEY) && Instant::now()<deadline{
            reap_payment_workers();std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!payment_worker_pending(&capture,KEY));
        assert!(shutdown_payment_workers().is_err());
        assert!(join_payment_workers().is_err());
    }
    #[test]
    fn core_payment_held_create_retirement_retains_request_without_late_ui() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        app.global::<AppState>().invoke_recharge_credits("fixture-pack".into());transport.wait();
        let saved=rows(&f);assert_eq!(saved.len(),1);
        let probe=f.persistence.begin_activity().unwrap();
        let activity=f.context.user_activity.clone();let lease=f.persistence.lease().clone();
        let retire=spawn_payment_fixture_thread(move||activity.begin_quiesce(&lease).unwrap().retire());
        let deadline=Instant::now()+Duration::from_secs(3);
        while !probe.is_quiescing() && Instant::now()<deadline{std::thread::sleep(Duration::from_millis(1));}
        assert!(probe.is_quiescing(),"retirement did not close admission");drop(probe);
        app.global::<AppState>().set_credit_payment_message("retired boundary".into());
        app.global::<AppState>().set_payment_status_message("retired boundary".into());
        transport.reply(0,order_response("closed","pending"));
        retire.join().unwrap();join_payment_workers().unwrap();pump_for(Duration::from_millis(40));
        assert_eq!(rows(&f)[0].client_request_id,saved[0].client_request_id);
        assert_eq!(app.global::<AppState>().get_credit_payment_message(),"retired boundary");
        assert_eq!(app.global::<AppState>().get_payment_status_message(),"retired boundary");
        assert_eq!(transport.finish().len(),1);
    }
    #[test]
    fn core_payment_recovery_disconnect_releases_tracking_but_retains_original() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        seed(&f,record(&f,"credit",ORDER));
        recover_pending_orders(&app,f.context.clone());transport.wait();
        transport.reply(0,String::new());join_payment_workers().unwrap();
        pump(|| !f.context.recovering_orders.borrow().contains(&recovering_order_key(&f.scope.request.session,KEY))
            && PAYMENT_THREADS.with(|threads|threads.borrow().is_empty()));
        assert_eq!(rows(&f)[0].client_request_id,KEY);
        assert!(!f.context.recovering_orders.borrow().contains(&recovering_order_key(&f.scope.request.session,KEY)));
        assert!(PAYMENT_THREADS.with(|threads|threads.borrow().is_empty()));
        assert_eq!(transport.finish().len(),1);
    }

    struct BeforeRecordRelease(Option<mpsc::Sender<()>>);
    impl BeforeRecordRelease {
        fn release(mut self) { self.0.take().unwrap().send(()).unwrap(); }
    }
    impl Drop for BeforeRecordRelease {
        fn drop(&mut self) { if let Some(sender)=self.0.take(){let _=sender.send(());} }
    }
    #[test]
    fn core_payment_pre_record_dismiss_and_billing_switch_cannot_create_second_key() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        let (entered_tx,entered_rx)=mpsc::channel();let (release_tx,release_rx)=mpsc::channel();
        PAYMENT_BEFORE_CREATE_RECORD.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{
            entered_tx.send(()).unwrap();release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        })));
        // Drop releases the actual worker even if a callback assertion unwinds.
        let release=BeforeRecordRelease(Some(release_tx));
        let state=app.global::<AppState>();state.invoke_recharge_credits("fixture-pack".into());
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(rows(&f).is_empty(),"worker must still be before the real durable upsert");
        let original=f.context.active_payment.borrow().as_ref().unwrap().client_request_id.clone();
        state.invoke_dismiss_payment();state.invoke_recharge_credits("second-pack".into());
        select(&f.context,&f.scope.request.session,OTHER,true);
        clear_billing_snapshot_state(&app,&f.context);
        state.invoke_recharge_credits("other-payer-pack".into());
        select(&f.context,&f.scope.request.session,PAYER,true);
        clear_billing_snapshot_state(&app,&f.context);
        recover_pending_orders(&app,f.context.clone());
        state.invoke_recharge_credits("third-pack".into());
        assert!(rows(&f).is_empty());
        assert_eq!(f.context.recovering_orders.borrow().len(),1);
        assert!(f.context.recovering_orders.borrow().contains(&recovering_order_key(&f.scope.request.session,&original)));
        assert_eq!(PAYMENT_THREADS.with(|threads|threads.borrow().len()),1);
        release.release();transport.wait();
        let saved=rows(&f);assert_eq!(saved.len(),1);assert_eq!(saved[0].client_request_id,original);
        assert_eq!(saved[0].billing_account_group_id,PAYER);assert_eq!(saved[0].product_code,"fixture-pack");
        transport.reply(0,order_response("closed","pending"));join_payment_workers().unwrap();
        pump_for(Duration::from_millis(40));
        let requests=transport.finish();assert_eq!(requests.len(),1);
        assert!(requests[0].starts_with("POST /v1/credits/orders "));assert!(requests[0].contains(&original));
    }

    #[test]
    fn core_payment_non_online_auth_dispatch_is_outside_completion_and_never_pays() {
        i_slint_backend_testing::init_no_event_loop();
        for offline in [true,false] {
            let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
            let state=app.global::<AppState>();state.set_session_state(if offline{"offline"}else{"signed_out"}.into());
            state.set_auth_method("wechat".into());state.set_auth_wechat_busy(false);state.set_auth_wechat_qr_ready(false);
            let completed=Rc::new(std::cell::Cell::new(false));let observed=completed.clone();
            let probes=Rc::new(RefCell::new(Vec::new()));let callback_probes=probes.clone();
            let latch=f.backend.api.upgrade_latch().clone();
            state.on_start_wechat_login(move||{
                let (tx,rx)=mpsc::channel();let latch=latch.clone();
                let probe=spawn_payment_fixture_thread(move||{let _=latch.snapshot();let _=tx.send(());});
                // On the faulty nested-dispatch path this bounded check fails,
                // but returns so the outer latch releases before the real join.
                observed.set(rx.recv_timeout(Duration::from_secs(1)).is_ok());
                callback_probes.borrow_mut().push(probe);
            });
            state.invoke_recharge_credits("fixture-pack".into());
            for probe in probes.borrow_mut().drain(..){probe.join().unwrap();}
            assert_eq!(completed.get(),!offline,"WeChat dispatch must run only outside the completion latch");
            assert!(state.get_generation_status().contains(if offline{"离线"}else{"请先登录"}));
            assert!(rows(&f).is_empty());assert!(f.context.recovering_orders.borrow().is_empty());
            assert!(!state.get_payment_active());assert!(transport.finish().is_empty());
        }
    }

    #[test]
    fn core_payment_return_to_a_restores_ready_checkout_presentation_before_http() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        seed(&f,record(&f,"credit",ORDER));
        *f.context.active_payment.borrow_mut()=Some(ActivePaymentSession{
            client_request_id:KEY.into(),billing_account_group_id:PAYER.into(),
            checkout_url:Some(format!("{}/v1/payments/alipay/checkout#order_id={ORDER}&token=fixture",transport.url.trim_end_matches('/'))),
            session_scope:f.scope.request.session.clone(),
        });
        f.context.recovering_orders.borrow_mut().insert(recovering_order_key(&f.scope.request.session,KEY));
        select(&f.context,&f.scope.request.session,OTHER,false);clear_billing_snapshot_state(&app,&f.context);
        select(&f.context,&f.scope.request.session,PAYER,true);clear_billing_snapshot_state(&app,&f.context);
        recover_pending_orders(&app,f.context.clone());
        let state=app.global::<AppState>();assert!(state.get_payment_dialog_open());assert!(state.get_payment_active());
        assert!(state.get_payment_browser_ready());assert!(state.get_credit_payment_busy());
        assert_eq!(state.get_payment_dialog_mode(),"waiting");assert_eq!(state.get_payment_kind(),"credit");
        assert!(!state.get_payment_waiting_message().is_empty());assert!(!state.get_payment_success_message().is_empty());
        assert!(!state.get_payment_success_detail().is_empty());assert!(!state.get_payment_status_message().is_empty());
        assert_eq!(f.context.active_payment.borrow().as_ref().unwrap().client_request_id,KEY);
        assert!(transport.finish().is_empty());
    }
}
