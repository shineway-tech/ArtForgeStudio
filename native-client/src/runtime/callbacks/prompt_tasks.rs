use super::*;
use sha2::{Digest, Sha256};



#[derive(Clone)]
struct PromptCapture {
    persistence:PrivatePersistence,
    authority:Arc<NamespaceStorageAuthority>,
    backend:Arc<BackendRuntime>,
    scope:SessionScope,
    active_namespace:Arc<Mutex<Option<NamespaceLease>>>,
}
impl PromptCapture {
    fn new(context:&AppContext)->std::result::Result<Self,ApiError>{
        let persistence=context.store.borrow().private_persistence.clone().ok_or(ApiError::AuthenticationRequired)?;
        let backend=context.backend.clone().ok_or(ApiError::AuthenticationRequired)?;
        let scope=backend.api.session().scope_for_user(persistence.lease().namespace.user_public_id())
            .filter(|scope|scope.auth_epoch==persistence.lease().auth_epoch).ok_or(ApiError::AuthenticationRequired)?;
        let authority=persistence.storage_authority().map_err(transition_error)?;
        let capture=Self{persistence,authority,backend,scope,active_namespace:context.active_namespace.clone()};
        if !capture.is_current(context){return Err(ApiError::AuthenticationRequired);}
        Ok(capture)
    }
    fn binding_matches(&self,context:&AppContext)->bool{
        context.store.borrow().private_persistence.as_ref().is_some_and(|p|p.lease()==self.persistence.lease())
    }
    fn namespace_current(&self,context:&AppContext)->bool{
        self.binding_matches(context) && self.persistence.is_current()
            && self.active_namespace.lock().ok().is_some_and(|active|active.as_ref()==Some(self.persistence.lease()))
    }
    fn is_current(&self,context:&AppContext)->bool{
        !PROMPT_SHUTDOWN.with(|closed|closed.get()) && self.namespace_current(context)
            && self.backend.api.session().is_scope_current(&self.scope)
    }
    fn apply<R>(&self,context:&AppContext,apply:impl FnOnce()->R)->Option<R>{
        if !self.is_current(context){return None;}
        context.apply_user_completion(self.persistence.lease(),||self.binding_matches(context).then(apply)).ok().flatten()
    }
    fn owns(&self,record:&PendingPromptTaskRecord)->bool{
        record.owner_user_id==self.scope.owner_user_id
            && !record.billing_account_group_id.trim().is_empty()
    }
}
struct PromptCancellation { cancelled:std::sync::atomic::AtomicBool, lock:Mutex<()>, wake:std::sync::Condvar }
impl PromptCancellation {
    fn new()->Self{Self{cancelled:std::sync::atomic::AtomicBool::new(false),lock:Mutex::new(()),wake:std::sync::Condvar::new()}}
    fn cancel(&self){self.cancelled.store(true,std::sync::atomic::Ordering::Release);self.wake.notify_all();}
}
struct PromptThread { id:String, lease:NamespaceLease, cancel:Arc<PromptCancellation>, handle:std::thread::JoinHandle<()> }
thread_local! {
    static PROMPT_THREADS:RefCell<Vec<PromptThread>>=const{RefCell::new(Vec::new())};
    static PROMPT_JOIN_FAILED:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
    static PROMPT_SHUTDOWN:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
    static PROMPT_ACTIVE_RESERVATIONS:RefCell<Vec<std::rc::Weak<PromptActiveRequest>>>=const{RefCell::new(Vec::new())};
    #[cfg(test)]
    static PROMPT_AFTER_SEND:RefCell<Option<Box<dyn FnOnce()+Send>>>=const{RefCell::new(None)};
    #[cfg(test)]
    static PROMPT_BEFORE_REMOVE:RefCell<Option<Box<dyn FnOnce()+Send>>>=const{RefCell::new(None)};
    #[cfg(test)]
    static PROMPT_DISCOVERY_COMPLETED:std::cell::Cell<usize>=const{std::cell::Cell::new(0)};
}
fn reap_prompt_workers(){
    let ready=PROMPT_THREADS.with(|threads|{
        let mut threads=threads.borrow_mut();let mut ready=Vec::new();let mut i=0;
        while i<threads.len(){if threads[i].handle.is_finished(){ready.push(threads.remove(i));}else{i+=1;}}ready
    });
    for worker in ready{if worker.handle.join().is_err(){PROMPT_JOIN_FAILED.with(|failed|failed.set(true));}}
}
fn prompt_worker_pending(id:&str)->bool{PROMPT_THREADS.with(|threads|threads.borrow().iter().any(|worker|worker.id==id))}
fn join_prompt_workers()->std::result::Result<(),String>{
    let workers=PROMPT_THREADS.with(|threads|std::mem::take(&mut *threads.borrow_mut()));
    for worker in workers{if worker.handle.join().is_err(){PROMPT_JOIN_FAILED.with(|failed|failed.set(true));}}
    if PROMPT_JOIN_FAILED.with(|failed|failed.get()){Err("prompt worker panicked".into())}else{Ok(())}
}
pub(super) fn cancel_prompt_workers_for_retirement(lease:&NamespaceLease){
    PROMPT_THREADS.with(|threads|for worker in threads.borrow().iter().filter(|worker|&worker.lease==lease){worker.cancel.cancel();});
}
/// Owning UI thread after event-loop exit, outside every completion/activity lock.
pub(super) fn shutdown_prompt_workers()->std::result::Result<(),String>{
    PROMPT_SHUTDOWN.with(|closed|closed.set(true));
    PROMPT_THREADS.with(|threads|for worker in threads.borrow().iter(){worker.cancel.cancel();});
    let joined=join_prompt_workers();
    // Event-loop shutdown may leave completion closures undispatched. Release
    // their exact reservations now; later closure Drop cannot remove a successor.
    let pending=PROMPT_ACTIVE_RESERVATIONS.with(|reservations|std::mem::take(&mut *reservations.borrow_mut()));
    for reservation in pending{if let Some(reservation)=reservation.upgrade(){reservation.release();}}
    release_prompt_result_actions(None);
    joined
}
struct PromptWorker{capture:PromptCapture,cancel:Arc<PromptCancellation>}
impl PromptWorker{
    fn ensure(&self)->std::result::Result<(),ApiError>{
        if self.cancel.cancelled.load(std::sync::atomic::Ordering::Acquire)
            || !self.capture.persistence.is_current()
            || self.capture.active_namespace.lock().ok().is_none_or(|active|active.as_ref()!=Some(self.capture.persistence.lease()))
            || !self.capture.backend.api.user_work_is_current(&self.capture.scope)
        {return Err(ApiError::AuthenticationRequired);}
        Ok(())
    }
    fn wait(&self,duration:Duration)->bool{
        let until=Instant::now()+duration;
        loop{
            if self.ensure().is_err(){return false;}
            let remaining=until.saturating_duration_since(Instant::now());if remaining.is_zero(){return true;}
            let lock=self.cancel.lock.lock().unwrap_or_else(|error|error.into_inner());
            drop(self.cancel.wake.wait_timeout(lock,remaining.min(Duration::from_millis(25))));
        }
    }
}
struct PromptJob<T>{id:String,receiver:mpsc::Receiver<std::result::Result<T,ApiError>>}
fn spawn_prompt_job<T:Send+'static>(
    context:&AppContext,capture:&PromptCapture,
    work:impl FnOnce(&PromptWorker)->std::result::Result<T,ApiError>+Send+'static,
)->std::result::Result<PromptJob<T>,ApiError>{
    if !capture.is_current(context){return Err(ApiError::AuthenticationRequired);}
    let activity=capture.persistence.begin_activity().map_err(transition_error)?;
    let cancel=Arc::new(PromptCancellation::new());
    let worker=PromptWorker{capture:capture.clone(),cancel:cancel.clone()};
    let id=Uuid::new_v4().to_string();let(sender,receiver)=mpsc::channel();
    #[cfg(test)]
    let after_send=PROMPT_AFTER_SEND.with(|hook|hook.borrow_mut().take());
    let handle=std::thread::Builder::new().name("prompt-task".into()).spawn(move||{
        let result=worker.ensure().and_then(|()|work(&worker));
        drop(activity);let _=sender.send(result);
        #[cfg(test)]
        if let Some(after_send)=after_send{after_send();}
    }).map_err(|_|ApiError::LocalState{message:"提示词工作无法启动，已有记录仍会保留".into()})?;
    PROMPT_THREADS.with(|threads|threads.borrow_mut().push(PromptThread{id:id.clone(),lease:capture.persistence.lease().clone(),cancel,handle}));
    Ok(PromptJob{id,receiver})
}
fn poll_prompt_job<T:'static>(
    app:Weak<AppWindow>,context:AppContext,capture:PromptCapture,job:PromptJob<T>,
    complete:impl FnOnce(&AppWindow,&AppContext,&PromptCapture,std::result::Result<T,ApiError>)+'static,
){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        reap_prompt_workers();
        if prompt_worker_pending(&job.id){poll_prompt_job(app,context,capture,job,complete);return;}
        let result=match job.receiver.try_recv(){
            Ok(result)=>result,
            Err(TryRecvError::Empty)=>{poll_prompt_job(app,context,capture,job,complete);return;},
            Err(TryRecvError::Disconnected)=>Err(ApiError::LocalState{message:"提示词工作已中断，已有结果仍会保留".into()}),
        };
        let Some(app)=app.upgrade()else{return;};
        if capture.is_current(&context){complete(&app,&context,&capture,result);}
        else if result.as_ref().err().is_some_and(prompt_task_api_error_requires_login)
            && capture.namespace_current(&context) && terminal_auth_scope_matches_context(&context,&capture.scope)
        {
            // Worker has been joined; no counted worker/effect is held by this UI dispatch.
            sign_out_locally(&app,&context,true,Some(capture.scope.auth_epoch));
        }
    });
}

#[cfg(test)]
thread_local! {
    static PROMPT_CLIPBOARD_TEST:RefCell<Option<Box<dyn FnMut(String)->Result<()>>>>=const{RefCell::new(None)};
}
fn write_prompt_clipboard(text:String)->Result<()> {
    #[cfg(test)]
    if let Some(result)=PROMPT_CLIPBOARD_TEST.with(|slot|slot.borrow_mut().as_mut().map(|write|write(text.clone()))) {return result;}
    let mut clipboard=arboard::Clipboard::new()?;
    clipboard.set_text(text)?;
    Ok(())
}
#[cfg(test)]
fn with_prompt_clipboard_test<T>(write:impl FnMut(String)->Result<()>+'static, action:impl FnOnce()->T)->T {
    struct Clear;
    impl Drop for Clear {fn drop(&mut self){PROMPT_CLIPBOARD_TEST.with(|slot|slot.borrow_mut().take());}}
    PROMPT_CLIPBOARD_TEST.with(|slot|{assert!(slot.borrow().is_none());*slot.borrow_mut()=Some(Box::new(write));});
    let _clear=Clear;
    action()
}

const PROMPT_TASK_RECOVERY_SCHEMA_VERSION: u32 = 2;
const PROMPT_TASK_RETRY_MIN_MS: u64 = 1_000;
const PROMPT_TASK_RETRY_MAX_MS: u64 = 30_000;

#[derive(Clone)]
pub(super) enum PromptResultTarget {
    Composer {
        category: String,
        input: String,
    },
    CustomPrompt {
        session_id: String,
        input: String,
        append_result: bool,
    },
    CanvasNode {
        id: String,
        input: String,
    },
    Video {
        source_id: String,
        input: String,
    },
}

pub(super) struct PromptTaskRequest {
    pub(super) model_code: String,
    pub(super) task_type: &'static str,
    pub(super) prompt: String,
    pub(super) target_language: Option<String>,
    pub(super) optimize: bool,
    pub(super) target: PromptResultTarget,
    pub(super) reference_paths: Vec<PathBuf>,
}

enum PromptTaskOutcome {
    Ready(PendingPromptTaskRecord),
    Settled(PendingPromptTaskRecord),
    Failed {
        record: PendingPromptTaskRecord,
        reason: String,
    },
    Suspended {
        record: PendingPromptTaskRecord,
        reason: String,
    },
    SessionEnded {
        record: PendingPromptTaskRecord,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PromptResultApplication {
    NotApplied,
    AppliedDurably,
    AppliedWithCleanupPending,
    AppliedPendingCustomPromptSave,
}

pub(super) fn wire_prompt_task_recovery_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_apply_recovered_prompt_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            claim_recovered_prompt_result(&app, &context);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_copy_recovered_prompt_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            copy_recovered_prompt_result(&app, &context);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_dismiss_recovered_prompt_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            discard_recovered_prompt_result(&app, &context);
        });
    }
}

fn apply_prompt_task_completion<R>(context: &AppContext, lease: &NamespaceLease, apply: impl FnOnce() -> R) -> Option<R> {
    context.apply_user_completion(lease, apply).ok()
}


pub(super) fn start_backend_prompt_task(
    app:&AppWindow,context:AppContext,task:PromptTaskRequest,
){
    let Some(lease)=context.active_namespace.lock().ok().and_then(|active|active.clone())else{return;};
    let (scope,authority,activity)=match context.capture_billing_action(KnownCapability::Bill){
        Ok(captured)=>captured,
        Err(error)=>{
            let _=apply_prompt_task_completion(&context,&lease,||set_prompt_task_start_failure(app,&task.target,&error.user_message()));
            return;
        }
    };
    drop(activity);
    start_backend_prompt_task_with_billing_scope(app,context,authority,&scope,task);
}
pub(super) fn start_backend_prompt_task_with_billing_scope(
    app:&AppWindow,context:AppContext,authority:Arc<NamespaceStorageAuthority>,billing_scope:&BillingScope,task:PromptTaskRequest,
){
    let lease=authority.lease().clone();
    let billing_scope=match capture_billing_scope_for_submission(context.backend.as_deref(),&authority,billing_scope){
        Ok(scope)=>scope,
        Err(error)=>{
            let _=apply_prompt_task_completion(&context,&lease,||set_prompt_task_start_failure(app,&task.target,&error.user_message()));
            return;
        }
    };
    let Ok(capture)=PromptCapture::new(&context)else{return;};
    if capture.persistence.lease()!=authority.lease() || billing_scope.request.session!=capture.scope
        || !context.billing_context.is_current(&billing_scope){return;}
    let (target_kind,target_id,target_category,target_input,append_result)=serialize_prompt_target(&task.target);
    let record=PendingPromptTaskRecord{
        schema_version:PROMPT_TASK_RECOVERY_SCHEMA_VERSION,created_at_epoch_ms:Local::now().timestamp_millis(),
        client_request_id:Uuid::new_v4().simple().to_string(),owner_user_id:capture.scope.owner_user_id.clone(),
        billing_account_group_id:billing_scope.request.account_group_id.clone(),auth_epoch:capture.scope.auth_epoch,
        server_task_id:String::new(),task_type:task.task_type.into(),model_code:task.model_code,prompt:task.prompt,
        target_language:task.target_language,optimize:task.optimize,target_kind,target_id,target_category,target_input,
        append_result,activity_kind:prompt_activity_kind(task.task_type,&task.target).into(),
        reference_paths:task.reference_paths.iter().map(|path|path.display().to_string()).collect(),
        reference_sha256:vec![],reference_size_bytes:vec![],uploaded_file_ids:vec![],result_prompt:String::new(),
        terminal_error:String::new(),applied_to_target:false,result_committed:false,
    };
    launch_prompt_record(app,context,capture,Some(billing_scope),record,true,true);
}
fn prompt_active_key(capture:&PromptCapture,record:&PendingPromptTaskRecord)->String{
    // Retained auth_epoch may change during validated recovery. Admission identity
    // is the original CURRENT lease, not that mutable persisted epoch.
    format!("{}:{}:{}:{}:{}",capture.scope.owner_user_id,capture.scope.auth_epoch,
        capture.persistence.lease().namespace_epoch,record.billing_account_group_id,record.client_request_id)
}
struct PromptActiveRequest{active:Arc<Mutex<BTreeSet<String>>>,key:String,released:std::cell::Cell<bool>}
impl PromptActiveRequest{
    fn release(&self){
        if !self.released.replace(true){self.active.lock().unwrap_or_else(|error|error.into_inner()).remove(&self.key);}
    }
}
impl Drop for PromptActiveRequest{
    fn drop(&mut self){self.release();}
}
fn launch_prompt_record(
    app:&AppWindow,context:AppContext,capture:PromptCapture,billing_scope:Option<BillingScope>,
    record:PendingPromptTaskRecord,visible:bool,new_record:bool,
){
    if record.owner_user_id!=capture.scope.owner_user_id{return;}
    let key=prompt_active_key(&capture,&record);
    let reserved=capture.apply(&context,||{
        let inserted=context.active_prompt_task_requests.lock().unwrap_or_else(|error|error.into_inner()).insert(key.clone());
        if inserted && visible{set_prompt_task_activity(app,&record,true);}inserted
    }).unwrap_or(false);
    if !reserved{return;}
    let (progress_sender, progress_receiver) = mpsc::channel();
    if visible && record.target_kind == "video_prompt" {
        poll_video_prompt_progress(app.as_weak(), context.clone(), capture.clone(), record.clone(), progress_receiver);
    }
    let work_record=record.clone();
    let release=Rc::new(PromptActiveRequest{active:context.active_prompt_task_requests.clone(),key,released:std::cell::Cell::new(false)});
    PROMPT_ACTIVE_RESERVATIONS.with(|reservations|{
        let mut reservations=reservations.borrow_mut();
        reservations.retain(|reservation|reservation.upgrade().is_some_and(|value|!value.released.get()));
        reservations.push(Rc::downgrade(&release));
    });
    // Only failures before durable submission preparation are known to have no server effect.
    // Later LocalState errors may follow successful billing, so they still refresh.
    let needs_refresh = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let prepared = needs_refresh.clone();
    let job=spawn_prompt_job(&context,&capture,move|worker|{
        let mut record=work_record;
        if new_record{
            let mut hashes=Vec::new();let mut sizes=Vec::new();
            for path in &record.reference_paths{
                worker.ensure()?;
                if !worker.capture.authority.lease().namespace.owns_path(Path::new(path)){
                    return Err(ApiError::LocalState{message:"参考图尚未导入原账号，未创建提示词请求".into()});
                }
                let (activity,effect)=worker.capture.persistence.begin_effect().map_err(transition_error)?;
                let bytes=worker.capture.authority.read_image_source(Path::new(path),100*1024*1024).map_err(transition_error)?;
                hashes.push(format!("{:x}",Sha256::digest(&bytes)));sizes.push(bytes.len() as u64);
                drop(effect);drop(activity);
            }
            record.reference_sha256=hashes;record.reference_size_bytes=sizes;
            worker.ensure()?;
            upsert_pending_prompt_task_for_namespace(&worker.capture.authority,billing_scope.as_ref().ok_or(ApiError::AuthenticationRequired)?,record.clone()).map_err(transition_error)?;
        }
        prepared.store(true, Ordering::Release);
        run_prompt_record(worker,record,billing_scope.as_ref(), &progress_sender)
    });
    match job{
        Ok(job)=>poll_prompt_job(app.as_weak(),context,capture,job,move|app,context,capture,result|{
            let _release=release;
            finish_prompt_record(app,context,capture,record,result,needs_refresh.load(Ordering::Acquire));
        }),
        Err(error)=>{
            drop(release);
            report_prompt_error(app,&context,&capture,&record,&error);
        }
    }
}
pub(super) fn recover_pending_prompt_tasks(app:&AppWindow,context:AppContext){
    let Ok(capture)=PromptCapture::new(&context)else{return;};
    let job=spawn_prompt_job(&context,&capture,|worker|{
        load_pending_prompt_tasks_for_namespace(&worker.capture.authority).map_err(transition_error)
    });
    match job{
        Ok(job)=>poll_prompt_job(app.as_weak(),context,capture,job,|app,context,capture,result|match result{
            Ok(records)=>{
                #[cfg(test)]
                PROMPT_DISCOVERY_COMPLETED.with(|count|count.set(count.get()+1));
                for record in records{
                    if record.owner_user_id==capture.scope.owner_user_id && valid_pending_prompt_task(&record){
                        launch_prompt_record(app,context.clone(),capture.clone(),None,record,false,false);
                    }
                }
                present_next_recovered_prompt_result(app,context);
            }
            Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
        }),
        Err(error)=>report_prompt_recovery_error(app,&context,&capture,&error),
    }
}

fn prompt_unsent_partial(record:&PendingPromptTaskRecord)->bool{
    record.server_task_id.is_empty() && record.result_prompt.is_empty() && record.terminal_error.is_empty()
        && !record.result_committed && !record.applied_to_target
        && record.uploaded_file_ids.len()<record.reference_paths.len()
        && record.reference_sha256.len()==record.reference_paths.len()
        && record.reference_size_bytes.len()==record.reference_paths.len()
}
fn verify_prompt_reference(worker:&PromptWorker,record:&PendingPromptTaskRecord,index:usize)->std::result::Result<(),ApiError>{
    worker.ensure()?;
    let path=record.reference_paths.get(index).ok_or_else(||ApiError::LocalState{message:"原引用路径不完整".into()})?;
    if !worker.capture.authority.lease().namespace.owns_path(Path::new(path)){
        return Err(ApiError::LocalState{message:"原引用不在已保存账号中，记录保持原样".into()});
    }
    let (activity,effect)=worker.capture.persistence.begin_effect().map_err(transition_error)?;
    let bytes=worker.capture.authority.read_image_source(Path::new(path),100*1024*1024).map_err(transition_error)?;
    let valid=record.reference_sha256.get(index).is_some_and(|expected|expected==&format!("{:x}",Sha256::digest(&bytes)))
        && record.reference_size_bytes.get(index)==Some(&(bytes.len() as u64));
    drop(effect);drop(activity);
    if !valid{return Err(ApiError::LocalState{message:"原引用内容已变化，提示词记录保持原样".into()});}
    Ok(())
}
fn revalidate_prompt_epoch(worker:&PromptWorker,mut record:PendingPromptTaskRecord)->std::result::Result<PendingPromptTaskRecord,ApiError>{
    if record.auth_epoch==worker.capture.scope.auth_epoch{return Ok(record);}
    worker.ensure()?;
    let current=load_pending_prompt_tasks_for_namespace(&worker.capture.authority).map_err(transition_error)?
        .into_iter().find(|current|current.identity()==record.identity()).ok_or_else(||ApiError::LocalState{message:"原提示词身份已变化".into()})?;
    if serde_json::to_value(&current).map_err(transition_error)?!=serde_json::to_value(&record).map_err(transition_error)?{
        return Err(ApiError::LocalState{message:"原提示词记录已变化，未重新绑定".into()});
    }
    let mut accepted_id=None;
    if prompt_unsent_partial(&record){
        // All upload IDs must be durably saved before every create/replay below.
        // Thus this exact partial state cannot have reached a billable POST.
        for index in record.uploaded_file_ids.len()..record.reference_paths.len(){verify_prompt_reference(worker,&record,index)?;}
    }else{
        let api=GenerationApi::new(worker.capture.backend.api.clone()).with_saved_group(&record.billing_account_group_id);
        let detail=if !record.server_task_id.is_empty(){
            api.task_scoped(&record.server_task_id,&worker.capture.scope)?
        }else if record.uploaded_file_ids.len()==record.reference_paths.len()
            && record.result_prompt.is_empty() && record.terminal_error.is_empty() && !record.result_committed && !record.applied_to_target{
            let replay=SavedReplayRequest::prompt(worker.capture.authority.clone(),&worker.capture.scope,&record.client_request_id).map_err(transition_error)?;
            worker.capture.backend.api.replay_saved::<GenerationTaskDetail>(&replay)?.data
        }else{return Err(ApiError::LocalState{message:"原任务身份无法安全核验，记录已保留".into()});};
        worker.ensure()?;require_saved_group(&record.billing_account_group_id,&detail.billing_account_group_id)?;
        if detail.id.trim().is_empty() || (!record.server_task_id.is_empty() && detail.id!=record.server_task_id){
            return Err(ApiError::LocalState{message:"原服务端任务编号不匹配".into()});
        }
        if record.server_task_id.is_empty(){accepted_id=Some(detail.id);}
    }
    worker.ensure()?;
    if !rebind_pending_prompt_task_epoch_for_namespace(&worker.capture.authority,&record.identity(),worker.capture.scope.auth_epoch).map_err(transition_error)?{
        return Err(ApiError::LocalState{message:"原任务身份无法重新绑定，记录已保留".into()});
    }
    record.auth_epoch=worker.capture.scope.auth_epoch;
    if let Some(id)=accepted_id{
        require_prompt_patch(&worker.capture,&record,PromptTaskRecoveryPatch::ServerTaskId(id.clone()))?;record.server_task_id=id;
    }
    Ok(record)
}

#[derive(Clone, Copy)]
enum VideoPromptProgress { Queued, Processing, Reconnecting }
impl VideoPromptProgress {
    fn message(self) -> &'static str {
        match self {
            Self::Queued => "视频提示词优化已提交，正在排队，请稍候…",
            Self::Processing => "正在优化视频提示词，请稍候…",
            Self::Reconnecting => "连接暂时中断，正在重新查询原优化任务，请勿重复提交…",
        }
    }
}
fn apply_video_prompt_progress(app: &AppWindow, context: &AppContext, record: &PendingPromptTaskRecord, progress: VideoPromptProgress) {
    let state = app.global::<AppState>();
    if record.target_kind == "video_prompt" && state.get_optimizing_video_prompt()
        && state.get_video_prompt_request_id().as_str() == record.client_request_id
        && prompt_target_matches(app, context, record) {
        state.set_video_prompt_status(progress.message().into());
    }
}
fn poll_video_prompt_progress(
    app: Weak<AppWindow>, context: AppContext, capture: PromptCapture,
    record: PendingPromptTaskRecord, receiver: mpsc::Receiver<VideoPromptProgress>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let Some(window) = app.upgrade() else { return; };
        if !capture.is_current(&context) { return; }
        let mut latest = None;
        let connected = loop {
            match receiver.try_recv() {
                Ok(progress) => latest = Some(progress),
                Err(TryRecvError::Empty) => break true,
                Err(TryRecvError::Disconnected) => break false,
            }
        };
        if let Some(progress) = latest {
            capture.apply(&context, || apply_video_prompt_progress(&window, &context, &record, progress));
        }
        if connected { poll_video_prompt_progress(app, context, capture, record, receiver); }
    });
}
fn prompt_terminal_failure_message(code: Option<&str>) -> &'static str {
    match code {
        Some("INSUFFICIENT_BALANCE" | "provider_credentials_unavailable" | "provider_disabled") =>
            "提示词优化服务暂时不可用，请稍后重试；原始提示词已保留",
        _ => "服务端提示词任务未完成，原始记录已保留",
    }
}

fn run_prompt_record(worker:&PromptWorker,mut record:PendingPromptTaskRecord,billing:Option<&BillingScope>, progress:&mpsc::Sender<VideoPromptProgress>)
    ->std::result::Result<PromptTaskOutcome,ApiError>{
    let capture=&worker.capture;worker.ensure()?;
    if record.owner_user_id!=capture.scope.owner_user_id || !valid_pending_prompt_task(&record){
        return Err(ApiError::LocalState{message:"提示词原始记录无效，已保留".into()});
    }
    record=revalidate_prompt_epoch(worker,record)?;
    let api=GenerationApi::new(capture.backend.api.clone()).with_saved_group(&record.billing_account_group_id);
    if prompt_task_completed_unclaimed(&record){return finish_prompt_terminal(worker,&api,record);}
    while record.uploaded_file_ids.len()<record.reference_paths.len(){
        worker.ensure()?;let index=record.uploaded_file_ids.len();let mut retry=PROMPT_TASK_RETRY_MIN_MS;
        let file_id=loop{
            worker.ensure()?;
            match api.upload_reference_for_namespace_checked(Path::new(&record.reference_paths[index]),&capture.authority,&capture.scope,false,
                &record.reference_sha256[index],record.reference_size_bytes[index]){
                Ok(file_id)=>break file_id,
                Err(error) if prompt_task_api_error_is_transient(&error)=>{
                    if !worker.wait(Duration::from_millis(retry)){return Err(ApiError::AuthenticationRequired);}
                    retry=next_prompt_task_retry_ms(retry);
                }
                Err(error)=>return Err(error),
            }
        };
        worker.ensure()?;record.uploaded_file_ids.push(file_id);
        require_prompt_patch(capture,&record,PromptTaskRecoveryPatch::UploadedFileIds(record.uploaded_file_ids.clone()))?;
    }
    let mut retry=PROMPT_TASK_RETRY_MIN_MS;
    let mut new_submission=billing.cloned();
    loop{
        worker.ensure()?;
        let result=if record.server_task_id.is_empty(){
            if let Some(scope)=new_submission.take(){
                api.create_task_billing(&prompt_task_create_request(&record),&scope)
            }else{
                let replay=SavedReplayRequest::prompt(capture.authority.clone(),&capture.scope,&record.client_request_id).map_err(transition_error)?;
                capture.backend.api.replay_saved::<GenerationTaskDetail>(&replay).map(|response|response.data)
            }
        }else{api.task_scoped(&record.server_task_id,&capture.scope)};
        let detail=match result{
            Ok(detail)=>detail,
            Err(error) if prompt_task_api_error_is_transient(&error)=>{
                let _ = progress.send(VideoPromptProgress::Reconnecting);
                if !worker.wait(Duration::from_millis(retry)){return Err(ApiError::AuthenticationRequired);}
                retry=next_prompt_task_retry_ms(retry);continue;
            }
            Err(error)=>return Err(error),
        };
        worker.ensure()?;
        require_saved_group(&record.billing_account_group_id,&detail.billing_account_group_id)?;
        if detail.id.trim().is_empty() || (!record.server_task_id.is_empty() && record.server_task_id!=detail.id){
            return Err(ApiError::LocalState{message:"服务端提示词编号不匹配，原始任务已保留".into()});
        }
        if record.auth_epoch!=capture.scope.auth_epoch{
            if !rebind_pending_prompt_task_epoch_for_namespace(&capture.authority,&record.identity(),capture.scope.auth_epoch).map_err(transition_error)?{
                return Err(ApiError::LocalState{message:"提示词原始身份已变化，记录已保留".into()});
            }record.auth_epoch=capture.scope.auth_epoch;
        }
        if record.server_task_id.is_empty(){
            require_prompt_patch(capture,&record,PromptTaskRecoveryPatch::ServerTaskId(detail.id.clone()))?;
            record.server_task_id=detail.id.clone();
        }
        if detail.terminal(){
            if matches!(detail.status.as_str(),"completed"|"partially_completed"){
                if let Some(result)=detail.result_prompt.as_deref().map(normalize_prompt_task_result).filter(|value|!value.trim().is_empty()){
                    require_prompt_patch(capture,&record,PromptTaskRecoveryPatch::ResultPrompt(result.clone()))?;record.result_prompt=result;
                }else{
                    let message="服务端已结束但未返回可用提示词，原始记录已保留".to_string();
                    require_prompt_patch(capture,&record,PromptTaskRecoveryPatch::TerminalError(message.clone()))?;record.terminal_error=message;
                }
            }else{
                let message=prompt_terminal_failure_message(detail.failure.as_ref().map(|failure| failure.code.as_str())).to_string();
                require_prompt_patch(capture,&record,PromptTaskRecoveryPatch::TerminalError(message.clone()))?;record.terminal_error=message;
            }
            return finish_prompt_terminal(worker,&api,record);
        }
        let _ = progress.send(if detail.status == "queued" {
            VideoPromptProgress::Queued
        } else {
            VideoPromptProgress::Processing
        });
        retry=PROMPT_TASK_RETRY_MIN_MS;
        if !worker.wait(Duration::from_millis(IMAGE_POLL_INTERVAL_MS)){return Err(ApiError::AuthenticationRequired);}
    }
}
fn require_prompt_patch(capture:&PromptCapture,record:&PendingPromptTaskRecord,patch:PromptTaskRecoveryPatch)->std::result::Result<(),ApiError>{
    if !apply_prompt_task_patch_for_namespace(&capture.authority,&record.identity(),patch).map_err(transition_error)?{
        return Err(ApiError::LocalState{message:"提示词记录已变化，未覆盖原始数据".into()});
    }Ok(())
}
fn finish_prompt_terminal(worker:&PromptWorker,api:&GenerationApi,mut record:PendingPromptTaskRecord)
    ->std::result::Result<PromptTaskOutcome,ApiError>{
    worker.ensure()?;
    if !record.uploaded_file_ids.is_empty(){
        match cleanup_prompt_references_captured(worker,api,&record.uploaded_file_ids){
            Ok(true)=>{
                worker.ensure()?;require_prompt_patch(&worker.capture,&record,PromptTaskRecoveryPatch::UploadedFileIds(vec![]))?;
                record.uploaded_file_ids.clear();
            }
            Ok(false)=>{},
            Err(error)=>return Err(error),
        }
    }
    if record.result_committed{
        if record.uploaded_file_ids.is_empty(){
            worker.ensure()?;
            if !remove_pending_prompt_task_for_namespace(&worker.capture.authority,&record.identity()).map_err(transition_error)?{
                return Err(ApiError::LocalState{message:"已保存结果的清理尚未确认，原始记录已保留".into()});
            }
        }
        Ok(PromptTaskOutcome::Settled(record))
    }else{Ok(PromptTaskOutcome::Ready(record))}
}
fn finish_prompt_record(app:&AppWindow,context:&AppContext,capture:&PromptCapture,expected:PendingPromptTaskRecord,result:std::result::Result<PromptTaskOutcome,ApiError>,refresh_account:bool){
    match result{
        Ok(PromptTaskOutcome::Ready(record))=>{
            if record.owner_user_id!=expected.owner_user_id || record.client_request_id!=expected.client_request_id
                || record.billing_account_group_id!=expected.billing_account_group_id{return;}
            capture.apply(context,||clear_prompt_task_activity_if_owned(app,&expected));
            if matches!(apply_prompt_result_if_target_matches(app,context,&record),PromptResultApplication::NotApplied){
                present_next_recovered_prompt_result(app,context);
            }
        }
        Ok(PromptTaskOutcome::Settled(_))=>{capture.apply(context,||clear_prompt_task_activity_if_owned(app,&expected));}
        Ok(_)=>{present_next_recovered_prompt_result(app,context);}
        Err(error)=>report_prompt_error(app,context,capture,&expected,&error),
    }
    if refresh_account && capture.namespace_current(context) {
        refresh_backend_snapshot_captured(app,context.clone(),capture.persistence.clone());
    }
}
fn report_prompt_recovery_error(app:&AppWindow,context:&AppContext,capture:&PromptCapture,error:&ApiError){
    if error.is_terminal_session_error() && capture.namespace_current(context)
        && terminal_auth_scope_matches_context(context,&capture.scope){
        sign_out_locally(app,context,true,Some(capture.scope.auth_epoch));return;
    }
    capture.apply(context,||{
        let state=app.global::<AppState>();
        let message=show_credit_rejection(&state,error).unwrap_or_else(||"提示词恢复记录已保留，暂时无法完成操作".into());
        state.set_generation_status(message.into());
    });
}
fn report_prompt_error(app:&AppWindow,context:&AppContext,capture:&PromptCapture,record:&PendingPromptTaskRecord,error:&ApiError){
    if error.is_terminal_session_error(){report_prompt_recovery_error(app,context,capture,error);return;}
    capture.apply(context,||{
        clear_prompt_task_activity_if_owned(app,record);
        if prompt_target_matches(app,context,record){
            let message=show_credit_rejection(&app.global::<AppState>(),error)
                .unwrap_or_else(||"原始记录已保留，请稍后恢复".into());
            set_prompt_task_failure(app,record,&message);
        }
    });
}

pub(super) fn prompt_task_create_request(record: &PendingPromptTaskRecord) -> CreateGenerationTask {
    CreateGenerationTask {
        client_request_id: record.client_request_id.clone(),
        task_type: record.task_type.clone(),
        model_code: record.model_code.clone(),
        prompt: record.prompt.clone(),
        quality: None,
        count: None,
        aspect_ratio: None,
        reference_file_ids: (!record.uploaded_file_ids.is_empty())
            .then(|| record.uploaded_file_ids.clone()),
        target_language: record.target_language.clone(),
    }
}

fn prompt_task_api_error_is_transient(error: &ApiError) -> bool {
    error.should_preserve_generation_recovery() || error.code() == Some("request_in_progress")
}

fn prompt_task_api_error_requires_login(error: &ApiError) -> bool {
    matches!(error, ApiError::AuthenticationRequired) || error.is_terminal_session_error()
}

fn prompt_task_session_ended(record: PendingPromptTaskRecord) -> PromptTaskOutcome {
    PromptTaskOutcome::SessionEnded {
        record,
        reason: "登录状态已失效，重新登录后将继续恢复提示词任务".to_string(),
    }
}

fn next_prompt_task_retry_ms(current: u64) -> u64 {
    current
        .saturating_mul(2)
        .clamp(PROMPT_TASK_RETRY_MIN_MS, PROMPT_TASK_RETRY_MAX_MS)
}

fn cleanup_prompt_references_captured(worker:&PromptWorker,api:&GenerationApi,file_ids:&[String])->std::result::Result<bool,ApiError>{
    for file_id in file_ids{
        worker.ensure()?;
        if !cleanup_prompt_task_references(api,std::slice::from_ref(file_id),&worker.capture.scope)?{return Ok(false);}
    }
    worker.ensure()?;Ok(true)
}
fn cleanup_prompt_task_references(
    api: &GenerationApi,
    file_ids: &[String],
    session_scope: &SessionScope,
) -> std::result::Result<bool, ApiError> {
    for file_id in file_ids {
        match api.delete_reference_scoped(file_id, session_scope) {
            Ok(()) => {}
            Err(error) if error.code() == Some("reference_file_in_use") => {}
            Err(ApiError::Http { status: 404, .. }) => {}
            Err(error) => match classify_prompt_reference_cleanup_error(error) {
                Ok(true) => {}
                outcome => return outcome,
            },
        }
    }
    Ok(true)
}

fn classify_prompt_reference_cleanup_error(error: ApiError) -> std::result::Result<bool, ApiError> {
    if prompt_task_api_error_requires_login(&error) {
        Err(error)
    } else {
        Ok(false)
    }
}

fn rebind_prompt_task_epoch(
    record: &mut PendingPromptTaskRecord,
    session_scope: &SessionScope,
    persist: impl FnOnce(&PendingPromptTaskRecord, u64) -> Result<bool>,
) -> Result<bool> {
    if record.owner_user_id != session_scope.owner_user_id {
        return Ok(false);
    }
    if record.auth_epoch == session_scope.auth_epoch {
        return Ok(true);
    }
    let new_auth_epoch = session_scope.auth_epoch;
    if !persist(record, new_auth_epoch)? {
        return Ok(false);
    }
    record.auth_epoch = new_auth_epoch;
    Ok(true)
}

fn valid_pending_prompt_task(record: &PendingPromptTaskRecord) -> bool {
    record.schema_version == PROMPT_TASK_RECOVERY_SCHEMA_VERSION
        && !record.client_request_id.trim().is_empty()
        && !record.owner_user_id.trim().is_empty()
        && !record.task_type.trim().is_empty()
        && !record.model_code.trim().is_empty()
        && !record.prompt.trim().is_empty()
        && record.reference_paths.len() == record.reference_sha256.len()
        && record.reference_paths.len() == record.reference_size_bytes.len()
        && record.uploaded_file_ids.len() <= record.reference_paths.len()
        && matches!(
            record.target_kind.as_str(),
            "composer" | "custom_prompt" | "canvas_node" | "video_prompt"
        )
        && (record.target_kind != "video_prompt" || !record.target_id.trim().is_empty())
}

fn prompt_task_completed_unclaimed(record: &PendingPromptTaskRecord) -> bool {
    !record.result_prompt.trim().is_empty() || !record.terminal_error.trim().is_empty()
}

fn prompt_task_scope_suspended(record: PendingPromptTaskRecord) -> PromptTaskOutcome {
    PromptTaskOutcome::Suspended {
        record,
        reason: "账号已切换，任务将保留给原账号恢复".to_string(),
    }
}

fn serialize_prompt_target(target: &PromptResultTarget) -> (String, String, String, String, bool) {
    match target {
        PromptResultTarget::Composer { category, input } => (
            "composer".to_string(),
            String::new(),
            category.clone(),
            input.clone(),
            false,
        ),
        PromptResultTarget::CustomPrompt {
            session_id,
            input,
            append_result,
        } => (
            "custom_prompt".to_string(),
            session_id.clone(),
            String::new(),
            input.clone(),
            *append_result,
        ),
        PromptResultTarget::CanvasNode { id, input } => (
            "canvas_node".to_string(),
            id.clone(),
            String::new(),
            input.clone(),
            false,
        ),
        PromptResultTarget::Video { source_id, input } => (
            "video_prompt".to_string(),
            source_id.clone(),
            String::new(),
            input.clone(),
            false,
        ),
    }
}

fn prompt_activity_kind(task_type: &str, target: &PromptResultTarget) -> &'static str {
    if matches!(target, PromptResultTarget::Video { .. }) {
        "video_optimize"
    } else if task_type == "prompt_translate" {
        "translate"
    } else if task_type == "image_style_analysis"
        && matches!(target, PromptResultTarget::CustomPrompt { .. })
    {
        "custom_style_analysis"
    } else {
        "optimize"
    }
}

fn prompt_target_matches(
    app: &AppWindow,
    context: &AppContext,
    record: &PendingPromptTaskRecord,
) -> bool {
    let state = app.global::<AppState>();
    if record.target_kind == "video_prompt" {
        return state.get_page() == "video-generation"
            && !state.get_video_generating()
            && !record.target_id.is_empty()
            && record.target_id == state.get_video_source_id().as_str()
            && (record.target_input == state.get_video_prompt().as_str()
                || (!record.result_prompt.is_empty()
                    && record.result_prompt == state.get_video_prompt().as_str()));
    }
    let canvas_input = if record.target_kind == "canvas_node" {
        context
            .store
            .borrow()
            .canvas_notes
            .iter()
            .find(|node| node.id == record.target_id && node.kind == "text")
            .map(|node| node.content.clone())
    } else {
        None
    };
    let current_reference_paths = if record.task_type == "image_style_analysis" {
        if record.target_kind == "composer" {
            references_for_category(&context.store.borrow().references, &record.target_category)
                .iter()
                .take(MAX_REFERENCE_IMAGES)
                .map(|reference| reference.source_path.clone())
                .collect::<Vec<_>>()
        } else if record.target_kind == "custom_prompt" {
            let model = state.get_custom_prompt_reference_items();
            (0..model.row_count())
                .filter_map(|index| model.row_data(index))
                .map(|item| item.source_path.to_string())
                .take(8)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let editor_matches = prompt_target_matches_snapshot(
        record,
        &current_workspace_category(app),
        state.get_prompt().as_str(),
        state.get_custom_prompt_editor_session_id().as_str(),
        state.get_custom_prompt_input().as_str(),
        canvas_input.as_deref(),
        &current_reference_paths,
    );
    editor_matches
}

fn prompt_target_matches_snapshot(
    record: &PendingPromptTaskRecord,
    composer_category: &str,
    composer_input: &str,
    custom_session_id: &str,
    custom_input: &str,
    canvas_input: Option<&str>,
    current_reference_paths: &[String],
) -> bool {
    let editor_matches = match record.target_kind.as_str() {
        "composer" => {
            record.target_category == composer_category
                && (record.target_input == composer_input
                    || (!record.result_prompt.trim().is_empty()
                        && record.result_prompt == composer_input))
        }
        "custom_prompt" => {
            !record.target_id.is_empty()
                && record.target_id == custom_session_id
                && record.target_input == custom_input
        }
        "canvas_node" => canvas_input.is_some_and(|input| {
            input == record.target_input
                || (!record.result_prompt.trim().is_empty() && input == record.result_prompt)
        }),
        _ => false,
    };
    editor_matches
        && (record.task_type != "image_style_analysis"
            || record.reference_paths == current_reference_paths)
}

fn durable_apply_before_result_commit(
    durable_apply: impl FnOnce() -> Result<()>,
    commit: impl FnOnce() -> Result<bool>,
) -> Result<bool> {
    durable_apply()?;
    commit()
}


fn apply_prompt_result_if_target_matches(app:&AppWindow,context:&AppContext,record:&PendingPromptTaskRecord)->PromptResultApplication{
    let Ok(capture)=PromptCapture::new(context)else{return PromptResultApplication::NotApplied;};
    if !capture.owns(record) || record.applied_to_target || record.result_committed || !record.terminal_error.trim().is_empty()
        || record.result_prompt.trim().is_empty() || !prompt_target_matches(app,context,record){return PromptResultApplication::NotApplied;}
    let Some(action)=PromptResultAction::begin(&capture,&record.client_request_id)else{return PromptResultApplication::AppliedWithCleanupPending;};
    let expected=record.clone();let identity=record.identity();let key=record.client_request_id.clone();
    let job=spawn_prompt_job(context,&capture,move|worker|{
        let current=prompt_action_record(worker,&key,Some(&identity))?;
        if current.result_prompt!=expected.result_prompt || current.target_input!=expected.target_input
            || current.reference_paths!=expected.reference_paths || current.reference_sha256!=expected.reference_sha256
            || current.reference_size_bytes!=expected.reference_size_bytes{
            return Err(ApiError::LocalState{message:"提示词结果或输入已变化，原始记录已保留".into()});
        }
        if current.task_type=="image_style_analysis"{
            for (index,path) in current.reference_paths.iter().enumerate(){
                worker.ensure()?;
                let (activity,effect)=worker.capture.persistence.begin_effect().map_err(transition_error)?;
                let bytes=worker.capture.authority.read_image_source(Path::new(path),100*1024*1024).map_err(transition_error)?;
                let valid=current.reference_sha256[index]==format!("{:x}",Sha256::digest(&bytes)) && current.reference_size_bytes[index]==bytes.len() as u64;
                drop(effect);drop(activity);
                if !valid{return Err(ApiError::LocalState{message:"参考图已变化，结果仍可在恢复窗口明确领取".into()});}
            }
        }
        Ok(current)
    });
    match job{
        Ok(job)=>poll_prompt_job(app.as_weak(),context.clone(),capture,job,move|app,context,capture,result|match result{
            Ok(record)=>begin_prompt_result_application(app,context,capture,record.clone(),record,action),
            Err(error)=>{report_prompt_recovery_error(app,context,capture,&error);present_next_recovered_prompt_result(app,context);}
        }),
        Err(error)=>{report_prompt_recovery_error(app,context,&capture,&error);return PromptResultApplication::NotApplied;}
    }
    // Scheduled, not a durable success. Record cleanup belongs to the real ack chain.
    PromptResultApplication::AppliedWithCleanupPending
}
enum PromptStoreBefore{
    Composer(String),
    Video(Option<VideoPromptDraft>),
    Canvas(String,CanvasSnapshot),
}
fn staged_prompt_is_current(store:&Store,target:&PendingPromptTaskRecord)->bool{
    match target.target_kind.as_str(){
        "composer"=>prompt_draft_for_category(&store.prompt_drafts,&target.target_category)==target.result_prompt,
        "video_prompt"=>store.prompt_drafts.video_by_owner.get(&target.owner_user_id)
            .is_some_and(|draft|draft.source_id==target.target_id && draft.prompt==target.result_prompt),
        "canvas_node"=>store.canvas_notes.iter().any(|node|node.id==target.target_id && node.kind=="text" && node.content==target.result_prompt),
        _=>false,
    }
}
fn rollback_prompt_stage(context:&AppContext,capture:&PromptCapture,target:&PendingPromptTaskRecord,before:&PromptStoreBefore){
    capture.apply(context,||{
        let mut store=context.store.borrow_mut();if !staged_prompt_is_current(&store,target){return;}
        match before{
            PromptStoreBefore::Composer(value)=>set_prompt_draft_for_category(&mut store.prompt_drafts,&target.target_category,value.clone()),
            PromptStoreBefore::Video(value)=>match value{
                Some(value)=>{store.prompt_drafts.video_by_owner.insert(target.owner_user_id.clone(),value.clone());}
                None=>{store.prompt_drafts.video_by_owner.remove(&target.owner_user_id);}
            },
            PromptStoreBefore::Canvas(value,_)=>if let Some(node)=store.canvas_notes.iter_mut().find(|node|node.id==target.target_id){node.content=value.clone();},
        }
    });
}
fn begin_prompt_result_application(
    app:&AppWindow,context:&AppContext,capture:&PromptCapture,record:PendingPromptTaskRecord,target:PendingPromptTaskRecord,action:PromptResultAction,
){
    if !prompt_target_matches(app,context,&target){present_next_recovered_prompt_result(app,context);return;}
    if target.target_kind=="custom_prompt"{
        begin_custom_prompt_result_application(app,context,capture,record,target,action);return;
    }
    let prepared=match capture.persistence.prepare_ordered_save(){
        Ok(prepared)=>prepared,
        Err(error)=>{report_prompt_recovery_error(app,context,capture,&transition_error(error));return;}
    };
    let mut prepared=Some(prepared);let mut before=None;
    let enqueued=capture.apply(context,||{
        if !prompt_target_matches(app,context,&target){return None;}
        let mut store=context.store.borrow_mut();
        before=Some(match target.target_kind.as_str(){
            "composer"=>{
                let old=prompt_draft_for_category(&store.prompt_drafts,&target.target_category);
                set_prompt_draft_for_category(&mut store.prompt_drafts,&target.target_category,target.result_prompt.clone());
                PromptStoreBefore::Composer(old)
            }
            "video_prompt"=>{
                let old=store.prompt_drafts.video_by_owner.get(&target.owner_user_id).cloned();
                store_video_prompt_draft(&mut store.prompt_drafts,&target.owner_user_id,&target.target_id,&target.result_prompt);
                PromptStoreBefore::Video(old)
            }
            "canvas_node"=>{
                let snapshot=canvas_snapshot(&store);
                let Some(node)=store.canvas_notes.iter_mut().find(|node|node.id==target.target_id && node.kind=="text")else{return None;};
                let old=node.content.clone();node.content=target.result_prompt.clone();PromptStoreBefore::Canvas(old,snapshot)
            }
            _=>return None,
        });
        let data=local_store_data(app,&store);
        // Return the WHOLE Result. Its error owns latch/activity guards.
        Some(prepared.take().unwrap().enqueue(data))
    }).flatten();
    drop(prepared);
    let Some(enqueued)=enqueued else{return;};
    let Some(before)=before else{return;};
    let receiver=match enqueued{
        Ok(receiver)=>receiver,
        Err(error)=>{
            drop(error);
            rollback_prompt_stage(context,capture,&target,&before);
            report_prompt_recovery_error(app,context,capture,&ApiError::LocalState{message:"本地写入无法入队".into()});return;
        }
    };
    let ack=spawn_prompt_job(context,capture,move|_worker|{
        receiver.recv().map_err(|_|ApiError::LocalState{message:"本地写入确认通道已关闭".into()})?
            .map_err(|_|ApiError::LocalState{message:"本地写入未确认".into()})?;
        Ok(())
    });
    match ack{
        Ok(job)=>poll_prompt_job(app.as_weak(),context.clone(),capture.clone(),job,move|app,context,capture,result|{
            if let Err(error)=result{
                rollback_prompt_stage(context,capture,&target,&before);
                report_prompt_recovery_error(app,context,capture,&error);return;
            }
            if !prompt_target_matches(app,context,&target) || !staged_prompt_is_current(&context.store.borrow(),&target){
                // A newer edit/save won. Keep the paid recovery record.
                present_next_recovered_prompt_result(app,context);return;
            }
            let published=capture.apply(context,||{
                if !prompt_target_matches(app,context,&target) || !staged_prompt_is_current(&context.store.borrow(),&target){return false;}
                let state=app.global::<AppState>();
                        match target.target_kind.as_str(){
                            "composer"=>state.set_prompt(target.result_prompt.clone().into()),
                            "video_prompt"=>{state.set_video_prompt(target.result_prompt.clone().into());}
                            "canvas_node"=>{
                                if let PromptStoreBefore::Canvas(_,snapshot)=&before{
                                    context.canvas_history.borrow_mut().record(snapshot.clone());
                                    let history=context.canvas_history.borrow();state.set_canvas_can_undo(history.can_undo());state.set_canvas_can_redo(history.can_redo());
                                }
                                let mut nodes=state.get_canvas_notes().iter().collect::<Vec<_>>();
                                if let Some(node)=nodes.iter_mut().find(|node|node.id.as_str()==target.target_id){node.content=target.result_prompt.clone().into();}
                                state.set_canvas_notes(ModelRc::new(VecModel::from(nodes)));
                            }
                            _=>{},
                        }

                state.set_generation_status("结果已保存，正在确认原始恢复记录".into());
                true
            }).unwrap_or(false);
            if !published{present_next_recovered_prompt_result(app,context);return;}
            commit_prompt_result_after_ack(app,context,capture,record,target,action);
        }),
        Err(error)=>{
            // The SAME writer still owns queued guards; no false commit/UI success.
            report_prompt_recovery_error(app,context,capture,&error);
        }
    }
}
fn commit_prompt_result_after_ack(
    app:&AppWindow,context:&AppContext,capture:&PromptCapture,record:PendingPromptTaskRecord,target:PendingPromptTaskRecord,
    action:PromptResultAction,
){
    let identity=record.identity();let key=record.client_request_id.clone();
    #[cfg(test)]
    let before_remove=PROMPT_BEFORE_REMOVE.with(|hook|hook.borrow_mut().take());
    let job=spawn_prompt_job(context,capture,move|worker|{
        let current=prompt_action_record(worker,&key,Some(&identity))?;
        if current.result_prompt!=record.result_prompt{return Err(ApiError::LocalState{message:"原始结果已变化".into()});}
        worker.ensure()?;require_prompt_patch(&worker.capture,&current,PromptTaskRecoveryPatch::ResultCommitted)?;
        #[cfg(test)]
        if let Some(before_remove)=before_remove{before_remove();}
        if current.uploaded_file_ids.is_empty() && !remove_pending_prompt_task_for_namespace(&worker.capture.authority,&current.identity()).map_err(transition_error)?{
            return Err(ApiError::LocalState{message:"本地已保存，恢复记录清理未确认".into()});
        }Ok(current)
    });
    match job{
        Ok(job)=>poll_prompt_job(app.as_weak(),context.clone(),capture.clone(),job,move|app,context,capture,result|{
            let _action=action;
            match result{
                Ok(record)=>{
                    capture.apply(context,||{
                        let mut displayed=target.clone();displayed.target_input=target.result_prompt.clone();
                        if !prompt_target_matches(app,context,&displayed) || !staged_prompt_is_current(&context.store.borrow(),&target){return;}
                        let state=app.global::<AppState>();
                        state.set_generation_status(prompt_task_success_message(&record).into());
                        if target.target_kind=="video_prompt"{state.set_video_prompt_status("视频提示词已优化".into());}
                        if state.get_recovered_prompt_client_request_id().as_str()==record.client_request_id{clear_recovered_prompt_presentation(&state);}
                    });
                    present_next_recovered_prompt_result(app,context);
                }
                Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
            }
        }),
        Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
    }
}
fn begin_custom_prompt_result_application(
    app:&AppWindow,context:&AppContext,capture:&PromptCapture,record:PendingPromptTaskRecord,target:PendingPromptTaskRecord,action:PromptResultAction,
){
    let identity=record.identity();let key=record.client_request_id.clone();
    let job=spawn_prompt_job(context,capture,move|worker|{
        let current=prompt_action_record(worker,&key,Some(&identity))?;
        worker.ensure()?;require_prompt_patch(&worker.capture,&current,PromptTaskRecoveryPatch::AppliedToTarget)?;Ok(current)
    });
    match job{
        Ok(job)=>poll_prompt_job(app.as_weak(),context.clone(),capture.clone(),job,move|app,context,capture,result|{
            let _action=action;
            match result{
                Ok(record)=>{let shown=capture.apply(context,||{
                    if !prompt_target_matches(app,context,&target){return false;}
                    let state=app.global::<AppState>();
                    let value=if record.append_result && !target.target_input.trim().is_empty(){format!("{}\n\n{}",target.target_input.trim(),record.result_prompt)}else{record.result_prompt};
                    state.set_custom_prompt_input(value.into());state.set_custom_prompt_message("已使用恢复的提示词结果，保存后完成领取".into());
                    state.set_custom_prompt_recovered_request_id(record.client_request_id.clone().into());
                    if state.get_recovered_prompt_client_request_id().as_str()==record.client_request_id{clear_recovered_prompt_presentation(&state);}
                    true
                }).unwrap_or(false);if !shown{present_next_recovered_prompt_result(app,context);}}
                Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
            }
        }),
        Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
    }
}

fn prompt_task_success_message(record: &PendingPromptTaskRecord) -> &'static str {
    if record.task_type == "image_style_analysis" {
        "图片风格分析完成"
    } else if record.task_type == "prompt_translate" {
        "提示词翻译完成"
    } else {
        "提示词优化完成"
    }
}

fn set_prompt_task_activity(app: &AppWindow, record: &PendingPromptTaskRecord, active: bool) {
    let state = app.global::<AppState>();
    match record.activity_kind.as_str() {
        "video_optimize" => {
            state.set_optimizing_video_prompt(active);
            state.set_video_prompt_request_id(
                if active { record.client_request_id.clone() } else { String::new() }.into(),
            );
        }
        "translate" => {
            state.set_translating_prompt(active);
            state.set_translating_prompt_request_id(
                if active {
                    record.client_request_id.clone()
                } else {
                    String::new()
                }
                .into(),
            );
        }
        "custom_style_analysis" => {
            state.set_custom_prompt_analyzing(active);
            state.set_custom_style_analysis_request_id(
                if active {
                    record.client_request_id.clone()
                } else {
                    String::new()
                }
                .into(),
            );
        }
        _ => {
            state.set_optimizing_prompt(active);
            state.set_optimizing_prompt_request_id(
                if active {
                    record.client_request_id.clone()
                } else {
                    String::new()
                }
                .into(),
            );
        }
    }
}

fn clear_prompt_task_activity_if_owned(app: &AppWindow, record: &PendingPromptTaskRecord) {
    let state = app.global::<AppState>();
    let owns_activity = match record.activity_kind.as_str() {
        "video_optimize" => {
            state.get_video_prompt_request_id().as_str() == record.client_request_id
        }
        "translate" => {
            state.get_translating_prompt_request_id().as_str() == record.client_request_id
        }
        "custom_style_analysis" => {
            state.get_custom_style_analysis_request_id().as_str() == record.client_request_id
        }
        _ => state.get_optimizing_prompt_request_id().as_str() == record.client_request_id,
    };
    if owns_activity {
        set_prompt_task_activity(app, record, false);
    }
}

fn set_prompt_task_start_failure(app: &AppWindow, target: &PromptResultTarget, reason: &str) {
    let state = app.global::<AppState>();
    match target {
        PromptResultTarget::Video { .. } => {
            state.set_optimizing_video_prompt(false);
            state.set_video_prompt_status(reason.into());
        }
        PromptResultTarget::CustomPrompt { .. } => {
            state.set_custom_prompt_analyzing(false);
            state.set_optimizing_prompt(false);
            state.set_custom_prompt_message(reason.into());
        }
        _ => {
            state.set_optimizing_prompt(false);
            state.set_translating_prompt(false);
            state.set_generation_status(reason.into());
        }
    }
}

fn set_prompt_task_failure(app: &AppWindow, record: &PendingPromptTaskRecord, reason: &str) {
    let state = app.global::<AppState>();
    if record.target_kind == "video_prompt" {
        state.set_video_prompt_status(format!("视频提示词优化失败：{reason}").into());
    } else if record.target_kind == "custom_prompt" {
        state.set_custom_prompt_message(format!("提示词处理失败：{reason}").into());
    } else {
        state.set_generation_status(format!("提示词处理失败：{reason}").into());
    }
}


fn prompt_action_record(worker:&PromptWorker,key:&str,expected:Option<&RecoveryRecordIdentity>)
    ->std::result::Result<PendingPromptTaskRecord,ApiError>{
    worker.ensure()?;
    let record=load_pending_prompt_tasks_for_namespace(&worker.capture.authority).map_err(transition_error)?
        .into_iter().find(|record|record.owner_user_id==worker.capture.scope.owner_user_id && record.client_request_id==key)
        .ok_or_else(||ApiError::LocalState{message:"原始提示词记录暂不可读取，已保留界面以便重试".into()})?;
    if !valid_pending_prompt_task(&record) || expected.is_some_and(|identity|&record.identity()!=identity){
        return Err(ApiError::LocalState{message:"原始提示词身份不匹配，未改写记录".into()});
    }
    revalidate_prompt_epoch(worker,record)
}
fn present_next_recovered_prompt_result(app:&AppWindow,context:&AppContext){
    let Ok(capture)=PromptCapture::new(context)else{return;};
    if app.global::<AppState>().get_recovered_prompt_result_open(){return;}
    let tracked=app.global::<AppState>().get_custom_prompt_recovered_request_id().to_string();
    let expected_selected=app.global::<AppState>().get_recovered_prompt_client_request_id().to_string();
    let job=spawn_prompt_job(context,&capture,|worker|load_pending_prompt_tasks_for_namespace(&worker.capture.authority).map_err(transition_error));
    match job{
        Ok(job)=>poll_prompt_job(app.as_weak(),context.clone(),capture,job,move|app,context,capture,result|{
            match result{
                Ok(records)=>{
                    let next=next_recovered_prompt_record(records,&capture.scope.owner_user_id,&tracked);
                    capture.apply(context,||{
                        let state=app.global::<AppState>();
                        if state.get_recovered_prompt_result_open() || state.get_custom_prompt_recovered_request_id().as_str()!=tracked
                            || state.get_recovered_prompt_client_request_id().as_str()!=expected_selected{return;}
                        match next{
                            Some(record)=>{
                                state.set_recovered_prompt_client_request_id(record.client_request_id.into());
                                state.set_recovered_prompt_task_type(record.task_type.into());state.set_recovered_prompt_target_kind(record.target_kind.into());
                                state.set_recovered_prompt_target_id(record.target_id.into());state.set_recovered_prompt_result(record.result_prompt.into());
                                state.set_recovered_prompt_error(record.terminal_error.into());state.set_recovered_prompt_result_open(true);
                            }
                            None=>clear_recovered_prompt_presentation(&state),
                        }
                    });
                }
                Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
            }
        }),
        Err(error)=>report_prompt_recovery_error(app,context,&capture,&error),
    }
}
fn next_recovered_prompt_record(records:Vec<PendingPromptTaskRecord>,owner:&str,tracked:&str)->Option<PendingPromptTaskRecord>{
    if !tracked.is_empty(){return None;}
    let mut records=records.into_iter().filter(|record|record.owner_user_id==owner && prompt_task_completed_unclaimed(record) && !record.result_committed).collect::<Vec<_>>();
    records.sort_by_key(|record|(record.created_at_epoch_ms,record.client_request_id.clone()));records.into_iter().next()
}
fn clear_recovered_prompt_presentation(state:&AppState){
    state.set_recovered_prompt_result_open(false);state.set_recovered_prompt_client_request_id("".into());
    state.set_recovered_prompt_task_type("".into());state.set_recovered_prompt_target_kind("".into());
    state.set_recovered_prompt_target_id("".into());state.set_recovered_prompt_result("".into());state.set_recovered_prompt_error("".into());
}
fn prompt_selected_key(app:&AppWindow)->Option<String>{
    let key=app.global::<AppState>().get_recovered_prompt_client_request_id().to_string();
    (!key.trim().is_empty()).then_some(key)
}
struct PromptResultAction { reservation:Rc<PromptResultReservation> }
struct PromptResultReservation {
    key:String,lease:NamespaceLease,active:Rc<RefCell<BTreeSet<String>>>,released:Cell<bool>,
}
thread_local!{
    static PROMPT_RESULT_ACTIONS:Rc<RefCell<BTreeSet<String>>>=Rc::new(RefCell::new(BTreeSet::new()));
    static PROMPT_RESULT_RESERVATIONS:RefCell<Vec<std::rc::Weak<PromptResultReservation>>>=const{RefCell::new(Vec::new())};
}
impl PromptResultAction{
    fn begin(capture:&PromptCapture,key:&str)->Option<Self>{
        let key=format!("{}:{}:{}:{}",capture.scope.owner_user_id,capture.scope.auth_epoch,capture.persistence.lease().namespace_epoch,key);
        let active=PROMPT_RESULT_ACTIONS.with(Clone::clone);
        // Never construct a losing guard: its Drop would release the winner.
        if !active.borrow_mut().insert(key.clone()){return None;}
        let reservation=Rc::new(PromptResultReservation {
            key,lease:capture.persistence.lease().clone(),active,released:Cell::new(false),
        });
        PROMPT_RESULT_RESERVATIONS.with(|pending|{
            let mut pending=pending.borrow_mut();
            pending.retain(|item|item.upgrade().is_some_and(|item|!item.released.get()));
            pending.push(Rc::downgrade(&reservation));
        });
        Some(Self{reservation})
    }
}
impl PromptResultReservation {
    fn release(&self){if !self.released.replace(true){self.active.borrow_mut().remove(&self.key);}}
}
// A Slint timer may outlive the TLS registry. The guard owns its actual set,
// and therefore never accesses TLS while a timer/worker is being destroyed.
impl Drop for PromptResultReservation{fn drop(&mut self){self.release();}}
fn release_prompt_result_actions(lease:Option<&NamespaceLease>){
    let pending=PROMPT_RESULT_RESERVATIONS.with(|pending|std::mem::take(&mut *pending.borrow_mut()));
    let mut retained=Vec::new();
    for item in pending {
        if let Some(reservation)=item.upgrade(){
            if lease.is_none_or(|lease|lease==&reservation.lease){reservation.release();}
            else {retained.push(Rc::downgrade(&reservation));}
        }
    }
    PROMPT_RESULT_RESERVATIONS.with(|pending|pending.borrow_mut().extend(retained));
}
fn with_selected_prompt_record(
    app:&AppWindow,context:&AppContext,
    complete:impl FnOnce(&AppWindow,&AppContext,&PromptCapture,PendingPromptTaskRecord,PromptResultAction)+'static,
){
    let Ok(capture)=PromptCapture::new(context)else{return;};let Some(key)=prompt_selected_key(app)else{return;};
    let Some(action)=PromptResultAction::begin(&capture,&key)else{return;};let work_key=key.clone();
    match spawn_prompt_job(context,&capture,move|worker|prompt_action_record(worker,&work_key,None)){
        Ok(job)=>poll_prompt_job(app.as_weak(),context.clone(),capture,job,move|app,context,capture,result|{
            if app.global::<AppState>().get_recovered_prompt_client_request_id().as_str()!=key{return;}
            match result{
                Ok(record) if prompt_task_completed_unclaimed(&record)=>complete(app,context,capture,record,action),
                Ok(_)=>{},
                Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
            }
        }),
        Err(error)=>report_prompt_recovery_error(app,context,&capture,&error),
    }
}
fn claim_recovered_prompt_result(app:&AppWindow,context:&AppContext){
    with_selected_prompt_record(app,context,|app,context,capture,record,action|{
        if record.result_committed {
            retry_committed_prompt_cleanup(app,context,capture,record,action);
            return;
        }
        if record.result_prompt.trim().is_empty() || !record.terminal_error.trim().is_empty(){return;}
        let state=app.global::<AppState>();let mut target=record.clone();
        if record.target_kind=="video_prompt"{
            if state.get_page()!="video-generation" || state.get_video_source_id().as_str()!=record.target_id || state.get_video_generating(){
                if reopen_video_prompt_target(app,context,&record) {
                    // Restore the original source page first; the recovery worker will
                    // rediscover this result once the video workspace is active.
                    state.set_recovered_prompt_result_open(false);
                    state.invoke_viewer_generate_video();
                    let weak=app.as_weak();
                    let context=context.clone();
                    slint::Timer::single_shot(Duration::from_millis(700),move||{
                        if let Some(app)=weak.upgrade(){claim_recovered_prompt_result(&app,&context);}
                    });
                }
                return;
            }
            target.target_input=state.get_video_prompt().to_string();
        }else if record.target_kind=="custom_prompt" && state.get_custom_prompt_editor_open(){
            target.target_id=state.get_custom_prompt_editor_session_id().to_string();
            target.target_input=state.get_custom_prompt_input().to_string();
            target.task_type="prompt_optimize".into();
        }else{
            target.target_kind="composer".into();target.target_category=current_workspace_category(app);
            target.target_input=state.get_prompt().to_string();target.task_type="prompt_optimize".into();
        }
        begin_prompt_result_application(app,context,capture,record,target,action);
    });
}
fn reopen_video_prompt_target(app:&AppWindow,context:&AppContext,record:&PendingPromptTaskRecord)->bool{
    let found={
        let store=context.store.borrow();
        store.assets.iter().map(|item|("asset",item))
            .chain(store.generations.iter().map(|item|("generation",item)))
            .chain(store.inspiration.iter().map(|item|("inspiration",item)))
            .find(|(_,item)|item.id==record.target_id)
            .map(|(source,item)|(source.to_string(),item.clone()))
    };
    let Some((source,item))=found else{return false;};
    let state=app.global::<AppState>();
    state.set_viewer_id(item.id.into());state.set_viewer_source(source.into());
    state.set_viewer_source_path(item.source_path.into());state.set_viewer_title(item.title.into());
    state.set_viewer_prompt(item.prompt.into());true
}
fn retry_committed_prompt_cleanup(
    app:&AppWindow,context:&AppContext,capture:&PromptCapture,record:PendingPromptTaskRecord,action:PromptResultAction,
){
    let identity=record.identity();let key=record.client_request_id.clone();
    let job=spawn_prompt_job(context,capture,move|worker|{
        let current=prompt_action_record(worker,&key,Some(&identity))?;
        if !current.result_committed{return Err(ApiError::LocalState{message:"原始保存确认已变化，结果仍已保留".into()});}
        let api=GenerationApi::new(worker.capture.backend.api.clone()).with_saved_group(&current.billing_account_group_id);
        if !cleanup_prompt_references_captured(worker,&api,&current.uploaded_file_ids)?{
            return Err(ApiError::LocalState{message:"引用清理未确认，已保存结果仍可重试".into()});
        }
        worker.ensure()?;
        if !remove_pending_prompt_task_for_namespace(&worker.capture.authority,&current.identity()).map_err(transition_error)?{
            return Err(ApiError::LocalState{message:"已保存结果的恢复记录未移除，仍可重试".into()});
        }Ok(current.client_request_id)
    });
    match job{
        Ok(job)=>poll_prompt_job(app.as_weak(),context.clone(),capture.clone(),job,move|app,context,capture,result|{
            let _action=action;
            match result{
                Ok(key)=>{
                    // This is cleanup-only: never reapply a previously saved result over later edits.
                    capture.apply(context,||if app.global::<AppState>().get_recovered_prompt_client_request_id().as_str()==key{
                        clear_recovered_prompt_presentation(&app.global::<AppState>());
                    });
                    present_next_recovered_prompt_result(app,context);
                }
                Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
            }
        }),
        Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
    }
}
fn copy_recovered_prompt_result(app:&AppWindow,context:&AppContext){
    with_selected_prompt_record(app,context,|app,context,capture,record,_action|{
        let Ok((activity,effect))=capture.persistence.begin_effect()else{return;};
        if !capture.is_current(context){drop(effect);drop(activity);return;}
        let text=if record.terminal_error.trim().is_empty(){record.result_prompt}else{record.terminal_error};
        let result=write_prompt_clipboard(text);
        drop(effect);drop(activity);
        capture.apply(context,||app.global::<AppState>().set_generation_status(
            if result.is_ok(){"恢复的提示词结果已复制，记录仍会保留"}else{"复制失败，请手动选择结果文本"}.into()));
    });
}
fn discard_recovered_prompt_result(app:&AppWindow,context:&AppContext){
    with_selected_prompt_record(app,context,|app,context,capture,record,action|{
        let identity=record.identity();let key=record.client_request_id.clone();
        #[cfg(test)]
        let before_remove=PROMPT_BEFORE_REMOVE.with(|hook|hook.borrow_mut().take());
        let job=spawn_prompt_job(context,capture,move|worker|{
            let record=prompt_action_record(worker,&key,Some(&identity))?;
            #[cfg(test)]
            if let Some(before_remove)=before_remove{before_remove();}
            let api=GenerationApi::new(worker.capture.backend.api.clone()).with_saved_group(&record.billing_account_group_id);
            if !cleanup_prompt_references_captured(worker,&api,&record.uploaded_file_ids)?{
                return Err(ApiError::LocalState{message:"引用清理未确认，提示词记录仍已保留".into()});
            }
            worker.ensure()?;
            if !remove_pending_prompt_task_for_namespace(&worker.capture.authority,&record.identity()).map_err(transition_error)?{
                return Err(ApiError::LocalState{message:"提示词记录未移除，仍可重试".into()});
            }Ok(record.client_request_id)
        });
        match job{
            Ok(job)=>poll_prompt_job(app.as_weak(),context.clone(),capture.clone(),job,move|app,context,capture,result|{
                let _action=action;
                match result{
                    Ok(key)=>{
                        capture.apply(context,||if app.global::<AppState>().get_recovered_prompt_client_request_id().as_str()==key{
                            clear_recovered_prompt_presentation(&app.global::<AppState>());
                        });
                        present_next_recovered_prompt_result(app,context);
                    }
                    Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
                }
            }),
            Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
        }
    });
}
pub(super) fn acknowledge_custom_prompt_recovered_result_captured(
    app:&AppWindow,context:&AppContext,persistence:PrivatePersistence,identity:RecoveryRecordIdentity,
){
    let Ok(capture)=PromptCapture::new(context)else{return;};
    if capture.persistence.lease()!=persistence.lease() || !persistence.is_current(){return;}
    let key=format!("ack:{identity:?}");
    let Some(action)=PromptResultAction::begin(&capture,&key)else{return;};
    let job=spawn_prompt_job(context,&capture,move|worker|{
        let record=load_pending_prompt_tasks_for_namespace(&worker.capture.authority).map_err(transition_error)?
            .into_iter().find(|record|record.identity()==identity && record.owner_user_id==worker.capture.scope.owner_user_id)
            .ok_or_else(||ApiError::LocalState{message:"原始自定义提示词身份未找到，未清理其他记录".into()})?;
        let record=revalidate_prompt_epoch(worker,record)?;
        if record.target_kind!="custom_prompt" || !record.applied_to_target || record.result_prompt.trim().is_empty(){
            return Err(ApiError::LocalState{message:"自定义提示词恢复确认不匹配，记录已保留".into()});
        }
        worker.ensure()?;
        require_prompt_patch(&worker.capture,&record,PromptTaskRecoveryPatch::ReleaseCustomPromptResult)?;
        if record.uploaded_file_ids.is_empty() && !remove_pending_prompt_task_for_namespace(&worker.capture.authority,&record.identity()).map_err(transition_error)?{
            return Err(ApiError::LocalState{message:"自定义提示词确认清理未完成，记录已保留".into()});
        }
        Ok(record.client_request_id)
    });
    match job{
        Ok(job)=>poll_prompt_job(app.as_weak(),context.clone(),capture,job,move|app,context,capture,result|{
            let _action=action;
            match result{
                Ok(expected_key)=>{capture.apply(context,||if app.global::<AppState>().get_custom_prompt_recovered_request_id().as_str()==expected_key{
                    app.global::<AppState>().set_custom_prompt_recovered_request_id("".into());
                });}
                Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
            }
        }),
        Err(error)=>report_prompt_recovery_error(app,context,&capture,&error),
    }
}
/// Synchronous legacy caller bridge only; asynchronous custom saves use the
/// captured overload with the pre-save immutable identity after actual writer ack.
pub(super) fn acknowledge_custom_prompt_recovered_result(app:&AppWindow,context:&AppContext){
    let Ok(capture)=PromptCapture::new(context)else{return;};
    let key=app.global::<AppState>().get_custom_prompt_recovered_request_id().to_string();if key.is_empty(){return;}
    let job=spawn_prompt_job(context,&capture,move|worker|prompt_action_record(worker,&key,None).map(|record|record.identity()));
    match job{
        Ok(job)=>poll_prompt_job(app.as_weak(),context.clone(),capture,job,|app,context,capture,result|match result{
            Ok(identity)=>acknowledge_custom_prompt_recovered_result_captured(app,context,capture.persistence.clone(),identity),
            Err(error)=>report_prompt_recovery_error(app,context,capture,&error),
        }),
        Err(error)=>report_prompt_recovery_error(app,context,&capture,&error),
    }
}
pub(super) fn release_custom_prompt_recovered_result_captured(app:&AppWindow,context:&AppContext,persistence:PrivatePersistence,key:&str){
    let Ok(capture)=PromptCapture::new(context)else{return;};
    if capture.persistence.lease()!=persistence.lease() || !persistence.is_current(){return;}
    capture.apply(context,||{
        let state=app.global::<AppState>();
        if state.get_custom_prompt_recovered_request_id().as_str()==key{
            state.set_custom_prompt_recovered_request_id("".into());
            if state.get_recovered_prompt_client_request_id().as_str()==key{clear_recovered_prompt_presentation(&state);}
        }
    });
    present_next_recovered_prompt_result(app,context);
}
pub(super) fn release_custom_prompt_recovered_result(app:&AppWindow,context:&AppContext){
    let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
    let key=app.global::<AppState>().get_custom_prompt_recovered_request_id().to_string();
    release_custom_prompt_recovered_result_captured(app,context,persistence,&key);
}

pub(super) fn clear_prompt_task_account_state(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_optimizing_prompt(false);
    state.set_translating_prompt(false);
    state.set_custom_prompt_analyzing(false);
    state.set_optimizing_prompt_request_id("".into());
    state.set_translating_prompt_request_id("".into());
    state.set_custom_style_analysis_request_id("".into());
    state.set_custom_prompt_recovered_request_id("".into());
    state.set_optimizing_video_prompt(false);
    state.set_video_prompt_request_id("".into());
    state.set_video_prompt_status("".into());
    state.set_video_prompt_expanded_open(false);
    state.set_video_prompt("".into());
    clear_recovered_prompt_presentation(&state);
}

pub(super) fn normalize_prompt_task_result(raw: &str) -> String {
    let mut candidate = raw.trim().to_string();
    for _ in 0..4 {
        let trimmed = candidate.trim();
        let unwrapped = if trimmed.starts_with('(') && trimmed.ends_with(')') {
            trimmed[1..trimmed.len() - 1].trim()
        } else {
            trimmed
        };
        let Ok(value) = serde_json::from_str::<Value>(unwrapped) else {
            return unwrapped.to_string();
        };
        let Some(decoded) = prompt_text_from_json(&value) else {
            return unwrapped.to_string();
        };
        if decoded.trim() == unwrapped {
            return decoded.trim().to_string();
        }
        candidate = decoded;
    }
    candidate.trim().to_string()
}

fn prompt_text_from_json(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(values) => values.iter().find_map(prompt_text_from_json),
        Value::Object(object) => [
            "prompt",
            "result_prompt",
            "optimized_prompt",
            "content",
            "text",
            "chinese_prompt",
        ]
        .iter()
        .find_map(|key| object.get(*key).and_then(prompt_text_from_json)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_prompt_start_failure_after_exact_upgrade_cannot_project_into_private_ui() {
        i_slint_backend_testing::init_no_event_loop();
        for pre_captured in [false, true] {
            let fixture = backend_generation::billing_capture_test_support::fixture("http://127.0.0.1:9");
            let app = AppWindow::new().unwrap();
            let state = app.global::<AppState>();
            state.set_page("video-generation".into());
            state.set_video_source_id("captured-source".into());
            state.set_video_prompt_status("private projection unchanged".into());
            state.set_optimizing_video_prompt(false);
            let request = PromptTaskRequest { model_code: "original-prompt-model".into(), task_type: "prompt_optimize",
                prompt: "captured video text".into(), target_language: None, optimize: true,
                target: PromptResultTarget::Video { source_id: "captured-source".into(), input: "captured video text".into() }, reference_paths: vec![] };
            fixture.backend.api.upgrade_latch().trip(RequiredUpgrade { minimum_version: None });
            if pre_captured {
                start_backend_prompt_task_with_billing_scope(&app, fixture.context.clone(), fixture.authority.clone(), &fixture.scope, request);
            } else {
                start_backend_prompt_task(&app, fixture.context.clone(), request);
            }
            assert_eq!(state.get_video_prompt_status().as_str(), "private projection unchanged", "captured={pre_captured}");
            assert!(!state.get_optimizing_video_prompt());
            assert!(load_pending_prompt_tasks_for_namespace(&fixture.authority).unwrap().is_empty());
        }
    }

    fn pending_record(target_kind: &str) -> PendingPromptTaskRecord {
        PendingPromptTaskRecord {
            schema_version: 2,
            created_at_epoch_ms: 1,
            client_request_id: "fixed-request-id".to_string(),
            owner_user_id: "user-a".to_string(),
            billing_account_group_id: "22222222-2222-4222-8222-222222222222".to_owned(),
            auth_epoch: 7,
            server_task_id: String::new(),
            task_type: "image_style_analysis".to_string(),
            model_code: "style-model".to_string(),
            prompt: "analyze".to_string(),
            target_language: None,
            optimize: true,
            target_kind: target_kind.to_string(),
            target_id: String::new(),
            target_category: "character".to_string(),
            target_input: "original prompt".to_string(),
            append_result: false,
            activity_kind: "optimize".to_string(),
            reference_paths: vec!["/tmp/reference.png".to_string()],
            reference_sha256: vec!["abc".to_string()],
            reference_size_bytes: vec![3],
            uploaded_file_ids: vec!["file-1".to_string()],
            result_prompt: String::new(),
            terminal_error: String::new(),
            applied_to_target: false,
            result_committed: false,
        }
    }

    #[test]
    fn prompt_terminal_service_failure_never_displays_provider_details() {
        for code in ["INSUFFICIENT_BALANCE", "provider_credentials_unavailable", "provider_disabled"] {
            let message = prompt_terminal_failure_message(Some(code));
            assert!(message.contains("服务暂时不可用"));
            assert!(!message.contains(code));
            assert!(!message.contains("积分"));
        }
        assert_eq!(prompt_terminal_failure_message(Some("unknown secret response")), prompt_terminal_failure_message(None));
    }

    #[test]
    fn video_prompt_progress_only_updates_the_active_original_target() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let context = AppContext::default();
        let state = app.global::<AppState>();
        let mut record = pending_record("video_prompt");
        record.activity_kind = "video_optimize".into();
        record.target_id = "original-source".into();
        state.set_page("video-generation".into());
        state.set_video_source_id(record.target_id.clone().into());
        state.set_video_prompt(record.target_input.clone().into());
        state.set_video_status("quote ready".into());
        set_prompt_task_activity(&app, &record, true);
        for progress in [VideoPromptProgress::Queued, VideoPromptProgress::Processing, VideoPromptProgress::Reconnecting] {
            apply_video_prompt_progress(&app, &context, &record, progress);
            assert_eq!(state.get_video_prompt_status(), progress.message());
            assert_eq!(state.get_video_status(), "quote ready");
            assert!(state.get_optimizing_video_prompt());
        }
        state.set_video_prompt_status("new editor status".into());
        state.set_video_prompt("edited input".into());
        apply_video_prompt_progress(&app, &context, &record, VideoPromptProgress::Queued);
        assert_eq!(state.get_video_prompt_status(), "new editor status");
        state.set_video_prompt(record.target_input.clone().into());
        state.set_video_prompt_request_id("successor".into());
        apply_video_prompt_progress(&app, &context, &record, VideoPromptProgress::Queued);
        assert_eq!(state.get_video_prompt_status(), "new editor status");
        state.set_video_prompt_request_id(record.client_request_id.clone().into());
        state.set_optimizing_video_prompt(false);
        apply_video_prompt_progress(&app, &context, &record, VideoPromptProgress::Queued);
        assert_eq!(state.get_video_prompt_status(), "new editor status");
    }

    #[test]
    fn video_prompt_activity_is_independent_and_only_the_owning_request_can_clear_it() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_optimizing_prompt(true);
        let mut record = pending_record("video_prompt");
        record.activity_kind = "video_optimize".into();
        set_prompt_task_activity(&app, &record, true);
        assert!(state.get_optimizing_video_prompt());
        let mut stale = record.clone();
        stale.client_request_id = "older-request".into();
        clear_prompt_task_activity_if_owned(&app, &stale);
        assert!(state.get_optimizing_video_prompt());
        clear_prompt_task_activity_if_owned(&app, &record);
        assert!(!state.get_optimizing_video_prompt());
        assert!(state.get_optimizing_prompt());
    }

    #[test]
    fn video_prompt_result_requires_the_same_source_page_and_unedited_text() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        let context = AppContext::default();
        state.set_page("video-generation".into());
        state.set_video_source_id("image-a".into());
        state.set_video_prompt("original prompt".into());
        state.set_prompt("image prompt must not change".into());

        let mut record = pending_record("video_prompt");
        record.task_type = "prompt_optimize".into();
        record.target_id = "image-a".into();
        assert!(valid_pending_prompt_task(&record));
        assert!(prompt_target_matches(&app, &context, &record));

        state.set_video_prompt("user edited the video prompt".into());
        assert!(!prompt_target_matches(&app, &context, &record));
        state.set_video_prompt("original prompt".into());
        state.set_video_source_id("image-b".into());
        assert!(!prompt_target_matches(&app, &context, &record));
        state.set_video_source_id("image-a".into());
        state.set_page("generation".into());
        assert!(!prompt_target_matches(&app, &context, &record));
        state.set_page("video-generation".into());
        state.set_video_generating(true);
        assert!(!prompt_target_matches(&app, &context, &record));
        assert_eq!(state.get_prompt(), "image prompt must not change");
    }

    #[test]
    fn idempotent_replay_uses_the_same_request_id_and_complete_body() {
        let record = pending_record("composer");
        let first = prompt_task_create_request(&record);
        let replay = prompt_task_create_request(&record);
        assert_eq!(first.client_request_id, "fixed-request-id");
        assert_eq!(replay.client_request_id, first.client_request_id);
        assert_eq!(replay.task_type, first.task_type);
        assert_eq!(replay.model_code, first.model_code);
        assert_eq!(replay.prompt, first.prompt);
        assert_eq!(replay.reference_file_ids, first.reference_file_ids);
        assert_eq!(replay.target_language, first.target_language);
    }

    #[test]
    fn terminal_result_with_references_rebinds_before_cleanup_launch() {
        let mut record = pending_record("composer");
        record.result_prompt = "paid result".to_string();
        let new_scope = SessionScope {
            owner_user_id: "user-a".to_string(),
            auth_epoch: 11,
        };
        let persisted = std::cell::Cell::new(false);

        let rebound = rebind_prompt_task_epoch(&mut record, &new_scope, |old_record, new_epoch| {
            assert_eq!(old_record.auth_epoch, 7);
            assert_eq!(new_epoch, 11);
            persisted.set(true);
            Ok(true)
        })
        .unwrap();

        assert!(rebound);
        assert!(persisted.get());
        assert!(prompt_task_completed_unclaimed(&record));
        assert!(!record.uploaded_file_ids.is_empty());
        assert_eq!(
            SessionScope {
                owner_user_id: record.owner_user_id.clone(),
                auth_epoch: record.auth_epoch,
            },
            new_scope
        );
    }

    #[test]
    fn committed_terminal_result_with_references_rebinds_before_cleanup_launch() {
        let mut record = pending_record("composer");
        record.result_prompt = "already saved result".to_string();
        record.result_committed = true;
        let new_scope = SessionScope {
            owner_user_id: "user-a".to_string(),
            auth_epoch: 12,
        };

        let rebound = rebind_prompt_task_epoch(&mut record, &new_scope, |old_record, new_epoch| {
            assert!(old_record.result_committed);
            assert_eq!(old_record.auth_epoch, 7);
            assert_eq!(new_epoch, 12);
            Ok(true)
        })
        .unwrap();

        assert!(rebound);
        assert!(record.result_committed);
        assert!(prompt_task_completed_unclaimed(&record));
        assert!(!record.uploaded_file_ids.is_empty());
        assert_eq!(record.auth_epoch, new_scope.auth_epoch);
    }

    #[test]
    fn composer_result_requires_the_original_category_and_prompt_snapshot() {
        let record = pending_record("composer");
        assert!(prompt_target_matches_snapshot(
            &record,
            "character",
            "original prompt",
            "",
            "",
            None,
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "scene",
            "original prompt",
            "",
            "",
            None,
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "new prompt",
            "",
            "",
            None,
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "original prompt",
            "",
            "",
            None,
            &["/tmp/replaced.png".to_string()],
        ));
    }

    #[test]
    fn custom_prompt_result_requires_the_original_session_and_content() {
        let mut record = pending_record("custom_prompt");
        record.target_id = "session-a".to_string();
        assert!(prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "session-a",
            "original prompt",
            None,
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "session-b",
            "original prompt",
            None,
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "session-a",
            "edited prompt",
            None,
            &record.reference_paths,
        ));
    }

    #[test]
    fn canvas_result_requires_the_original_node_content() {
        let record = pending_record("canvas_node");
        assert!(prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "",
            "",
            Some("original prompt"),
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "",
            "",
            Some("edited prompt"),
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "",
            "",
            None,
            &record.reference_paths,
        ));
    }

    #[test]
    fn durable_but_uncommitted_result_can_be_idempotently_reapplied() {
        let mut composer = pending_record("composer");
        composer.result_prompt = "recovered result".to_string();
        assert!(prompt_target_matches_snapshot(
            &composer,
            "character",
            "recovered result",
            "",
            "",
            None,
            &composer.reference_paths,
        ));

        let mut canvas = pending_record("canvas_node");
        canvas.result_prompt = "recovered result".to_string();
        assert!(prompt_target_matches_snapshot(
            &canvas,
            "character",
            "",
            "",
            "",
            Some("recovered result"),
            &canvas.reference_paths,
        ));
    }

    #[test]
    fn transient_retry_backoff_is_bounded() {
        assert_eq!(next_prompt_task_retry_ms(1_000), 2_000);
        assert_eq!(next_prompt_task_retry_ms(16_000), 30_000);
        assert_eq!(next_prompt_task_retry_ms(30_000), 30_000);
    }

    #[test]
    fn idempotency_request_in_progress_is_retried_without_dropping_recovery() {
        let error = ApiError::Http {
            status: 409,
            code: "request_in_progress".to_string(),
            message: "still processing".to_string(),
            request_id: None,
            details: None,
        };
        assert!(prompt_task_api_error_is_transient(&error));
    }

    #[test]
    fn terminal_reference_cleanup_preserves_the_paid_record_and_requests_sign_out() {
        let mut record = pending_record("composer");
        record.result_prompt = "durable paid result".to_string();
        record.result_committed = true;
        let expected_request_id = record.client_request_id.clone();

        let outcome = prompt_task_session_ended(record);

        match outcome {
            PromptTaskOutcome::SessionEnded { record, .. } => {
                assert_eq!(record.client_request_id, expected_request_id);
                assert!(record.result_committed);
                assert!(!record.uploaded_file_ids.is_empty());
            }
            _ => panic!("terminal cleanup must retain the record and signal session end"),
        }
    }

    #[test]
    fn reference_cleanup_classifies_terminal_errors_separately_from_retryable_failures() {
        let terminal = ApiError::Http {
            status: 401,
            code: "session_invalid".to_string(),
            message: "revoked".to_string(),
            request_id: None,
            details: None,
        };
        let retryable = ApiError::Network {
            message: "offline".to_string(),
            timeout: false,
        };

        assert!(classify_prompt_reference_cleanup_error(terminal).is_err());
        assert_eq!(
            classify_prompt_reference_cleanup_error(retryable).unwrap(),
            false
        );
    }

    #[test]
    fn failed_durable_apply_never_marks_the_result_committed() {
        let commit_called = std::cell::Cell::new(false);

        let result = durable_apply_before_result_commit(
            || Err(anyhow!("disk full")),
            || {
                commit_called.set(true);
                Ok(true)
            },
        );

        assert!(result.is_err());
        assert!(!commit_called.get());
    }

    #[test]
    fn result_commit_happens_only_after_durable_apply_succeeds() {
        let events = RefCell::new(Vec::new());

        let result = durable_apply_before_result_commit(
            || {
                events.borrow_mut().push("durable_apply");
                Ok(())
            },
            || {
                events.borrow_mut().push("commit_marker");
                Ok(true)
            },
        );

        assert_eq!(result.unwrap(), true);
        assert_eq!(*events.borrow(), vec!["durable_apply", "commit_marker"]);
    }

    #[test]
    fn tracked_custom_result_pauses_the_entire_recovery_queue() {
        let mut first = pending_record("custom_prompt");
        first.client_request_id = "custom-first".to_string();
        first.result_prompt = "first result".to_string();
        first.applied_to_target = true;
        let mut second = first.clone();
        second.client_request_id = "custom-second".to_string();
        second.created_at_epoch_ms = 2;
        second.result_prompt = "second result".to_string();
        second.applied_to_target = false;

        assert!(next_recovered_prompt_record(
            vec![first, second],
            "user-a",
            "custom-first",
        )
        .is_none());
    }

    #[test]
    fn clearing_custom_tracking_releases_the_next_recovered_result() {
        let mut second = pending_record("custom_prompt");
        second.client_request_id = "custom-second".to_string();
        second.created_at_epoch_ms = 2;
        second.result_prompt = "second result".to_string();

        let selected = next_recovered_prompt_record(vec![second], "user-a", "")
            .expect("next recovered result");

        assert_eq!(selected.client_request_id, "custom-second");
    }
}

#[cfg(test)]
mod billing_capture_tests {
    use super::*;
    use backend_generation::billing_capture_test_support::*;
    use super::core_prompt_tests::fixture;
    fn request() -> PromptTaskRequest {
        PromptTaskRequest {
            model_code: "fixture-model".into(),
            task_type: "prompt_optimize",
            prompt: "fixture prompt".into(),
            target_language: None,
            optimize: true,
            target: PromptResultTarget::Composer {
                category: "other".into(),
                input: "fixture prompt".into(),
            },
            reference_paths: Vec::new(),
        }
    }
    #[test]
    fn billing_capture_prompt_start_persists_before_real_dispatch() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let (listener, url) = listener();
        let mut fixture = fixture(&url);
        let (release, transport) = capture(
            listener,
            fixture.authority.clone(),
            "pending-prompt-tasks.json",
        );
        start_backend_prompt_task_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            request(),
        );
        fixture.scope.request.account_group_id = OTHER.into();
        fixture.scope.context_epoch += 1;
        release.send(()).unwrap();
        let observed = transport.join().unwrap();
        assert_capture(&observed, "prompt_tasks");
        let deadline = Instant::now() + Duration::from_secs(3);
        while !fixture
            .context
            .active_prompt_task_requests
            .lock()
            .unwrap()
            .is_empty()
            && Instant::now() < deadline
        {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(fixture
            .context
            .active_prompt_task_requests
            .lock()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn billing_capture_prompt_storage_and_scope_failure_prevent_dispatch() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let (listener, url) = listener();
        let fixture = fixture(&url);
        corrupt(&fixture.authority, "pending-prompt-tasks.json");
        start_backend_prompt_task_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            request(),
        );
        let mut wrong = fixture.scope.clone();
        wrong.request.session.owner_user_id = OTHER.into();
        start_backend_prompt_task_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.authority.clone(),
            &wrong,
            request(),
        );
        join_prompt_workers().unwrap();
        let deadline=Instant::now()+Duration::from_secs(3);
        while !fixture.context.active_prompt_task_requests.lock().unwrap().is_empty() && Instant::now()<deadline{
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();std::thread::sleep(Duration::from_millis(2));
        }
        assert!(fixture
            .context
            .active_prompt_task_requests
            .lock()
            .unwrap()
            .is_empty());
        assert_no_request(&listener);
    }
}

#[cfg(test)]
mod core_prompt_tests {
    use super::*;
    use std::io::{Read,Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool,Ordering};
    const OWNER:&str="11111111-1111-4111-8111-111111111111";
    const PAYER:&str="22222222-2222-4222-8222-222222222222";
    const OTHER:&str="33333333-3333-4333-8333-333333333333";
    const KEY:&str="44444444-4444-4444-8444-444444444444";
    const TASK:&str="55555555-5555-4555-8555-555555555555";
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
        let index = FileIndex::initialize(path.join("prompt-index.sqlite3")).unwrap();
        let backend = Arc::new(BackendRuntime { api: ApiClient::new(ApiClientConfig {
            base_url: reqwest::Url::parse(url).unwrap(), app_version: "999.0.0".into(),
            timeout: Duration::from_secs(3),
        }, DeviceIdentity { id: OTHER.into(), name: "prompt-fixture".into(), platform: "macos".into() },
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
            {
                let mut active=self.context.active_namespace.lock().unwrap();
                if active.as_ref()==Some(self.persistence.lease()){active.take();}
            }
            cancel_prompt_workers_for_retirement(self.persistence.lease());
            let joined = join_prompt_workers();
            release_prompt_result_actions(Some(self.persistence.lease()));
            if !std::thread::panicking() { assert_eq!(joined.is_err(),self.expected_join_failure,"prompt worker join outcome"); }
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

    fn app(f:&Fixture)->AppWindow {
        let app=AppWindow::new().unwrap();let state=app.global::<AppState>();
        state.set_session_state("online".into());state.set_asset_type("character".into());
        state.set_prompt("original input".into());state.set_page("generation".into());
        wire_prompt_task_recovery_callbacks(&app,f.context.clone());app
    }
    fn row(f:&Fixture,kind:&str)->PendingPromptTaskRecord {
        PendingPromptTaskRecord{
            schema_version:2,created_at_epoch_ms:1,client_request_id:KEY.into(),owner_user_id:OWNER.into(),
            billing_account_group_id:PAYER.into(),auth_epoch:f.scope.request.session.auth_epoch,
            server_task_id:TASK.into(),task_type:"prompt_optimize".into(),model_code:"fixture-model".into(),
            prompt:"original input".into(),target_language:None,optimize:true,target_kind:kind.into(),
            target_id:if kind=="composer"{String::new()}else{OTHER.into()},target_category:"character".into(),
            target_input:"original input".into(),append_result:false,activity_kind:"optimize".into(),
            reference_paths:vec![],reference_sha256:vec![],reference_size_bytes:vec![],uploaded_file_ids:vec![],
            result_prompt:"paid result".into(),terminal_error:String::new(),applied_to_target:false,result_committed:false,
        }
    }
    fn seed(f:&Fixture,record:PendingPromptTaskRecord){
        upsert_pending_prompt_task_for_namespace(&f.authority,&f.scope,record).unwrap();
    }
    fn rows(f:&Fixture)->Vec<PendingPromptTaskRecord>{load_pending_prompt_tasks_for_namespace(&f.authority).unwrap()}
    fn response(status:u16,data:Value,code:&str)->String {
        let body=serde_json::json!({"request_id":"fixture","data":data,"meta":null,
            "error":if status==200{Value::Null}else{serde_json::json!({"code":code,"message":"controlled failure","details":null})}}).to_string();
        format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())
    }
    fn task_response()->String {response(200,serde_json::json!({
        "id":TASK,"billing_account_group_id":PAYER,"status":"completed","progress_percent":100,
        "success_count":1,"failure_count":0,"failure":null,"prompt":"original input",
        "result_prompt":"paid result","items":[]}),"")}
    fn request()->PromptTaskRequest {
        PromptTaskRequest{model_code:"fixture-model".into(),task_type:"prompt_optimize",prompt:"original input".into(),
            target_language:None,optimize:true,
            target:PromptResultTarget::Composer{category:"character".into(),input:"original input".into()},reference_paths:vec![]}
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
        assert!(ready(),"prompt completion was not observed");
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
            // A preconnect with no HTTP request must not consume a controlled response slot.
            let response_slots=Arc::new(Mutex::new((0usize,receivers)));
            let handle=std::thread::spawn(move || {
                let deadline=Instant::now()+Duration::from_secs(12); let mut children=Vec::new();
                while !worker_stop.load(Ordering::Acquire) && Instant::now()<deadline {
                    match listener.accept() {
                        Ok((mut stream,_)) => {
                            let seen_tx=seen_tx.clone();let response_slots=response_slots.clone();
                            children.push(std::thread::spawn(move || {
                                stream.set_nonblocking(false).unwrap();
                                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                                stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
                                let mut bytes=Vec::new();let mut block=[0u8;1024];
                                let header_end=loop{
                                    if let Some(end)=bytes.windows(4).position(|value|value==b"\r\n\r\n"){
                                        assert!(end+4<=16384,"bounded prompt headers exceeded");break end+4;
                                    }
                                    assert!(bytes.len()<16384,"bounded prompt headers exceeded");
                                    let count=match stream.read(&mut block){
                                        Ok(count)=>count,
                                        Err(error) if bytes.is_empty() && matches!(error.kind(),std::io::ErrorKind::TimedOut|std::io::ErrorKind::WouldBlock)=>return None,
                                        Err(error)=>panic!("incomplete prompt fixture headers: {error}"),
                                    };
                                    if count==0{assert!(bytes.is_empty(),"incomplete prompt fixture headers");return None;}
                                    bytes.extend_from_slice(&block[..count]);
                                };
                                let header=std::str::from_utf8(&bytes[..header_end]).unwrap();
                                let lengths=header.lines().filter_map(|line|line.split_once(':'))
                                    .filter(|(name,_)|name.eq_ignore_ascii_case("content-length"))
                                    .map(|(_,value)|value.trim().parse::<usize>().unwrap()).collect::<Vec<_>>();
                                assert!(lengths.len()<=1,"duplicate fixture content length");
                                assert!(!header.lines().filter_map(|line|line.split_once(':')).any(|(name,_)|name.eq_ignore_ascii_case("transfer-encoding")),"fixture expects bounded content length");
                                let length=lengths.first().copied().unwrap_or(0);
                                assert!(length<=16384,"bounded prompt body exceeded");
                                let total=header_end.checked_add(length).unwrap();
                                assert!(bytes.len()<=total,"unexpected pipelined fixture bytes");
                                while bytes.len()<total{
                                    let remaining=(total-bytes.len()).min(block.len());
                                    let count=stream.read(&mut block[..remaining]).expect("incomplete prompt fixture body");
                                    assert!(count>0,"incomplete prompt fixture body");bytes.extend_from_slice(&block[..count]);
                                }
                                let header=std::str::from_utf8(&bytes[..header_end]).unwrap();
                                // Account refreshes are independent of the controlled task exchange.
                                // Reject the snapshot explicitly so it cannot start profile/catalog follow-ups.
                                if header.starts_with("GET /v1/account ") {
                                    assert!(header.to_ascii_lowercase().contains("x-account-group-id:"));
                                    let _ = stream.write_all(response(400, Value::Null, "fixture_refresh_refused").as_bytes());
                                    return None;
                                }
                                let(index,reply)={let mut slots=response_slots.lock().unwrap();let index=slots.0;
                                    slots.0=slots.0.checked_add(1).unwrap();(index,slots.1.get_mut(index).and_then(Option::take))};
                                seen_tx.send(index).unwrap();
                                let value=reply.and_then(|rx|rx.recv_timeout(Duration::from_secs(5)).ok())
                                    .unwrap_or_else(||response(400,Value::Null,"fixture_refused"));
                                let _=stream.write_all(value.as_bytes());
                                Some(String::from_utf8(bytes).unwrap())
                            }));
                        }
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
                        Err(_)=>panic!("fixture listener failed"),
                    }
                }
                let mut requests=Vec::new();let mut failed=false;
                for child in children {match child.join(){Ok(Some(request))=>requests.push(request),Ok(None)=>{},Err(_)=>failed=true}}
                assert!(!failed,"fixture connection panicked");requests
            });
            Self{url,seen,replies,stop,handle:Some(handle)}
        }
        fn wait(&self){self.seen.recv_timeout(Duration::from_secs(4)).expect("prompt request was not dispatched");}
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
    fn video_prompt_worker_reports_queue_processing_reconnect_and_safe_failure() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(4);let f=fixture(&transport.url);let app=app(&f);
        let state=app.global::<AppState>();
        state.set_page("video-generation".into());
        state.set_video_source_id("source-a".into());
        state.set_video_prompt("original input".into());
        state.set_video_status("quote ready".into());
        let mut task=request();
        task.target=PromptResultTarget::Video{source_id:"source-a".into(),input:"original input".into()};
        start_backend_prompt_task(&app,f.context.clone(),task);
        for (index, status, progress) in [
            (0,"queued",VideoPromptProgress::Queued),
            (1,"running",VideoPromptProgress::Processing),
        ] {
            transport.wait();
            transport.reply(index,response(200,serde_json::json!({"id":TASK,"billing_account_group_id":PAYER,
                "status":status,"progress_percent":0,"success_count":0,"failure_count":0,
                "failure":null,"prompt":"original input","result_prompt":null,"items":[]}),""));
            pump(||state.get_video_prompt_status()==progress.message());
            assert!(state.get_optimizing_video_prompt());
        }
        transport.wait();transport.reply(2,response(503,Value::Null,"temporary_failure"));
        pump(||state.get_video_prompt_status()==VideoPromptProgress::Reconnecting.message());
        transport.wait();transport.reply(3,response(200,serde_json::json!({"id":TASK,"billing_account_group_id":PAYER,
            "status":"failed","progress_percent":0,"success_count":0,"failure_count":1,
            "failure":{"code":"INSUFFICIENT_BALANCE","message":"private provider response"},
            "prompt":"original input","result_prompt":null,"items":[]}),""));
        pump(||!state.get_optimizing_video_prompt());
        pump(||state.get_recovered_prompt_result_open());join_prompt_workers().unwrap();
        assert!(state.get_recovered_prompt_error().contains("服务暂时不可用"));
        assert!(!state.get_recovered_prompt_error().contains("private provider response"));
        assert!(!state.get_recovered_prompt_error().contains("INSUFFICIENT_BALANCE"));
        assert_eq!(state.get_video_prompt(),"original input");
        assert_eq!(state.get_video_status(),"quote ready");
        let requests=transport.finish();
        assert_eq!(requests.iter().filter(|request|request.starts_with("POST /v1/generation/tasks ")).count(),1);
    }

    #[test]
    fn core_prompt_real_start_late_upgrade_result_cannot_clear_activity_or_status() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        start_backend_prompt_task(&app,f.context.clone(),request());transport.wait();
        let saved=rows(&f);assert_eq!(saved.len(),1);assert_eq!(saved[0].billing_account_group_id,PAYER);
        let trip=JoinedTrip::start(f.backend.api.upgrade_latch().clone());
        let state=app.global::<AppState>();state.set_generation_status("upgrade-boundary".into());
        state.set_optimizing_prompt(true);
        transport.reply(0,task_response());trip.join();join_prompt_workers().unwrap();
        pump_for(Duration::from_millis(50));
        assert_eq!(state.get_generation_status(),"upgrade-boundary");
        assert!(state.get_optimizing_prompt());assert_eq!(state.get_prompt(),"original input");
        assert_eq!(rows(&f)[0].client_request_id,saved[0].client_request_id);
        let requests=transport.finish();assert_eq!(requests.len(),1);
        assert!(requests[0].starts_with("POST /v1/generation/tasks "));
        assert!(requests[0].contains(&saved[0].client_request_id));
    }
    #[test]
    fn core_prompt_missing_store_binding_rejects_new_start_before_durable_row_or_transport() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        f.context.store.borrow_mut().private_persistence=None;
        let state=app.global::<AppState>();state.set_generation_status("unchanged".into());
        start_backend_prompt_task(&app,f.context.clone(),request());
        join_prompt_workers().unwrap();pump_for(Duration::from_millis(30));
        assert!(rows(&f).is_empty());assert_eq!(state.get_generation_status(),"unchanged");
        assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_prompt_held_http_after_original_lease_cancel_cannot_publish_into_replaced_store() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        start_backend_prompt_task(&app,f.context.clone(),request());transport.wait();let original=rows(&f)[0].clone();
        cancel_prompt_workers_for_retirement(f.persistence.lease());
        f.context.store.borrow_mut().private_persistence=None;*f.context.active_namespace.lock().unwrap()=None;
        app.global::<AppState>().set_prompt("replacement editor".into());app.global::<AppState>().set_generation_status("replacement boundary".into());
        transport.reply(0,task_response());join_prompt_workers().unwrap();pump_for(Duration::from_millis(60));
        assert_eq!(app.global::<AppState>().get_prompt(),"replacement editor");assert_eq!(app.global::<AppState>().get_generation_status(),"replacement boundary");
        assert_eq!(serde_json::to_value(&rows(&f)[0]).unwrap(),serde_json::to_value(original).unwrap());
        assert!(PROMPT_THREADS.with(|workers|workers.borrow().is_empty()));assert_eq!(transport.finish().len(),1);
    }
    #[test]
    fn core_prompt_claim_writer_rejection_keeps_original_visible_text_and_paid_row() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        seed(&f,row(&f,"composer"));present_next_recovered_prompt_result(&app,&f.context);pump(||app.global::<AppState>().get_recovered_prompt_result_open());
        f.writer.deactivate(f.persistence.lease()).unwrap();
        app.global::<AppState>().invoke_apply_recovered_prompt_result();
        join_prompt_workers().unwrap();pump_for(Duration::from_millis(50));
        assert_eq!(app.global::<AppState>().get_prompt(),"original input");
        assert!(app.global::<AppState>().get_recovered_prompt_result_open());
        let saved=rows(&f);assert_eq!(saved.len(),1);assert!(!saved[0].result_committed);
        assert_eq!(saved[0].result_prompt,"paid result");assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_prompt_recovery_read_failure_preserves_visible_claim_and_discard() {
        i_slint_backend_testing::init_no_event_loop();
        for discard in [false,true] {
            let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
            seed(&f,row(&f,"composer"));present_next_recovered_prompt_result(&app,&f.context);pump(||app.global::<AppState>().get_recovered_prompt_result_open());
            let path=f.persistence.lease().namespace.path(ManagedUserArea::Recovery).join("pending-prompt-tasks.json");
            // Fault injection only into this fixture's explicitly owned recovery file.
            fs::write(&path,b"retained-invalid-document").unwrap();
            let state=app.global::<AppState>();
            if discard{state.invoke_dismiss_recovered_prompt_result();}else{state.invoke_apply_recovered_prompt_result();}
            join_prompt_workers().unwrap();pump_for(Duration::from_millis(30));
            assert!(state.get_recovered_prompt_result_open());assert_eq!(state.get_recovered_prompt_result(),"paid result");
            assert_eq!(state.get_recovered_prompt_client_request_id(),KEY);assert_eq!(state.get_prompt(),"original input");
            assert_eq!(fs::read(&path).unwrap(),b"retained-invalid-document");assert!(transport.finish().is_empty());
        }
    }
    #[test]
    fn core_prompt_claim_copy_discard_and_custom_release_refuse_exact_upgrade() {
        i_slint_backend_testing::init_no_event_loop();
        for action in 0..4 {
            let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
            seed(&f,row(&f,"composer"));present_next_recovered_prompt_result(&app,&f.context);pump(||app.global::<AppState>().get_recovered_prompt_result_open());
            let state=app.global::<AppState>();state.set_custom_prompt_recovered_request_id(KEY.into());
            f.backend.api.upgrade_latch().trip(RequiredUpgrade{minimum_version:None});
            with_prompt_clipboard_test(|_|panic!("clipboard must not be called after upgrade"),||match action{
                0=>state.invoke_apply_recovered_prompt_result(),1=>state.invoke_copy_recovered_prompt_result(),
                2=>state.invoke_dismiss_recovered_prompt_result(),_=>release_custom_prompt_recovered_result(&app,&f.context),
            });
            join_prompt_workers().unwrap();pump_for(Duration::from_millis(30));
            assert!(state.get_recovered_prompt_result_open());assert_eq!(state.get_recovered_prompt_result(),"paid result");
            assert_eq!(state.get_custom_prompt_recovered_request_id(),KEY);assert_eq!(state.get_prompt(),"original input");
            assert_eq!(rows(&f).len(),1);assert!(transport.finish().is_empty());
        }
    }
    #[test]
    fn core_prompt_clipboard_late_result_cannot_publish_into_replaced_store_binding() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        seed(&f,row(&f,"composer"));present_next_recovered_prompt_result(&app,&f.context);pump(||app.global::<AppState>().get_recovered_prompt_result_open());
        let context=f.context.clone();let weak=app.as_weak();
        with_prompt_clipboard_test(move|text|{
            assert_eq!(text,"paid result");
            context.store.borrow_mut().private_persistence=None;
            *context.active_namespace.lock().unwrap()=None;
            weak.upgrade().unwrap().global::<AppState>().set_generation_status("new-binding-boundary".into());
            Ok(())
        },||{app.global::<AppState>().invoke_copy_recovered_prompt_result();pump(||app.global::<AppState>().get_generation_status()=="new-binding-boundary");});
        assert_eq!(app.global::<AppState>().get_generation_status(),"new-binding-boundary");
        assert_eq!(rows(&f).len(),1);assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_prompt_normal_composer_and_video_claims_require_real_sqlite_content() {
        i_slint_backend_testing::init_no_event_loop();
        for video in [false,true] {
            let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
            let state=app.global::<AppState>();
            if video{state.set_page("video-generation".into());state.set_video_source_id(OTHER.into());state.set_video_prompt("original input".into());}
            seed(&f,row(&f,if video{"video_prompt"}else{"composer"}));present_next_recovered_prompt_result(&app,&f.context);pump(||app.global::<AppState>().get_recovered_prompt_result_open());
            state.invoke_apply_recovered_prompt_result();
            pump(||if video{state.get_video_prompt()=="paid result"}else{state.get_prompt()=="paid result"});
            join_prompt_workers().unwrap();
            let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
            assert!(serde_json::to_string(&durable.prompt_drafts).unwrap().contains("paid result"));
            pump(||rows(&f).iter().all(|record|record.result_committed));
            assert!(transport.finish().is_empty());
        }
    }
    #[test]
    fn core_prompt_edited_target_keeps_paid_result_until_explicit_accept() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        let record=row(&f,"composer");seed(&f,record.clone());
        app.global::<AppState>().set_prompt("user edited input".into());
        assert!(matches!(apply_prompt_result_if_target_matches(&app,&f.context,&record),PromptResultApplication::NotApplied));
        assert_eq!(app.global::<AppState>().get_prompt(),"user edited input");
        assert!(!rows(&f)[0].result_committed);
        present_next_recovered_prompt_result(&app,&f.context);pump(||app.global::<AppState>().get_recovered_prompt_result_open());app.global::<AppState>().invoke_apply_recovered_prompt_result();
        pump(||app.global::<AppState>().get_prompt()=="paid result");join_prompt_workers().unwrap();
        assert!(transport.finish().is_empty());
    }

    struct ReleasePromptWorker(Option<mpsc::Sender<()>>);
    impl ReleasePromptWorker {fn release(mut self){self.0.take().unwrap().send(()).unwrap();}}
    impl Drop for ReleasePromptWorker{fn drop(&mut self){if let Some(tx)=self.0.take(){let _=tx.send(());}}}
    #[test]
    fn core_prompt_duplicate_result_action_cannot_release_the_original_guard() {
        let f=fixture("http://127.0.0.1:9/");
        let capture=PromptCapture::new(&f.context).unwrap();
        let original=PromptResultAction::begin(&capture,KEY).unwrap();
        let set=original.reservation.active.clone();
        assert!(PromptResultAction::begin(&capture,KEY).is_none());
        assert_eq!(set.borrow().len(),1);
        drop(original);
        assert!(set.borrow().is_empty());
    }
    #[test]
    fn core_prompt_repeated_claim_and_copy_keep_one_actual_worker_until_result_disposal() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        seed(&f,row(&f,"composer"));
        present_next_recovered_prompt_result(&app,&f.context);
        pump(||app.global::<AppState>().get_recovered_prompt_result_open());
        let(sent_tx,sent_rx)=mpsc::channel();let(release_tx,release_rx)=mpsc::channel();
        PROMPT_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{
            sent_tx.send(()).unwrap();release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        })));
        let release=ReleasePromptWorker(Some(release_tx));
        app.global::<AppState>().invoke_apply_recovered_prompt_result();
        sent_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        with_prompt_clipboard_test(|_|panic!("another result action cannot reach clipboard"),||{
            app.global::<AppState>().invoke_copy_recovered_prompt_result();
            app.global::<AppState>().invoke_apply_recovered_prompt_result();
        });
        assert_eq!(PROMPT_THREADS.with(|workers|workers.borrow().len()),1);
        assert_eq!(PROMPT_RESULT_ACTIONS.with(|active|active.borrow().len()),1);
        release.release();
        pump(||app.global::<AppState>().get_prompt()=="paid result");
        pump(||rows(&f).is_empty() && PROMPT_RESULT_ACTIONS.with(|active|active.borrow().is_empty()));
        join_prompt_workers().unwrap();
        let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(prompt_draft_for_category(&durable.prompt_drafts,"character"),"paid result");
        assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_prompt_result_is_not_consumed_until_real_registered_worker_exits() {
        i_slint_backend_testing::init_no_event_loop();
        let f=fixture("http://127.0.0.1:9/");let app=app(&f);let capture=PromptCapture::new(&f.context).unwrap();
        let (sent_tx,sent_rx)=mpsc::channel();let (release_tx,release_rx)=mpsc::channel();
        PROMPT_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{sent_tx.send(()).unwrap();release_rx.recv_timeout(Duration::from_secs(3)).unwrap();})));
        let release=ReleasePromptWorker(Some(release_tx));
        let job=spawn_prompt_job(&f.context,&capture,|_|Ok(())).unwrap();
        let observed=Rc::new(std::cell::Cell::new(false));let changed=observed.clone();
        poll_prompt_job(app.as_weak(),f.context.clone(),capture,job,move|_,_,_,result|{result.unwrap();changed.set(true);});
        sent_rx.recv_timeout(Duration::from_secs(3)).unwrap();pump_for(Duration::from_millis(60));
        assert!(!observed.get());release.release();pump(||observed.get());
        assert!(PROMPT_THREADS.with(|workers|workers.borrow().is_empty()));join_prompt_workers().unwrap();
    }
    #[test]
    fn core_prompt_reaped_panic_is_sticky_at_empty_shutdown() {
        let mut f=fixture("http://127.0.0.1:9/");f.expected_join_failure=true;
        let capture=PromptCapture::new(&f.context).unwrap();
        let job=spawn_prompt_job::<()>(&f.context,&capture,|_|panic!("controlled prompt worker panic")).unwrap();
        assert!(job.receiver.recv_timeout(Duration::from_secs(3)).is_err());
        let deadline=Instant::now()+Duration::from_secs(3);
        while prompt_worker_pending(&job.id) && Instant::now()<deadline{reap_prompt_workers();std::thread::sleep(Duration::from_millis(1));}
        assert!(!prompt_worker_pending(&job.id));assert!(shutdown_prompt_workers().is_err());assert!(join_prompt_workers().is_err());
    }
    #[test]
    fn core_prompt_registered_backoff_cancels_without_waiting_thirty_seconds() {
        let f=fixture("http://127.0.0.1:9/");let capture=PromptCapture::new(&f.context).unwrap();
        let(tx,rx)=mpsc::channel();
        let job=spawn_prompt_job(&f.context,&capture,move|worker|{tx.send(()).unwrap();Ok(worker.wait(Duration::from_secs(30)))}).unwrap();
        rx.recv_timeout(Duration::from_secs(3)).unwrap();cancel_prompt_workers_for_retirement(f.persistence.lease());
        assert!(!job.receiver.recv_timeout(Duration::from_secs(1)).unwrap().unwrap());join_prompt_workers().unwrap();
    }
    #[test]
    fn core_prompt_canvas_result_uses_real_writer_and_original_history_without_callback_reentry() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        f.context.store.borrow_mut().canvas_notes.push(CanvasNoteData{id:OTHER.into(),content:"original input".into(),..Default::default()});
        app.global::<AppState>().set_canvas_notes(ModelRc::new(VecModel::from(vec![CanvasNote{id:OTHER.into(),content:"original input".into(),..Default::default()}])));
        app.global::<AppState>().on_update_canvas_node(|_,_,_,_|panic!("paid result must not reenter callback persistence"));
        let record=row(&f,"canvas_node");seed(&f,record.clone());
        assert!(matches!(apply_prompt_result_if_target_matches(&app,&f.context,&record),PromptResultApplication::AppliedWithCleanupPending));
        pump(||app.global::<AppState>().get_canvas_notes().row_data(0).unwrap().content=="paid result");join_prompt_workers().unwrap();
        let data=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(data.canvas_notes[0].content,"paid result");assert!(f.context.canvas_history.borrow().can_undo());
        pump(||rows(&f).iter().all(|record|record.result_committed));assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_prompt_custom_result_retains_row_until_real_independent_save_ack() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);let state=app.global::<AppState>();
        state.set_custom_prompt_editor_open(true);state.set_custom_prompt_editor_session_id(OTHER.into());state.set_custom_prompt_input("original input".into());
        seed(&f,row(&f,"custom_prompt"));present_next_recovered_prompt_result(&app,&f.context);pump(||state.get_recovered_prompt_result_open());
        state.invoke_apply_recovered_prompt_result();pump(||state.get_custom_prompt_input()=="paid result");join_prompt_workers().unwrap();
        let saved=rows(&f);assert_eq!(saved.len(),1);assert!(saved[0].applied_to_target);assert!(!saved[0].result_committed);
        let mut store=f.context.store.borrow_mut();
        assert_eq!(save_custom_prompt_to_store(&mut store,"","paid result","fixture-time"),SaveCustomPromptResult::Saved);drop(store);
        // Actual independent custom save, not a fake successful acknowledgement.
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        acknowledge_custom_prompt_recovered_result_captured(&app,&f.context,f.persistence.clone(),saved[0].identity());
        pump(||state.get_custom_prompt_recovered_request_id().is_empty());join_prompt_workers().unwrap();
        assert!(rows(&f).is_empty());
        let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();assert!(durable.custom_prompts.iter().any(|value|value=="paid result"));
        assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_prompt_new_edit_before_queued_ack_completion_is_not_overwritten_or_committed() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);let state=app.global::<AppState>();
        seed(&f,row(&f,"composer"));present_next_recovered_prompt_result(&app,&f.context);pump(||state.get_recovered_prompt_result_open());
        state.invoke_apply_recovered_prompt_result();
        // One dispatch at a time: the staging callback schedules the real ack
        // timer, which must not be dispatched before this test's new user edit.
        let deadline=Instant::now()+Duration::from_secs(5);
        while prompt_draft_for_category(&f.context.store.borrow().prompt_drafts,"character")!="paid result" && Instant::now()<deadline {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(prompt_draft_for_category(&f.context.store.borrow().prompt_drafts,"character"),"paid result");
        assert_eq!(state.get_prompt(),"original input","the original ack timer must still be pending");
        let retained=rows(&f);assert_eq!(retained.len(),1);assert!(!retained[0].result_committed);
        // The real ordered write was queued; do not dispatch its UI acknowledgement yet.
        state.set_prompt("new user edit".into());
        set_prompt_draft_for_category(&mut f.context.store.borrow_mut().prompt_drafts,"character","new user edit".into());
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        join_prompt_workers().unwrap();pump_for(Duration::from_millis(60));join_prompt_workers().unwrap();
        assert_eq!(state.get_prompt(),"new user edit");
        let retained=rows(&f);assert_eq!(retained.len(),1,"a newer edit keeps the original paid result");assert!(!retained[0].result_committed);
        let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(prompt_draft_for_category(&durable.prompt_drafts,"character"),"new user edit");assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_prompt_failed_cleanup_retry_never_reapplies_over_a_new_editor_value() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);let state=app.global::<AppState>();
        seed(&f,row(&f,"composer"));present_next_recovered_prompt_result(&app,&f.context);pump(||state.get_recovered_prompt_result_open());
        let(tx,rx)=mpsc::channel();let(release_tx,release_rx)=mpsc::channel();
        PROMPT_BEFORE_REMOVE.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{tx.send(()).unwrap();release_rx.recv_timeout(Duration::from_secs(3)).unwrap();})));
        let release=ReleasePromptWorker(Some(release_tx));state.invoke_apply_recovered_prompt_result();
        let reached=std::cell::Cell::new(false);pump(||{if rx.try_recv().is_ok(){reached.set(true);}reached.get()});
        assert_eq!(state.get_prompt(),"paid result");assert!(rows(&f)[0].result_committed);
        let path=f.persistence.lease().namespace.path(ManagedUserArea::Recovery).join("pending-prompt-tasks.json");
        let committed=fs::read(&path).unwrap();fs::write(&path,b"cleanup-write-fault").unwrap();
        release.release();join_prompt_workers().unwrap();pump_for(Duration::from_millis(60));
        assert!(state.get_recovered_prompt_result_open());
        // Remove only the injected fixture fault, restoring the exact retained bytes.
        fs::write(&path,committed).unwrap();state.set_prompt("new user edit".into());
        set_prompt_draft_for_category(&mut f.context.store.borrow_mut().prompt_drafts,"character","new user edit".into());
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        state.invoke_apply_recovered_prompt_result();pump(||rows(&f).is_empty());join_prompt_workers().unwrap();pump_for(Duration::from_millis(40));
        assert_eq!(state.get_prompt(),"new user edit");
        let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(prompt_draft_for_category(&durable.prompt_drafts,"character"),"new user edit");assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_prompt_repeated_discovery_after_epoch_rebind_keeps_one_original_worker() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(3);let f=fixture(&transport.url);let app=app(&f);
        let mut original=row(&f,"composer");original.result_prompt.clear();
        let retained=seed_old_prompt_fixture(&f,original);
        recover_pending_prompt_tasks(&app,f.context.clone());
        pump(||PROMPT_DISCOVERY_COMPLETED.with(|count|count.get())==1);transport.wait();
        transport.reply(0,response(200,serde_json::json!({"id":TASK,"billing_account_group_id":PAYER,
            "status":"running","progress_percent":10,"success_count":0,"failure_count":0,
            "failure":null,"prompt":"original input","result_prompt":null,"items":[]}),""));
        transport.wait(); // Exact header-free validation completed, epoch rebound, normal polling now held.
        assert_eq!(rows(&f)[0].auth_epoch,f.scope.request.session.auth_epoch);
        assert_eq!(rows(&f)[0].billing_account_group_id,retained.billing_account_group_id);
        recover_pending_prompt_tasks(&app,f.context.clone());
        pump(||PROMPT_DISCOVERY_COMPLETED.with(|count|count.get())==2);
        assert_eq!(f.context.active_prompt_task_requests.lock().unwrap().len(),1,"rebound row must reuse original active reservation");
        assert!(transport.seen.try_recv().is_err(),"duplicate recovery dispatched another HTTP request");
        transport.reply(1,response(403,Value::Null,"group_frozen"));join_prompt_workers().unwrap();pump_for(Duration::from_millis(60));
        let requests=transport.finish();assert_eq!(requests.len(),2);
        for request in requests{assert!(request.starts_with(&format!("GET /v1/generation/tasks/{TASK} ")));assert!(!request.to_ascii_lowercase().contains("x-account-group-id:"));}
        assert_eq!(rows(&f)[0].client_request_id,retained.client_request_id);assert_eq!(rows(&f)[0].billing_account_group_id,PAYER);
    }
    #[test]
    fn core_prompt_transport_idle_preconnect_does_not_consume_controlled_reply() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        let address=transport.url.trim_start_matches("http://").trim_end_matches('/');
        drop(std::net::TcpStream::connect(address).unwrap());
        start_backend_prompt_task(&app,f.context.clone(),request());transport.wait();
        transport.reply(0,response(403,Value::Null,"group_frozen"));join_prompt_workers().unwrap();pump_for(Duration::from_millis(60));
        let requests=transport.finish();assert_eq!(requests.len(),1);
        assert!(requests[0].starts_with("POST /v1/generation/tasks "));
        assert!(requests[0].contains(&rows(&f)[0].client_request_id));
    }
    #[test]
    fn core_prompt_finished_worker_keeps_reservation_until_original_completion_then_allows_successor() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(3);let f=fixture(&transport.url);let app=app(&f);
        let mut record=row(&f,"composer");record.result_prompt.clear();seed(&f,record.clone());
        let capture=PromptCapture::new(&f.context).unwrap();
        launch_prompt_record(&app,f.context.clone(),capture.clone(),None,record.clone(),false,false);
        transport.wait();transport.reply(0,response(403,Value::Null,"group_frozen"));join_prompt_workers().unwrap();
        // Result is sent and the real worker joined, but its original UI completion has not run.
        launch_prompt_record(&app,f.context.clone(),capture.clone(),None,record.clone(),false,false);
        assert!(PROMPT_THREADS.with(|workers|workers.borrow().is_empty()),"a successor escaped before original completion disposed its reservation");
        assert_eq!(f.context.active_prompt_task_requests.lock().unwrap().len(),1);
        pump_for(Duration::from_millis(60));assert!(f.context.active_prompt_task_requests.lock().unwrap().is_empty());
        launch_prompt_record(&app,f.context.clone(),capture,None,record,false,false);
        transport.wait();pump_for(Duration::from_millis(60));
        assert_eq!(f.context.active_prompt_task_requests.lock().unwrap().len(),1,"old completion must not erase a successor reservation");
        transport.reply(1,response(403,Value::Null,"group_frozen"));join_prompt_workers().unwrap();pump_for(Duration::from_millis(60));
        assert!(f.context.active_prompt_task_requests.lock().unwrap().is_empty());assert_eq!(transport.finish().len(),2);
    }
    #[test]
    fn core_prompt_shutdown_releases_original_pending_ui_reservations_without_timer_dispatch() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        let mut record=row(&f,"composer");record.result_prompt.clear();seed(&f,record.clone());
        let capture=PromptCapture::new(&f.context).unwrap();
        launch_prompt_record(&app,f.context.clone(),capture,None,record,false,false);
        transport.wait();transport.reply(0,response(403,Value::Null,"group_frozen"));join_prompt_workers().unwrap();
        assert_eq!(f.context.active_prompt_task_requests.lock().unwrap().len(),1);
        shutdown_prompt_workers().unwrap();
        assert!(f.context.active_prompt_task_requests.lock().unwrap().is_empty());
        app.global::<AppState>().set_generation_status("shutdown boundary".into());pump_for(Duration::from_millis(60));
        assert_eq!(app.global::<AppState>().get_generation_status(),"shutdown boundary");assert_eq!(transport.finish().len(),1);
    }
    #[test]
    fn core_prompt_dropped_window_disposes_original_completion_reservation() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport=Transport::new(1);let f=fixture(&transport.url);let app=app(&f);
        let mut record=row(&f,"composer");record.result_prompt.clear();seed(&f,record.clone());
        let capture=PromptCapture::new(&f.context).unwrap();
        launch_prompt_record(&app,f.context.clone(),capture,None,record,false,false);
        transport.wait();transport.reply(0,response(403,Value::Null,"group_frozen"));join_prompt_workers().unwrap();
        assert_eq!(f.context.active_prompt_task_requests.lock().unwrap().len(),1);
        drop(app);pump_for(Duration::from_millis(60));
        assert!(f.context.active_prompt_task_requests.lock().unwrap().is_empty());assert_eq!(transport.finish().len(),1);
    }
    fn seed_old_prompt_fixture(f:&Fixture,mut record:PendingPromptTaskRecord)->PendingPromptTaskRecord {
        seed(f,record.clone());record.auth_epoch=record.auth_epoch.checked_sub(1).unwrap();
        let path=f.persistence.lease().namespace.path(ManagedUserArea::Recovery).join("pending-prompt-tasks.json");
        let mut document:Value=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        document["prompt_tasks"][0]=serde_json::to_value(&record).unwrap();
        fs::write(path,serde_json::to_vec(&document).unwrap()).unwrap();record
    }
    #[test]
    fn core_prompt_old_epoch_partial_upload_rebinds_only_original_verified_unsent_record() {
        let transport=Transport::new(0);let f=fixture(&transport.url);
        let source=f.persistence.lease().namespace.path(ManagedUserArea::ReferencesLibrary).join("original-reference.png");fs::write(&source,b"original-reference-bytes").unwrap();
        let mut original=row(&f,"composer");original.server_task_id.clear();original.result_prompt.clear();
        original.reference_paths=vec![source.display().to_string()];original.reference_sha256=vec![format!("{:x}",Sha256::digest(b"original-reference-bytes"))];original.reference_size_bytes=vec![24];
        let saved=seed_old_prompt_fixture(&f,original);let expected=saved.clone();let capture=PromptCapture::new(&f.context).unwrap();
        select(&f.context,&f.scope.request.session,OTHER,false);
        let job=spawn_prompt_job(&f.context,&capture,move|worker|revalidate_prompt_epoch(worker,expected)).unwrap();
        let rebound=job.receiver.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();join_prompt_workers().unwrap();
        let mut expected=saved;expected.auth_epoch=f.scope.request.session.auth_epoch;
        assert_eq!(serde_json::to_value(rebound).unwrap(),serde_json::to_value(&expected).unwrap());
        assert_eq!(serde_json::to_value(&rows(&f)[0]).unwrap(),serde_json::to_value(expected).unwrap());assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_prompt_old_partial_accepted_terminal_or_missing_fingerprint_never_uses_unsent_rebind() {
        for mode in ["accepted","terminal","fingerprint","replaced"] {
            let mut transport=Transport::new(if mode=="accepted"{1}else{0});let f=fixture(&transport.url);
            let source=f.persistence.lease().namespace.path(ManagedUserArea::ReferencesLibrary).join("original-reference.png");fs::write(&source,b"original-reference-bytes").unwrap();
            let mut original=row(&f,"composer");original.server_task_id.clear();original.result_prompt.clear();
            original.reference_paths=vec![source.display().to_string()];original.reference_sha256=vec![format!("{:x}",Sha256::digest(b"original-reference-bytes"))];original.reference_size_bytes=vec![24];
            if mode=="accepted"{original.server_task_id=TASK.into();}else if mode=="terminal"{original.result_prompt="paid result".into();}
            let mut saved=seed_old_prompt_fixture(&f,original);
            if mode=="fingerprint"{
                saved.reference_sha256.clear();let path=f.persistence.lease().namespace.path(ManagedUserArea::Recovery).join("pending-prompt-tasks.json");
                let mut doc:Value=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();doc["prompt_tasks"][0]=serde_json::to_value(&saved).unwrap();fs::write(path,serde_json::to_vec(&doc).unwrap()).unwrap();
            }
            if mode=="replaced"{fs::write(&source,b"changed-reference-content").unwrap();}
            let expected=saved.clone();let capture=PromptCapture::new(&f.context).unwrap();
            let job=spawn_prompt_job(&f.context,&capture,move|worker|revalidate_prompt_epoch(worker,expected)).unwrap();
            if mode=="accepted"{transport.wait();transport.reply(0,response(403,Value::Null,"group_frozen"));}
            assert!(job.receiver.recv_timeout(Duration::from_secs(3)).unwrap().is_err());join_prompt_workers().unwrap();
            let path=f.persistence.lease().namespace.path(ManagedUserArea::Recovery).join("pending-prompt-tasks.json");
            let doc:Value=serde_json::from_slice(&fs::read(path).unwrap()).unwrap();assert_eq!(doc["prompt_tasks"][0],serde_json::to_value(saved).unwrap());
            let requests=transport.finish();assert_eq!(requests.len(),usize::from(mode=="accepted"));
            if mode=="accepted"{assert!(requests[0].starts_with(&format!("GET /v1/generation/tasks/{TASK} ")));assert!(!requests[0].to_ascii_lowercase().contains("x-account-group-id:"));}
        }
    }

    #[test]
    fn core_prompt_discard_write_failure_after_successful_read_preserves_paid_modal() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        seed(&f,row(&f,"composer"));present_next_recovered_prompt_result(&app,&f.context);pump(||app.global::<AppState>().get_recovered_prompt_result_open());
        let(tx,rx)=mpsc::channel();let(release_tx,release_rx)=mpsc::channel();
        PROMPT_BEFORE_REMOVE.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{tx.send(()).unwrap();release_rx.recv_timeout(Duration::from_secs(3)).unwrap();})));
        let release=ReleasePromptWorker(Some(release_tx));app.global::<AppState>().invoke_dismiss_recovered_prompt_result();
        let reached=std::cell::Cell::new(false);pump(||{if rx.try_recv().is_ok(){reached.set(true);}reached.get()});
        let path=f.persistence.lease().namespace.path(ManagedUserArea::Recovery).join("pending-prompt-tasks.json");
        fs::write(&path,b"changed-before-removal").unwrap();release.release();join_prompt_workers().unwrap();pump_for(Duration::from_millis(40));
        assert!(app.global::<AppState>().get_recovered_prompt_result_open());assert_eq!(app.global::<AppState>().get_recovered_prompt_result(),"paid result");
        assert_eq!(fs::read(path).unwrap(),b"changed-before-removal");assert!(transport.finish().is_empty());
    }
    #[test]
    fn core_prompt_replaced_reference_prevents_automatic_paid_result_application() {
        i_slint_backend_testing::init_no_event_loop();
        let transport=Transport::new(0);let f=fixture(&transport.url);let app=app(&f);
        let source=f.root.path().join("reference.png");fs::write(&source,b"original-reference-bytes").unwrap();
        f.context.store.borrow_mut().references.character.push(ReferenceData{id:OTHER.into(),source_path:source.display().to_string()});
        let mut record=row(&f,"composer");record.task_type="image_style_analysis".into();
        record.reference_paths=vec![source.display().to_string()];record.reference_sha256=vec![format!("{:x}",Sha256::digest(b"original-reference-bytes"))];record.reference_size_bytes=vec![24];
        seed(&f,record.clone());fs::write(&source,b"changed-reference-content").unwrap();
        let _=apply_prompt_result_if_target_matches(&app,&f.context,&record);
        pump(||app.global::<AppState>().get_recovered_prompt_result_open());join_prompt_workers().unwrap();
        assert_eq!(app.global::<AppState>().get_prompt(),"original input");assert_eq!(app.global::<AppState>().get_recovered_prompt_result(),"paid result");
        assert!(!rows(&f)[0].result_committed);assert!(transport.finish().is_empty());
    }
}
