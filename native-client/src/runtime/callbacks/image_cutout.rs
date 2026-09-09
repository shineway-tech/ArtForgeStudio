use super::*;
use std::sync::atomic::{AtomicBool,AtomicI32};

const CUTOUT_MIN_EDGE:u32=33;
const CUTOUT_POLL_RETRY_LIMIT:usize=4;
const CUTOUT_MAX_OWNED_BYTES:u64=100*1024*1024;
fn cutout_sha256_hex(bytes:&[u8])->String{
    use sha2::{Digest,Sha256};
    format!("{:x}",Sha256::digest(bytes))
}
#[derive(Clone,Copy,Debug)]
enum CutoutSourceError{Unsupported,TooSmall(u32)}
impl std::fmt::Display for CutoutSourceError{
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"cutout source rejected")}
}
impl std::error::Error for CutoutSourceError{}
#[derive(Debug)]
struct CutoutNoOutput;
impl std::fmt::Display for CutoutNoOutput{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"cutout ended without output")}}
impl std::error::Error for CutoutNoOutput{}
#[cfg(test)]
thread_local!{
    static CUTOUT_TEST_AFTER_SEND:RefCell<Option<Box<dyn FnOnce()+Send>>>=const{RefCell::new(None)};
    static CUTOUT_TEST_AFTER_SOURCE:RefCell<Option<Box<dyn FnOnce(&NamespaceStorageAuthority)+Send>>>=const{RefCell::new(None)};
}
#[derive(Clone)]
struct CutoutView{id:String,kind:String,path:String,title:String}
#[derive(Default)]
struct CutoutUi{request:Option<Uuid>,busy:Vec<(NamespaceLease,Uuid,String)>}
thread_local!{
    static CUTOUT_UI:Rc<RefCell<CutoutUi>>=Rc::new(RefCell::new(CutoutUi::default()));
    static CUTOUT_WORKERS:RefCell<Vec<CutoutWorker>>=const{RefCell::new(Vec::new())};
    static CUTOUT_CLOSED:Cell<bool>=const{Cell::new(false)};
    static CUTOUT_FAILED:Cell<bool>=const{Cell::new(false)};
}
#[derive(Clone)]
struct CutoutCapture{
    context:AppContext,persistence:PrivatePersistence,session:SessionScope,ui:Rc<RefCell<CutoutUi>>,request:Uuid,
    viewer_id:String,viewer_kind:String,viewer_path:String,view:Option<CutoutView>,view_ticket:Option<Rc<CapturedViewerSource>>,
}
impl CutoutCapture{
    fn binding_matches(&self)->bool{
        self.context.store.borrow().private_persistence.as_ref().is_some_and(|bound|bound.same_binding_metadata(&self.persistence))
            && self.context.active_namespace.lock().ok().is_some_and(|active|active.as_ref()==Some(self.persistence.lease()))
    }
    fn current(&self)->bool{
        !CUTOUT_CLOSED.with(Cell::get) && self.binding_matches() && self.persistence.is_current()
            && self.context.backend.as_ref().is_some_and(|backend|backend.api.session().is_scope_current(&self.session))
    }
    fn presentation_matches(&self,app:&AppWindow)->bool{
        let state=app.global::<AppState>();
        self.ui.borrow().request==Some(self.request)
            && state.get_viewer_id()==self.viewer_id && state.get_viewer_source()==self.viewer_kind
            && state.get_viewer_source_path()==self.viewer_path
            && self.view_ticket.as_ref().map_or(true,|ticket|ticket.is_current(app,&self.context,&self.persistence))
    }
    fn apply<R>(&self,app:&AppWindow,apply:impl FnOnce()->R)->Option<R>{
        if !self.current(){return None;}
        self.context.apply_user_completion(self.persistence.lease(),||{
            if !self.binding_matches() || !self.presentation_matches(app){return None;}Some(apply())
        }).ok().flatten()
    }
    fn capture(app:&AppWindow,context:AppContext,persistence:PrivatePersistence)->Option<Self>{
        let session=context.current_account_session_scope()?;let state=app.global::<AppState>();
        let viewer_id=state.get_viewer_id().to_string();let viewer_kind=state.get_viewer_source().to_string();
        let viewer_path=state.get_viewer_source_path().to_string();
        let view_ticket=capture_current_viewer_source(app,&context).ok().map(Rc::new);
        let view=view_ticket.as_ref().filter(|ticket|ticket.is_current(app,&context,&persistence)).map(|ticket|CutoutView{
            id:ticket.source_id().to_owned(),kind:viewer_kind.clone(),path:ticket.path().to_string_lossy().into_owned(),
            title:state.get_viewer_title().to_string(),
        });
        let capture=Self{context,persistence,session,ui:CUTOUT_UI.with(Clone::clone),request:Uuid::new_v4(),viewer_id,viewer_kind,viewer_path,view,view_ticket};
        if !capture.current(){return None;}
        capture.context.apply_user_completion(capture.persistence.lease(),||{
            if !capture.binding_matches(){return false;}capture.ui.borrow_mut().request=Some(capture.request);true
        }).ok().filter(|value|*value)?;Some(capture)
    }
}
struct CutoutBusy{ui:Rc<RefCell<CutoutUi>>,lease:NamespaceLease,id:Uuid}
impl Drop for CutoutBusy{
    fn drop(&mut self){self.ui.borrow_mut().busy.retain(|(lease,id,_)|lease!=&self.lease || *id!=self.id);}
}
fn cutout_busy_for(persistence:&PrivatePersistence,key:Option<&str>)->bool{
    CUTOUT_UI.with(|ui|ui.borrow().busy.iter().any(|(lease,_,active_key)|lease==persistence.lease()
        && (active_key.is_empty() || key.map_or(true,|key|key==active_key))))
}
fn reserve_cutout(capture:&CutoutCapture,key:Option<&str>)->Option<CutoutBusy>{
    if cutout_busy_for(&capture.persistence,key){return None;}
    let id=Uuid::new_v4();let lease=capture.persistence.lease().clone();
    capture.ui.borrow_mut().busy.push((lease.clone(),id,key.unwrap_or("").into()));
    Some(CutoutBusy{ui:capture.ui.clone(),lease,id})
}
struct CutoutWorker{id:Uuid,lease:NamespaceLease,cancel:Arc<AtomicBool>,handle:std::thread::JoinHandle<()>}
fn cutout_worker_failed(){
    CUTOUT_FAILED.with(|failed|failed.set(true));CUTOUT_CLOSED.with(|closed|closed.set(true));
    cancel_cutout_workers_for_upgrade();
}
fn reap_cutout_workers(){
    let ready=CUTOUT_WORKERS.with(|workers|{
        let mut workers=workers.borrow_mut();let mut ready=Vec::new();let mut index=0;
        while index<workers.len(){if workers[index].handle.is_finished(){ready.push(workers.remove(index));}else{index+=1;}}ready
    });
    for worker in ready{if worker.handle.join().is_err(){cutout_worker_failed();}}
}
fn cutout_worker_pending(id:Uuid)->bool{
    CUTOUT_WORKERS.with(|workers|workers.borrow().iter().any(|worker|worker.id==id))
}
pub(super) fn cancel_cutout_workers_for_retirement(lease:&NamespaceLease){
    CUTOUT_WORKERS.with(|workers|for worker in workers.borrow().iter().filter(|worker|&worker.lease==lease){worker.cancel.store(true,Ordering::Release);});
}
pub(super) fn cancel_cutout_workers_for_upgrade(){
    CUTOUT_WORKERS.with(|workers|for worker in workers.borrow().iter(){worker.cancel.store(true,Ordering::Release);});
}
fn join_cutout_workers()->Result<()>{
    let mut workers=CUTOUT_WORKERS.with(|workers|std::mem::take(&mut *workers.borrow_mut())).into_iter();
    while let Some(worker)=workers.next(){if worker.handle.join().is_err(){
        cutout_worker_failed();for pending in workers.as_slice(){pending.cancel.store(true,Ordering::Release);}
    }}
    anyhow::ensure!(!CUTOUT_FAILED.with(Cell::get),"cutout worker panicked");Ok(())
}
/// Owner UI thread, after event-loop exit, outside ordinary/short guards.
pub(super) fn shutdown_cutout_workers()->Result<()>{
    CUTOUT_CLOSED.with(|closed|closed.set(true));cancel_cutout_workers_for_upgrade();join_cutout_workers()
}
#[cfg(test)]
fn drain_cutout_test_workers(){
    let result=join_cutout_workers();if !std::thread::panicking(){result.unwrap();}
}
struct CutoutJob<R>{id:Uuid,cancel:Arc<AtomicBool>,progress:Arc<AtomicI32>,receiver:mpsc::Receiver<Result<R>>,show_progress:bool}
impl<R> Drop for CutoutJob<R>{fn drop(&mut self){self.cancel.store(true,Ordering::Release);}}
fn cutout_orphan_poll(id:Uuid){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        reap_cutout_workers();if cutout_worker_pending(id){cutout_orphan_poll(id);}
    });
}
fn spawn_cutout_work<R:Send+'static>(
    app:&AppWindow,capture:CutoutCapture,show_progress:bool,
    work:impl FnOnce(&PrivatePersistence,&Arc<AtomicBool>,&Arc<AtomicI32>)->Result<R>+Send+'static,
    complete:impl FnOnce(&AppWindow,&CutoutCapture,Result<R>)+'static,
)->bool{
    if !capture.current(){return false;}
    let Ok(activity)=capture.persistence.begin_activity()else{return false;};
    let persistence=capture.persistence.clone();let cancel=Arc::new(AtomicBool::new(false));
    let worker_cancel=cancel.clone();let progress=Arc::new(AtomicI32::new(1));let worker_progress=progress.clone();
    let id=Uuid::new_v4();let(sender,receiver)=mpsc::channel();
    #[cfg(test)]
    let after_send=CUTOUT_TEST_AFTER_SEND.with(|hook|hook.borrow_mut().take());
    let spawned=std::thread::Builder::new().name("cutout-owned-work".into()).spawn(move||{
        let result=if worker_cancel.load(Ordering::Acquire) || activity.is_quiescing() || !persistence.is_current(){
            Err(anyhow!("cutout work retired"))
        }else{work(&persistence,&worker_cancel,&worker_progress)};
        drop(activity);let _=sender.send(result);
        #[cfg(test)]
        if let Some(after_send)=after_send{after_send();}
    });
    let handle=match spawned{Ok(handle)=>handle,Err(_)=>{
        cutout_error(app,&capture,anyhow!("cutout worker unavailable"));return false;
    }};
    CUTOUT_WORKERS.with(|workers|workers.borrow_mut().push(CutoutWorker{id,lease:capture.persistence.lease().clone(),cancel:cancel.clone(),handle}));
    poll_cutout_work(app.as_weak(),capture,Rc::new(RefCell::new(CutoutJob{id,cancel,progress,receiver,show_progress})),complete);true
}
fn poll_cutout_work<R:Send+'static>(
    weak:Weak<AppWindow>,capture:CutoutCapture,job:Rc<RefCell<CutoutJob<R>>>,
    complete:impl FnOnce(&AppWindow,&CutoutCapture,Result<R>)+'static,
){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        reap_cutout_workers();let id=job.borrow().id;
        let Some(app)=weak.upgrade()else{
            job.borrow().cancel.store(true,Ordering::Release);if cutout_worker_pending(id){cutout_orphan_poll(id);}return;
        };
        if !capture.current(){job.borrow().cancel.store(true,Ordering::Release);}
        if cutout_worker_pending(id){
            if job.borrow().show_progress{
                let progress=job.borrow().progress.load(Ordering::Acquire).clamp(1,99);
                capture.apply(&app,||app.global::<AppState>().set_cutout_progress(progress));
            }
            poll_cutout_work(weak,capture,job,complete);return;
        }
        let result=match job.borrow().receiver.try_recv(){
            Ok(result)=>result,Err(TryRecvError::Disconnected)=>Err(anyhow!("cutout worker disconnected")),
            Err(TryRecvError::Empty)=>{poll_cutout_work(weak,capture,job.clone(),complete);return;}
        };
        // Terminal-session dispatch must happen after the worker/activity has exited
        // even if session invalidation made ordinary current() false.
        if result.as_ref().err().is_some_and(|error|error.downcast_ref::<ApiError>().is_some_and(|error|error.is_terminal_session_error())){
            cutout_error(&app,&capture,result.err().unwrap());return;
        }
        if capture.current(){complete(&app,&capture,result);}
    });
}
fn cutout_error(app:&AppWindow,capture:&CutoutCapture,error:anyhow::Error){
    if let Some(api)=error.downcast_ref::<ApiError>(){
        if api.is_terminal_session_error(){
            if capture.binding_matches() && terminal_auth_scope_matches_context(&capture.context,&capture.session){
                drop(error);sign_out_locally(app,&capture.context,true,Some(capture.session.auth_epoch));
            }
            return;
        }
    }
    capture.apply(app,||{
        let state=app.global::<AppState>();state.set_cutout_processing(false);state.set_cutout_progress(0);
        if let Some(source)=error.downcast_ref::<CutoutSourceError>(){set_cutout_source_error(app,*source);return;}
        let english=state.get_language().as_str()=="en";
        if error.is::<CutoutNoOutput>(){
            state.set_cutout_message(if english{"The task ended without a cutout image. You can submit a new task."}else{"原任务已结束且没有抠图结果，可重新提交任务"}.into());return;
        }
        state.set_cutout_message(if english{"The operation was not confirmed. Retained work is preserved; retry to recover it."}else{"操作未确认，原任务和结果已保留，请重试恢复"}.into());
        if let Some(api_error)=error.downcast_ref::<ApiError>() {
            if let Some(message)=show_credit_rejection(&state,api_error) {
                state.set_cutout_message(message.into());
            }
        }
    });
}

pub(super) fn wire_image_cutout_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_submit_cutout(move |subject_type| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_image_cutout(&app, context.clone(), subject_type.as_str());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_reveal_cutout_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_cutout_result_path().to_string());
            if !path.is_file() {
                state.set_cutout_message(
                    if state.get_language().as_str() == "en" {
                        "No cutout image is available yet"
                    } else {
                        "暂无可保存的抠图结果"
                    }
                    .into(),
                );
                return;
            }
            let default_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("抠图结果.png");
            let Some(destination) = rfd::FileDialog::new()
                .add_filter("PNG", &["png"])
                .set_file_name(default_name)
                .save_file()
            else {
                return;
            };
            let destination = normalize_cutout_destination(destination);
            let result = if destination == path {
                Ok(())
            } else {
                fs::read(&path).and_then(|bytes| {
                    atomic_write_file(&destination, &bytes)
                        .map_err(|error| std::io::Error::other(error.to_string()))
                })
            };
            match result {
                Ok(()) => state.set_cutout_message(
                    if state.get_language().as_str() == "en" {
                        "Saved the PNG image"
                    } else {
                        "抠图结果已保存到本地"
                    }
                    .into(),
                ),
                Err(error) => state.set_cutout_message(
                    if state.get_language().as_str() == "en" {
                        format!("Failed to save the PNG image: {error}")
                    } else {
                        format!("保存抠图结果失败：{error}")
                    }
                    .into(),
                ),
            }
        });
    }
}

fn normalize_cutout_destination(mut path: PathBuf) -> PathBuf {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("png"))
    {
        path.set_extension("png");
    }
    path
}

fn normalized_cutout_type(value: &str) -> Option<&'static str> {
    match value {
        "general" => Some("general"),
        "portrait" => Some("portrait"),
        "avatar" => Some("avatar"),
        "skin" => Some("skin"),
        "product" => Some("product"),
        "clothing" => Some("clothing"),
        "sky" => Some("sky"),
        _ => None,
    }
}

fn cutout_type_label(value: &str) -> &'static str {
    match value {
        "portrait" => "人像",
        "avatar" => "头像",
        "skin" => "皮肤",
        "product" => "商品",
        "clothing" => "服饰",
        "sky" => "天空",
        _ => "通用",
    }
}

fn minimum_cutout_edge(subject_type: &str) -> u32 {
    if matches!(subject_type, "clothing" | "sky") {
        51
    } else {
        CUTOUT_MIN_EDGE
    }
}

fn validate_cutout_dimensions(
    width: u32,
    height: u32,
    subject_type: &str,
) -> std::result::Result<(), CutoutSourceError> {
    let min_edge = minimum_cutout_edge(subject_type);
    if width < min_edge || height < min_edge {
        return Err(CutoutSourceError::TooSmall(min_edge));
    }
    Ok(())
}

fn set_cutout_source_error(app: &AppWindow, error: CutoutSourceError) {
    let state = app.global::<AppState>();
    let english = state.get_language().as_str() == "en";
    let message = match error {
        CutoutSourceError::Unsupported => {
            if english {
                "Cutout supports JPG, PNG and WebP images"
            } else {
                "抠图仅支持 JPG、PNG 和 WebP 图片"
            }
        }
        CutoutSourceError::TooSmall(min_edge) => {
            if english {
                return state.set_cutout_message(
                    format!("Both image edges must be at least {min_edge} pixels").into(),
                );
            } else {
                return state
                    .set_cutout_message(format!("图片宽高均不能小于 {min_edge} 像素").into());
            }
        }
    };
    state.set_cutout_message(message.into());
}



fn decode_cutout_source_bytes(bytes:&[u8],subject:&str)->Result<image::DynamicImage>{
    let format=image::guess_format(bytes).map_err(|_|CutoutSourceError::Unsupported)?;
    anyhow::ensure!(matches!(format,image::ImageFormat::Jpeg|image::ImageFormat::Png|image::ImageFormat::WebP),CutoutSourceError::Unsupported);
    let image=decode_reference_bytes(bytes).map_err(|_|CutoutSourceError::Unsupported)?;
    validate_cutout_dimensions(image.width(),image.height(),subject).map_err(anyhow::Error::from)?;
    Ok(image)
}
fn start_image_cutout(app:&AppWindow,context:AppContext,subject:&str){
    let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
    if cutout_busy_for(&persistence,None){return;}
    let Some(capture)=CutoutCapture::capture(app,context.clone(),persistence)else{return;};
    if normalized_cutout_type(subject).is_none(){
        capture.apply(app,||app.global::<AppState>().set_cutout_message(if app.global::<AppState>().get_language().as_str()=="en"{"Choose a valid subject type"}else{"请选择有效的抠图类型"}.into()));return;
    }
    if app.global::<AppState>().get_session_state().as_str()!="online"{
        capture.apply(app,||{let state=app.global::<AppState>();state.set_auth_open(true);state.set_cutout_message(
            if state.get_language().as_str()=="en"{"Sign in and connect to the service before starting cutout"}else{"请先登录并连接服务后再开始抠图"}.into());});return;
    }
    let(scope,authority,activity)=match context.capture_billing_action(KnownCapability::Bill){
        Ok(value)=>value,Err(error)=>{cutout_error(app,&capture,error.into());return;}
    };
    drop(activity);start_image_cutout_with_billing_scope(app,context,authority,&scope,subject);
}
pub(super) fn start_image_cutout_with_billing_scope(
    app:&AppWindow,context:AppContext,authority:Arc<NamespaceStorageAuthority>,scope:&BillingScope,subject:&str,
){
    let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
    if cutout_busy_for(&persistence,None){return;}
    let Some(capture)=CutoutCapture::capture(app,context.clone(),persistence)else{return;};
    if capture.persistence.lease()!=authority.lease(){return;}
    let Some(subject)=normalized_cutout_type(subject)else{return;};
    let billing=match capture_billing_scope_for_submission(context.backend.as_deref(),&authority,scope){
        Ok(value)=>value,Err(error)=>{cutout_error(app,&capture,error.into());return;}
    };
    let Some(view)=capture.view.clone()else{cutout_error(app,&capture,CutoutSourceError::Unsupported.into());return;};
    if view.path!=capture.viewer_path || !capture.persistence.owns_path(Path::new(&view.path)){
        cutout_error(app,&capture,CutoutSourceError::Unsupported.into());return;
    }
    let Some(busy)=reserve_cutout(&capture,None)else{return;};
    let subject=subject.to_owned();let key=Uuid::new_v4().to_string();let original_new_key=key.clone();let local_task=Uuid::new_v4().to_string();
    let backend=context.backend.clone().unwrap();
    capture.apply(app,||{
        let state=app.global::<AppState>();state.set_cutout_processing(true);state.set_cutout_progress(1);
        state.set_cutout_type(subject.clone().into());state.set_cutout_estimated_credits("20".into());
        state.set_cutout_message(if state.get_language().as_str()=="en"{"Preparing the original cutout task..."}else{"正在准备原始抠图任务..."}.into());
    });
    #[cfg(test)]
    let after_source=CUTOUT_TEST_AFTER_SOURCE.with(|hook|hook.borrow_mut().take());
    spawn_cutout_work(app,capture,true,move|persistence,cancel,progress|{
        let authority=persistence.storage_authority()?;
        let rows=load_pending_generations_for_namespace(&authority)?.into_iter().filter(|row|row.task_type=="image_cutout").collect::<Vec<_>>();
        anyhow::ensure!(rows.len()<=1,"multiple cutout records require explicit recovery");
        if let Some(record)=rows.into_iter().next(){
            return run_cutout_record(&backend,&authority,None,&billing.request.session,record,cancel,progress);
        }
        anyhow::ensure!(persistence.is_current() && !cancel.load(Ordering::Acquire),"cutout retired");
        // Only the original Store-owned path is admitted. No cached-pixel/raw legacy fallback.
        let bytes=authority.read_image_source(Path::new(&view.path),CUTOUT_MAX_OWNED_BYTES)?;
        let image=decode_cutout_source_bytes(&bytes,&subject)?;
        let source=persist_reference_image_for_namespace(&authority,&image)?;
        let bytes=authority.read_image_source(&source,CUTOUT_MAX_OWNED_BYTES)?;
        let _=decode_cutout_source_bytes(&bytes,&subject)?;
        let record=new_cutout_record(&billing,key,local_task,&view,&source,&subject,&bytes)?;
        upsert_pending_generation_for_namespace(&authority,&billing,record.clone())?;
        #[cfg(test)]
        if let Some(after_source)=after_source{after_source(&authority);}
        run_cutout_record(&backend,&authority,Some(&billing),&billing.request.session,record,cancel,progress)
    },move|app,capture,result|finish_cutout_work(app,capture,result,busy,Some(original_new_key)));
}
fn new_cutout_record(scope:&BillingScope,key:String,local_task:String,view:&CutoutView,source:&Path,subject:&str,bytes:&[u8])->Result<PendingGenerationRecord>{
    let path=source.to_str().ok_or_else(||anyhow!("invalid owned cutout path"))?.to_owned();
    Ok(PendingGenerationRecord{
        source_asset_id:if view.kind!="reference"{view.id.clone()}else{String::new()},video_request:None,
        schema_version:2,cancel_requested:false,created_at_epoch_ms:Local::now().timestamp_millis(),client_request_id:key,
        owner_user_id:scope.request.session.owner_user_id.clone(),billing_account_group_id:scope.request.account_group_id.clone(),auth_epoch:scope.request.session.auth_epoch,
        local_task_id:local_task,server_task_id:String::new(),raw_prompt:if view.title.trim().is_empty(){"图片".into()}else{view.title.clone()},
        generation_prompt:"智能抠图".into(),task_type:"image_cutout".into(),category:"other".into(),mode:"game".into(),ratio:String::new(),
        quality:subject.into(),model_code:"aliyun_image_segmentation".into(),conversation_id:String::new(),count:1,target_width:0,target_height:0,create_conversation:false,
        reference_paths:vec![path.clone()],reference_sha256:vec![cutout_sha256_hex(bytes)],reference_size_bytes:vec![bytes.len() as u64],
        lineage_reference_paths:vec![path],uploaded_file_ids:vec![],deliveries:vec![],terminal:false,expected_success_count:0,
        canvas_source_node_id:String::new(),canvas_ui_extraction:false,
    })
}
pub(super) fn resume_pending_image_cutout(app:&AppWindow,context:AppContext,record:PendingGenerationRecord){
    let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
    if cutout_busy_for(&persistence,Some(&record.client_request_id)){return;}
    let Some(capture)=CutoutCapture::capture(app,context.clone(),persistence)else{return;};
    let Some(busy)=reserve_cutout(&capture,Some(&record.client_request_id))else{return;};
    let Some(backend)=context.backend.clone()else{return;};let session=capture.session.clone();
    capture.apply(app,||{
        let state=app.global::<AppState>();state.set_cutout_processing(true);state.set_cutout_progress(5);
        state.set_cutout_message(if state.get_language().as_str()=="en"{"Recovering the original cutout task..."}else{"正在恢复原抠图任务..."}.into());
    });
    spawn_cutout_work(app,capture,true,move|persistence,cancel,progress|{
        let authority=persistence.storage_authority()?;
        run_cutout_record(&backend,&authority,None,&session,record,cancel,progress)
    },move|app,capture,result|finish_cutout_work(app,capture,result,busy,None));
}
fn finish_cutout_work(app:&AppWindow,capture:&CutoutCapture,result:Result<PreparedNamespaceDelivery>,busy:CutoutBusy,original_new_key:Option<String>){
    let prepared=match result{Ok(value)=>value,Err(error)=>{drop(busy);cutout_error(app,capture,error);return;}};
    if prepared.lease()!=capture.persistence.lease() || !capture.current(){return;}
    let path=prepared.source_path().to_owned();let subject=prepared.record().quality.clone();
    let source_matches=original_new_key.as_ref()==Some(&prepared.record().client_request_id) || capture.view.as_ref().is_some_and(|view|
        (prepared.record().source_asset_id.is_empty() || prepared.record().source_asset_id==view.id)
        && prepared.record().reference_paths.first().is_some_and(|path|path==&view.path));
    let original=capture.clone();
    start_image_delivery_commit(app,capture.context.clone(),prepared,Local::now().format("%Y-%m-%d %H:%M").to_string(),move|app,result|{
        // The shared callback already owns the short original-lease completion.
        // Only pure checks and presentation; no guard acquisition/drop or I/O.
        if original.binding_matches() && original.presentation_matches(app){
            let state=app.global::<AppState>();let english=state.get_language().as_str()=="en";state.set_cutout_processing(false);
            match result{
                Ok((image,_,ack))=>{
                    if !source_matches{
                        state.set_cutout_message(if english{"The original cutout was saved to My Assets / Other; the current viewer was not changed"}else{"原抠图结果已保存到“我的资产 / 其他”，当前查看图片未改变"}.into());drop(busy);return;
                    }
                    state.set_cutout_type(subject.clone().into());state.set_cutout_result_path(path.clone().into());
                    state.set_cutout_result_name(Path::new(&path).file_name().and_then(|name|name.to_str()).unwrap_or("cutout.png").into());
                    state.set_cutout_result_image(image);state.set_cutout_progress(100);
                    state.set_cutout_message(match(ack,english){
                        (true,true)=>"Cutout saved to My Assets / Other",(true,false)=>"抠图完成，已保存到“我的资产 / 其他”",
                        (false,true)=>"Cutout saved locally; server acknowledgment is pending",(false,false)=>"抠图已本地保存，服务端确认待恢复",
                    }.into());
                },
                Err(_)=>state.set_cutout_message(if english{"Local save is unconfirmed; the original cutout is retained for retry"}else{"本地保存未确认，原抠图结果已保留，请重试恢复"}.into()),
            }
        }
        drop(busy);
    });
}

fn cutout_worker_current(backend:&BackendRuntime,authority:&NamespaceStorageAuthority,session:&SessionScope,cancel:&AtomicBool)->bool{
    !cancel.load(Ordering::Acquire) && authority.lease().auth_epoch==session.auth_epoch
        && authority.user_public_id()==session.owner_user_id && backend.api.user_work_is_current(session)
        && !backend.api.upgrade_latch().is_tripped()
}
fn cutout_wait(backend:&BackendRuntime,authority:&NamespaceStorageAuthority,session:&SessionScope,cancel:&AtomicBool,duration:Duration)->bool{
    let deadline=Instant::now()+duration;
    while Instant::now()<deadline{
        if !cutout_worker_current(backend,authority,session,cancel){return false;}
        std::thread::sleep(Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now())));
    }
    cutout_worker_current(backend,authority,session,cancel)
}
fn validate_cutout_detail(record:&PendingGenerationRecord,detail:&GenerationTaskDetail)->Result<()>{
    api::require_saved_group(&record.billing_account_group_id,&detail.billing_account_group_id)?;
    anyhow::ensure!(Uuid::parse_str(&detail.id).is_ok() && (record.server_task_id.is_empty() || record.server_task_id==detail.id),"cutout task identity mismatch");
    anyhow::ensure!(detail.requested_count==1,"cutout task count mismatch");Ok(())
}
fn run_cutout_record(
    backend:&BackendRuntime,authority:&Arc<NamespaceStorageAuthority>,billing:Option<&BillingScope>,session:&SessionScope,
    expected:PendingGenerationRecord,cancel:&Arc<AtomicBool>,progress:&Arc<AtomicI32>,
)->Result<PreparedNamespaceDelivery>{
    let _activity=backend.api.begin_user_work(session)?;
    anyhow::ensure!(cutout_worker_current(backend,authority,session,cancel),"cutout retired");
    let mut record=load_pending_generations_for_namespace(authority)?.into_iter().find(|record|record.identity()==expected.identity()).ok_or_else(||anyhow!("original cutout record changed"))?;
    anyhow::ensure!(record.task_type=="image_cutout" && record.owner_user_id==session.owner_user_id && record.count==1
        && normalized_cutout_type(&record.quality).is_some() && !record.cancel_requested
        && record.canvas_source_node_id.is_empty() && !record.canvas_ui_extraction,"cutout record unsupported");
    if let Some(billing)=billing{
        capture_billing_scope_for_submission(Some(backend),authority,billing)?;
        anyhow::ensure!(record.billing_account_group_id==billing.request.account_group_id && record.auth_epoch==billing.request.session.auth_epoch,"new cutout payer changed");
    }
    let api=GenerationApi::new(backend.api.clone()).with_saved_group(&record.billing_account_group_id);
    // Incomplete original input proves no POST was possible: uploaded id was always
    // acknowledged before every create. Cross-epoch rebind changes only auth_epoch.
    if record.auth_epoch!=session.auth_epoch && record.server_task_id.is_empty() && record.uploaded_file_ids.is_empty(){
        anyhow::ensure!(!record.terminal && record.deliveries.is_empty() && record.expected_success_count==0
            && record.reference_paths.len()==1 && record.reference_sha256.len()==1 && record.reference_size_bytes.len()==1
            && generation_references_match_for_namespace(authority,&record),"incomplete old cutout source is not verifiable");
        anyhow::ensure!(rebind_pending_generation_epoch_for_namespace(authority,&record.identity(),session.auth_epoch)?,"cutout rebind refused");
        record.auth_epoch=session.auth_epoch;
    }
    if record.server_task_id.is_empty() && record.uploaded_file_ids.is_empty(){
        anyhow::ensure!(record.reference_paths.len()==1 && record.reference_sha256.len()==1 && record.reference_size_bytes.len()==1,"original cutout input missing");
        anyhow::ensure!(cutout_worker_current(backend,authority,session,cancel),"cutout retired");
        let uploaded=api.upload_reference_for_namespace_checked(Path::new(&record.reference_paths[0]),authority,session,false,&record.reference_sha256[0],record.reference_size_bytes[0])?;
        let uploaded=vec![uploaded];
        anyhow::ensure!(apply_generation_patch_for_namespace(authority,&record.identity(),GenerationRecoveryPatch::UploadedFileIds(uploaded.clone()))?,"upload receipt not saved");
        record.uploaded_file_ids=uploaded;
    }
    anyhow::ensure!(cutout_worker_current(backend,authority,session,cancel),"cutout retired");
    let mut detail=if record.server_task_id.is_empty(){
        anyhow::ensure!(!record.terminal && record.uploaded_file_ids.len()==1,"original cutout create body missing");
        if let Some(billing)=billing{
            api.create_image_cutout_billing(&CreateImageCutout{
                client_request_id:record.client_request_id.clone(),reference_file_id:record.uploaded_file_ids[0].clone(),subject_type:record.quality.clone(),
            },billing)?
        }else{
            let replay=SavedReplayRequest::generation(authority.clone(),session,&record.client_request_id)?;
            backend.api.replay_saved::<GenerationTaskDetail>(&replay)?.data
        }
    }else{api.task_scoped(&record.server_task_id,session)?};
    validate_cutout_detail(&record,&detail)?;
    if record.auth_epoch!=session.auth_epoch{
        anyhow::ensure!(rebind_pending_generation_epoch_for_namespace(authority,&record.identity(),session.auth_epoch)?,"cutout rebind refused");
        record.auth_epoch=session.auth_epoch;
    }
    if record.server_task_id.is_empty(){
        anyhow::ensure!(apply_generation_patch_for_namespace(authority,&record.identity(),GenerationRecoveryPatch::Accepted{
            server_task_id:detail.id.clone(),uploaded_file_ids:record.uploaded_file_ids.clone(),clear_reference_inputs:false,
        })?,"cutout task binding not saved");
        record.server_task_id=detail.id.clone();
    }
    // Accepted cutout keeps source arrays: skin/sky still need exact original bytes.
    progress.store(8,Ordering::Release);
    loop{
        anyhow::ensure!(cutout_worker_current(backend,authority,session,cancel),"cutout retired");
        validate_cutout_detail(&record,&detail)?;progress.store(detail.progress_percent.clamp(8,99),Ordering::Release);
        if let Some(item)=detail.items.iter().find(|item|item.status=="succeeded"){
            return prepare_namespace_cutout_delivery(&api,authority.clone(),authority.delivery_index()?,&record.identity(),item.index).map_err(Into::into);
        }
        if detail.terminal(){
            // Only exact authoritative no-output terminal completion is releasable.
            // A malformed summary or any prior delivery keeps the recovery source.
            anyhow::ensure!(detail.success_count==0 && detail.items.iter().all(|item|item.status!="succeeded")
                && record.deliveries.is_empty(),"cutout output remains unresolved");
            anyhow::ensure!(apply_generation_patch_for_namespace(authority,&record.identity(),GenerationRecoveryPatch::Terminal{expected_success_count:0})?,"terminal cutout not retained");
            anyhow::ensure!(apply_generation_patch_for_namespace(authority,&record.identity(),GenerationRecoveryPatch::ReleaseReferenceInputs)?,"terminal inputs not released");
            let saved=load_pending_generations_for_namespace(authority)?.into_iter().find(|saved|saved.identity()==record.identity()).ok_or_else(||anyhow!("terminal cutout changed"))?;
            anyhow::ensure!(saved.server_task_id==detail.id && saved.count==1 && saved.terminal && saved.expected_success_count==0
                && saved.deliveries.is_empty() && saved.reference_paths.is_empty() && saved.reference_sha256.is_empty()
                && saved.reference_size_bytes.is_empty(),"terminal cutout cannot be released");
            anyhow::ensure!(remove_pending_generation_for_namespace(authority,&saved.identity())?,"terminal cutout release not acknowledged");
            return Err(CutoutNoOutput.into());
        }
        anyhow::ensure!(cutout_wait(backend,authority,session,cancel,Duration::from_millis(IMAGE_POLL_INTERVAL_MS)),"cutout retired");
        let mut retries=0;
        detail=loop{
            match api.task_scoped(&record.server_task_id,session){
                Ok(detail)=>break detail,
                Err(error)if error.should_preserve_generation_recovery() && retries<CUTOUT_POLL_RETRY_LIMIT=>{
                    retries+=1;anyhow::ensure!(cutout_wait(backend,authority,session,cancel,Duration::from_millis(IMAGE_POLL_INTERVAL_MS)),"cutout retired");
                },
                Err(error)=>return Err(error.into()),
            }
        };
    }
}


/// Pure byte codec for the owner-held remote proof/derived PNG bridge.
/// source_name is display context only; it is never opened or reinterpreted as authority.
pub(super) fn decode_cutout_result_bytes(
    _source_name:&Path,source_bytes:&[u8],subject_type:&str,remote_png:&[u8],
)->Result<(Vec<u8>,i32,i32)>{
    anyhow::ensure!(normalized_cutout_type(subject_type).is_some(),"invalid cutout subject");
    anyhow::ensure!(image::guess_format(remote_png)?==image::ImageFormat::Png,"server cutout is not PNG");
    let decoded=decode_reference_bytes(remote_png)?;
    let result=if decoded.color().has_alpha(){decoded.to_rgba8()}
        else if matches!(subject_type,"skin"|"sky"){apply_cutout_mask_bytes(source_bytes,&decoded)?}
        else{anyhow::bail!("server cutout lacks transparency");};
    let(width,height)=result.dimensions();
    anyhow::ensure!(width>0 && height>0 && width<=i32::MAX as u32 && height<=i32::MAX as u32,"cutout dimensions invalid");
    let bytes=if decoded.color().has_alpha(){remote_png.to_vec()}else{encode_png_rgba(&result,width,height)?};
    Ok((bytes,width as i32,height as i32))
}
fn apply_cutout_mask_bytes(source_bytes:&[u8],mask:&image::DynamicImage)->Result<image::RgbaImage>{
    let source=decode_reference_bytes(source_bytes)?.to_rgb8();let(width,height)=source.dimensions();
    anyhow::ensure!(width>0 && height>0 && mask.width()>0 && mask.height()>0,"source/mask dimensions invalid");
    let alpha=mask.resize_exact(width,height,image::imageops::FilterType::Lanczos3).to_luma8();
    let mut result=image::RgbaImage::new(width,height);
    for(x,y,pixel)in source.enumerate_pixels(){result.put_pixel(x,y,image::Rgba([pixel[0],pixel[1],pixel[2],alpha.get_pixel(x,y)[0]]));}
    Ok(result)
}
#[cfg(test)]
fn decode_cutout_result(source_path:&Path,subject_type:&str,bytes:&[u8])->Result<(Vec<u8>,i32,i32)>{
    let source=if image::guess_format(bytes)?==image::ImageFormat::Png && matches!(subject_type,"skin"|"sky")
        && !decode_reference_bytes(bytes)?.color().has_alpha(){std::fs::read(source_path)?}else{Vec::new()};
    decode_cutout_result_bytes(source_path,&source,subject_type,bytes)
}
#[cfg(test)]
fn run_image_cutout_worker(
    backend:Arc<BackendRuntime>,authority:Arc<NamespaceStorageAuthority>,billing:BillingScope,session:SessionScope,
    record:PendingGenerationRecord,sender:mpsc::Sender<Result<PreparedNamespaceDelivery>>,
){
    let result=run_cutout_record(&backend,&authority,Some(&billing),&session,record,&Arc::new(AtomicBool::new(false)),&Arc::new(AtomicI32::new(1)));
    let _=sender.send(result);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutout_subject_types_match_the_server_contract() {
        for value in [
            "general", "portrait", "avatar", "skin", "product", "clothing", "sky",
        ] {
            assert_eq!(normalized_cutout_type(value), Some(value));
        }
        assert_eq!(normalized_cutout_type("unknown"), None);
    }

    #[test]
    fn general_cutout_accepts_a_1536_by_2048_source() {
        assert!(validate_cutout_dimensions(1536, 2048, "general").is_ok());
        assert!(validate_cutout_dimensions(33, 33, "general").is_ok());
        assert!(validate_cutout_dimensions(50, 50, "clothing").is_err());
        assert!(validate_cutout_dimensions(51, 51, "clothing").is_ok());
        assert!(validate_cutout_dimensions(50, 50, "sky").is_err());
        assert!(validate_cutout_dimensions(51, 51, "sky").is_ok());
    }

    #[test]
    fn alpha_cutout_result_is_kept_as_a_png() {
        let rgba = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 0]));
        let bytes = encode_png_rgba(&rgba, 2, 2).expect("encode png");
        let (result, width, height) =
            decode_cutout_result(Path::new("unused"), "general", &bytes).expect("decode result");
        assert_eq!(result, bytes);
        assert_eq!((width, height), (2, 2));

        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&[1, 2, 3], 1, 1, image::ExtendedColorType::Rgb8)
            .expect("encode jpeg");
        assert!(decode_cutout_result(Path::new("unused"), "general", &jpeg).is_err());
    }

    #[test]
    fn skin_and_sky_masks_are_composed_with_the_local_source() {
        let source_path = std::env::temp_dir().join(format!(
            "artforge-cutout-mask-source-{}.png",
            Uuid::new_v4()
        ));
        let source =
            image::RgbImage::from_fn(2, 2, |x, y| image::Rgb([10 + x as u8, 20 + y as u8, 30]));
        source
            .save_with_format(&source_path, image::ImageFormat::Png)
            .expect("write source");

        let mask = image::GrayImage::from_raw(2, 2, vec![0, 64, 128, 255]).expect("mask");
        let mut mask_bytes = Vec::new();
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(Cursor::new(&mut mask_bytes)),
            mask.as_raw(),
            2,
            2,
            image::ExtendedColorType::L8,
        )
        .expect("encode mask");

        for subject_type in ["skin", "sky"] {
            let (result_bytes, width, height) =
                decode_cutout_result(&source_path, subject_type, &mask_bytes)
                    .expect("compose mask");
            let result =
                image::load_from_memory_with_format(&result_bytes, image::ImageFormat::Png)
                    .expect("decode composed result")
                    .to_rgba8();
            assert_eq!((width, height), (2, 2));
            assert_eq!(result.get_pixel(0, 0).0, [10, 20, 30, 0]);
            assert_eq!(result.get_pixel(1, 1).0, [11, 21, 30, 255]);
        }

        assert!(decode_cutout_result(&source_path, "general", &mask_bytes).is_err());
        let _ = fs::remove_file(source_path);
    }
}

#[cfg(test)]
mod billing_capture_tests {
    use super::*;
    #[test]
    fn billing_capture_cutout_worker_keeps_persisted_payer() {
        backend_generation::billing_capture_test_support::assert_generation_worker(
            "image_cutout",
            run_image_cutout_worker,
        );
    }
}

#[cfg(test)]
mod core_cutout_tests{
    use super::*;
    use std::io::{Read,Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool,Ordering};
    const OWNER:&str="11111111-1111-4111-8111-111111111111";
    const PAYER:&str="22222222-2222-4222-8222-222222222222";
    const TASK:&str="33333333-3333-4333-8333-333333333333";
    struct Fixture{inner:video_image_callbacks::tests::scoped_inputs::Fixture,expected_failure:bool}
    impl std::ops::Deref for Fixture{
        type Target=video_image_callbacks::tests::scoped_inputs::Fixture;
        fn deref(&self)->&Self::Target{&self.inner}
    }
    fn fixture(base_url:Option<&str>)->(Fixture,AppWindow){
        i_slint_backend_testing::init_no_event_loop();
        let mut inner=video_image_callbacks::tests::scoped_inputs::Fixture::new();
        if let Some(base_url)=base_url{
            let old=inner.context.backend.as_ref().unwrap();
            let backend=Arc::new(BackendRuntime{api:ApiClient::new(ApiClientConfig{
                base_url:reqwest::Url::parse(base_url).unwrap(),app_version:"999.0.0".into(),timeout:Duration::from_secs(2),
            },DeviceIdentity{id:Uuid::new_v4().to_string(),name:"cutout-fixture".into(),platform:"macos".into()},
                old.api.session().clone()).unwrap()});
            backend.api.bind_user_work(UserWorkAdmission::new(inner.context.active_namespace.clone(),inner.context.user_activity.clone())).unwrap();
            let persistence=PrivatePersistence::for_test_with_storage((*inner.writer).clone(),inner.persistence.lease().clone(),
                inner.context.user_activity.clone(),backend.api.upgrade_latch().clone(),
                inner.context.data_root_capability.clone().unwrap(),backend.api.clone(),inner.context.file_index.clone().unwrap());
            inner.context.backend=Some(backend);inner.context.store.borrow_mut().private_persistence=Some(persistence.clone());
            inner.authority=persistence.storage_authority().unwrap();inner.persistence=persistence;
        }
        let transition=inner.context.namespace_operations.try_begin_transition().unwrap();
        let phase=transition.begin_prepublication_recovery(inner.persistence.lease()).unwrap();
        phase.verify_no_unsupported_imports(&inner.authority).unwrap();
        let recovered=phase.finish().unwrap();transition.prepare_publication(inner.persistence.lease(),recovered).unwrap().publish();
        let app=AppWindow::new().unwrap();let state=app.global::<AppState>();
        state.set_page("assets".into());state.set_session_state("online".into());
        wire_image_cutout_callbacks(&app,inner.context.clone());
        inner.persistence.save_store(local_store_data(&app,&inner.context.store.borrow())).unwrap();
        (Fixture{inner,expected_failure:false},app)
    }
    impl Drop for Fixture{fn drop(&mut self){
        {
            let mut active=self.context.active_namespace.lock().unwrap_or_else(|error|error.into_inner());
            if active.as_ref()==Some(self.persistence.lease()){*active=None;}
        }
        cancel_cutout_workers_for_retirement(self.persistence.lease());
        let workers=join_cutout_workers();
        let delivery=drain_delivery_commit_workers_for_lease_for_test(self.persistence.lease());
        let retired=self.context.user_activity.begin_quiesce(self.persistence.lease());
        let retired=match retired{Ok(guard)=>{guard.retire();Ok(())},Err(error)=>Err(error)};
        if !std::thread::panicking(){assert_eq!(workers.is_err(),self.expected_failure);delivery.unwrap();retired.unwrap();}
    }}
    fn owned(f:&Fixture)->PathBuf{
        let bytes=png();let decoded=decode_reference_bytes(&bytes).unwrap();
        persist_reference_image_for_namespace(&f.authority,&decoded).unwrap()
    }
    fn png()->Vec<u8>{
        let image=image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(80,80,image::Rgba([12,34,56,255])));
        let mut bytes=std::io::Cursor::new(Vec::new());image.write_to(&mut bytes,image::ImageFormat::Png).unwrap();bytes.into_inner()
    }
    fn pump_for(duration:Duration){
        let end=Instant::now()+duration;while Instant::now()<end{
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn pump_until(mut ready:impl FnMut()->bool){
        let end=Instant::now()+Duration::from_secs(5);
        while !ready() && Instant::now()<end{pump_for(Duration::from_millis(5));}
        assert!(ready(),"cutout callback completion missing");
    }
    struct Http{
        url:String,seen:mpsc::Receiver<String>,release:Option<mpsc::Sender<()>>,
        stop:Arc<AtomicBool>,worker:Option<std::thread::JoinHandle<()>>,
    }
    impl Http{
        fn new(status:u16)->Self{
            let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
            let url=format!("http://{}/",listener.local_addr().unwrap());
            let(seen_tx,seen)=mpsc::channel();let(release,wait)=mpsc::channel();
            let stop=Arc::new(AtomicBool::new(false));let cancelled=stop.clone();
            let worker=std::thread::spawn(move||{
                let end=Instant::now()+Duration::from_secs(6);
                let mut stream=loop{
                    if cancelled.load(Ordering::Acquire){return;}
                    match listener.accept(){
                        Ok((stream,_))=>break stream,
                        Err(error)if error.kind()==std::io::ErrorKind::WouldBlock && Instant::now()<end=>std::thread::sleep(Duration::from_millis(2)),
                        Err(error)=>panic!("controlled cutout accept: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut request=Vec::new();let mut chunk=[0u8;1024];
                loop{
                    let read=stream.read(&mut chunk).unwrap();assert!(read>0);request.extend_from_slice(&chunk[..read]);assert!(request.len()<=1024*1024);
                    if let Some(end)=request.windows(4).position(|bytes|bytes==b"\r\n\r\n"){
                        let headers=String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                        let size=headers.lines().find_map(|line|line.strip_prefix("content-length:")).map(|size|size.trim().parse::<usize>().unwrap()).unwrap_or(0);
                        if request.len()>=end+4+size{break;}
                    }
                }
                let _=seen_tx.send(String::from_utf8(request).unwrap());
                let _=wait.recv_timeout(Duration::from_secs(3));
                let body=br#"{"request_id":"fixture-private","data":null,"error":{"code":"account_group_not_selectable","message":"private provider response must not display","details":null},"meta":null}"#;
                let _=write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                let _=stream.write_all(body);
            });
            Self{url,seen,release:Some(release),stop,worker:Some(worker)}
        }
        fn release(&mut self){if let Some(release)=self.release.take(){let _=release.send(());}}
        fn finish(&mut self){self.release();self.stop.store(true,Ordering::Release);if let Some(worker)=self.worker.take(){worker.join().unwrap();}}
    }
    impl Drop for Http{fn drop(&mut self){
        self.release();self.stop.store(true,Ordering::Release);
        if let Some(worker)=self.worker.take(){let joined=worker.join();if !std::thread::panicking(){joined.unwrap();}}
    }}
    fn publish_group(f:&Fixture,group:&str){
        let manager=&f.context.billing_context;let session=f.context.current_account_session_scope().unwrap();
        if manager.confirmed_scope().is_none(){manager.bind_authenticated_session(session.clone()).unwrap();}
        let ticket=manager.begin_switch(&session,"cutout-device",group,PreviousBillingAuthority::StillValid).unwrap();
        let snapshot:AccountSnapshot=serde_json::from_value(serde_json::json!({
            "user":{"id":OWNER,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
            "read_only":false,"capabilities":["bill"],"membership":null,"entitlement":{},"credits":null,"quota":null,
            "billing_group":{"group_id":group,"name":"fixture","group_status":"active","role":"owner","member_id":null,"relationship_status":null,
                "readable_context":true,"selectable":true,"group_version":"1","membership_version":null,"capabilities":["bill"],"quota":null}
        })).unwrap();
        let staged=manager.stage_confirmation(&ticket,snapshot.billing_group.clone(),snapshot).unwrap();
        f.writer.save_selected_group(OWNER,"cutout-device",group).unwrap();manager.publish_persisted(ticket,staged);
    }

    #[test]
    fn core_cutout_missing_binding_does_not_write_private_ui(){
        let(f,app)=fixture(None);f.context.store.borrow_mut().private_persistence=None;
        let state=app.global::<AppState>();state.set_cutout_message("original private state".into());state.set_cutout_progress(61);
        state.invoke_submit_cutout("general".into());
        assert_eq!(state.get_cutout_message(),"original private state");assert_eq!(state.get_cutout_progress(),61);
        assert!(f.context.store.borrow().assets.is_empty());
    }
    #[test]
    fn core_cutout_exact_upgrade_rejects_entry_before_private_error(){
        let(f,app)=fixture(None);publish_group(&f,PAYER);
        f.persistence.upgrade_latch().trip(RequiredUpgrade{minimum_version:Some("99.0.0".into())});
        let state=app.global::<AppState>();state.set_cutout_message("upgrade protected".into());state.set_cutout_progress(57);
        state.invoke_submit_cutout("unknown".into());
        assert_eq!(state.get_cutout_message(),"upgrade protected");assert_eq!(state.get_cutout_progress(),57);
        assert!(f.context.store.borrow().assets.is_empty());
    }
    fn retained(f:&Fixture)->PendingGenerationRecord{
        let scope=BillingScope{request:GroupRequestScope{session:f.context.current_account_session_scope().unwrap(),account_group_id:PAYER.into()},context_epoch:1};
        let mut record=backend_generation::billing_capture_test_support::generation_record(&scope,"image_cutout");
        record.server_task_id=TASK.into();record.quality="skin".into();
        upsert_pending_generation_for_namespace(&f.authority,&scope,record.clone()).unwrap();record
    }
    #[test]
    fn core_cutout_saved_payer_resume_fetches_header_free_and_keeps_subject(){
        let server=Http::new(403);let(f,app)=fixture(Some(&server.url));let mut server=server;let record=retained(&f);
        resume_pending_image_cutout(&app,f.context.clone(),record.clone());
        let request=server.seen.recv_timeout(Duration::from_millis(500));
        if request.is_err(){server.finish();}
        assert!(request.is_ok(),"saved cutout resume did not fetch original task");
        let request=request.unwrap();assert!(request.starts_with(&format!("GET /v1/generation/tasks/{TASK} ")));
        assert!(!request.to_ascii_lowercase().contains("x-account-group-id:"));
        server.finish();drain_cutout_test_workers();pump_for(Duration::from_millis(100));
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();
        assert_eq!(rows.len(),1);assert_eq!(rows[0].quality,"skin");assert_eq!(rows[0].billing_account_group_id,PAYER);
        assert_eq!(rows[0].client_request_id,record.client_request_id);
        assert_eq!(rows[0].reference_paths,record.reference_paths);assert_eq!(rows[0].reference_sha256,record.reference_sha256);
        assert!(!app.global::<AppState>().get_cutout_message().contains("private provider"));
    }
    struct HeldCutout{ready:mpsc::Receiver<()>,release:Option<mpsc::Sender<()>>}
    impl HeldCutout{
        fn install()->Self{
            let(sent,ready)=mpsc::channel();let(release,wait)=mpsc::channel();
            CUTOUT_TEST_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{
                let _=sent.send(());let _=wait.recv_timeout(Duration::from_secs(3));
            })));Self{ready,release:Some(release)}
        }
        fn release(&mut self){if let Some(release)=self.release.take(){let _=release.send(());}}
    }
    impl Drop for HeldCutout{fn drop(&mut self){self.release();}}
    #[test]
    fn core_cutout_received_failure_waits_for_real_worker_exit(){
        let server=Http::new(403);let(f,app)=fixture(Some(&server.url));let mut server=server;
        publish_group(&f,PAYER);let record=retained(&f);let mut held=HeldCutout::install();server.release();
        // Same saved resource, now through the production captured recovery entry.
        resume_pending_image_cutout(&app,f.context.clone(),record);
        held.ready.recv_timeout(Duration::from_secs(3)).unwrap();
        app.global::<AppState>().set_cutout_message("still owned by live worker".into());
        let advanced=Rc::new(Cell::new(false));let observed=advanced.clone();
        slint::Timer::single_shot(Duration::ZERO,move||observed.set(true));pump_for(Duration::from_millis(160));
        assert!(advanced.get());assert_eq!(app.global::<AppState>().get_cutout_message(),"still owned by live worker");
        held.release();drain_cutout_test_workers();pump_for(Duration::from_millis(100));server.finish();
    }
    const FILE:&str="44444444-4444-4444-8444-444444444444";
    const INPUT:&str="55555555-5555-4555-8555-555555555555";
    struct Scenario{
        url:String,listener:Option<TcpListener>,stop:Arc<AtomicBool>,worker:Option<std::thread::JoinHandle<()>>,
        requests:Arc<Mutex<Vec<(String,Vec<u8>)>>>,
    }
    impl Scenario{
        fn new()->Self{
            let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
            Self{url:format!("http://{}/",listener.local_addr().unwrap()),listener:Some(listener),
                stop:Arc::new(AtomicBool::new(false)),worker:None,requests:Arc::new(Mutex::new(Vec::new()))}
        }
        fn start(&mut self,mut reply:impl FnMut(&str,&[u8])->(u16,Vec<u8>)+Send+'static){
            let listener=self.listener.take().unwrap();let stop=self.stop.clone();let requests=self.requests.clone();
            self.worker=Some(std::thread::spawn(move||{
                let deadline=Instant::now()+Duration::from_secs(15);
                while !stop.load(Ordering::Acquire) && Instant::now()<deadline{
                    let mut stream=match listener.accept(){
                        Ok((stream,_))=>stream,Err(error)if error.kind()==std::io::ErrorKind::WouldBlock=>{std::thread::sleep(Duration::from_millis(2));continue;},
                        Err(error)=>panic!("cutout scenario accept: {error}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut request=Vec::new();let mut block=[0u8;2048];
                    let boundary=loop{
                        let read=match stream.read(&mut block){
                            Ok(0)if request.is_empty()=>break None,Ok(read)=>read,
                            Err(_)if request.is_empty()=>break None,Err(error)=>panic!("partial scenario request: {error}"),
                        };
                        assert!(read>0);request.extend_from_slice(&block[..read]);assert!(request.len()<=1024*1024);
                        if let Some(end)=request.windows(4).position(|window|window==b"\r\n\r\n"){
                            let headers=String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                            let size=headers.lines().find_map(|line|line.strip_prefix("content-length:")).map(|size|size.trim().parse::<usize>().unwrap()).unwrap_or(0);
                            if request.len()>=end+4+size{break Some((end,size));}
                        }
                    };
                    let Some((end,size))=boundary else{continue;};
                    let headers=String::from_utf8(request[..end].to_vec()).unwrap();let body=request[end+4..end+4+size].to_vec();
                    requests.lock().unwrap().push((headers.clone(),body.clone()));
                    let(status,body)=reply(&headers,&body);
                    let _=write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                    let _=stream.write_all(&body);
                }
            }));
        }
        fn finish(&mut self){
            self.stop.store(true,Ordering::Release);if let Some(worker)=self.worker.take(){worker.join().unwrap();}
        }
    }
    impl Drop for Scenario{fn drop(&mut self){
        self.stop.store(true,Ordering::Release);
        if let Some(worker)=self.worker.take(){let result=worker.join();if !std::thread::panicking(){result.unwrap();}}
    }}
    fn envelope(data:serde_json::Value)->Vec<u8>{
        serde_json::to_vec(&serde_json::json!({"request_id":"cutout-fixture","data":data,"error":null,"meta":null})).unwrap()
    }

    const SOURCE:&str="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    fn install_viewer_source(f:&Fixture,app:&AppWindow)->String{
        let path=owned(f).to_string_lossy().into_owned();
        f.context.store.borrow_mut().assets.push(AssetData{
            id:SOURCE.into(),conversation_id:String::new(),title:"Original portrait".into(),category:"other".into(),kind:"game".into(),
            time:"fixture".into(),prompt:String::new(),ratio:"1:1".into(),quality:"1K".into(),model:"fixture".into(),origin:"fixture".into(),
            width:80,height:80,source_path:path.clone(),reference_paths:vec![],cutout_done:false,remove_black_done:false,upscale_done:false,
            is_new:false,delivery_recoverable:false,delivery_downloading:false,
        });
        let state=app.global::<AppState>();state.set_viewer_id(SOURCE.into());state.set_viewer_source("asset".into());
        state.set_viewer_source_path(path.clone().into());state.set_viewer_title("Original portrait".into());
        state.set_viewer_open(false);state.set_cutout_open(true);
        f.persistence.save_store(local_store_data(app,&f.context.store.borrow())).unwrap();path
    }
    fn retained_with_source(f:&Fixture,source:&str)->PendingGenerationRecord{
        let scope=BillingScope{request:GroupRequestScope{session:f.context.current_account_session_scope().unwrap(),account_group_id:PAYER.into()},context_epoch:1};
        let bytes=f.authority.read_image_source(Path::new(source),100*1024*1024).unwrap();
        let mut row=backend_generation::billing_capture_test_support::generation_record(&scope,"image_cutout");
        row.server_task_id=TASK.into();row.quality="skin".into();row.source_asset_id=SOURCE.into();row.raw_prompt="Original portrait".into();
        row.reference_paths=vec![source.into()];row.lineage_reference_paths=vec![source.into()];
        row.reference_sha256=vec![cutout_sha256_hex(&bytes)];row.reference_size_bytes=vec![bytes.len() as u64];
        upsert_pending_generation_for_namespace(&f.authority,&scope,row.clone()).unwrap();row
    }
    fn mask_png()->Vec<u8>{
        let mask=image::GrayImage::from_fn(80,80,|x,_|image::Luma([if x%2==0{0}else{255}]));
        let mut bytes=Vec::new();image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(&mut bytes),mask.as_raw(),80,80,image::ExtendedColorType::L8,
        ).unwrap();bytes
    }
    fn cutout_task(url:&str,bytes:&[u8])->Vec<u8>{
        envelope(serde_json::json!({
            "id":TASK,"billing_account_group_id":PAYER,"status":"completed","progress_percent":100,"success_count":1,"failure_count":0,
            "failure":null,"prompt":null,"result_prompt":null,"request":{},"model":null,"quality":"skin","requested_count":1,"type":"image_cutout",
            "items":[{"index":0,"status":"succeeded","credit_cost":"20","failure":null,"file":{
                "id":FILE,"status":"available","mime_type":"image/png","size_bytes":bytes.len().to_string(),"sha256":cutout_sha256_hex(bytes),
                "width":80,"height":80,"download_url":format!("{url}mask.png")
            }}]
        }))
    }
    fn serve_cutout_delivery(server:&mut Scenario,f:&Fixture,acks:Arc<std::sync::atomic::AtomicUsize>){
        let url=server.url.clone();let mask=mask_png();let writer=(*f.writer).clone();let lease=f.persistence.lease().clone();let authority=f.authority.clone();
        server.start(move|headers,body|{
            let line=headers.lines().next().unwrap();let lower=headers.to_ascii_lowercase();
            if line.starts_with(&format!("GET /v1/generation/tasks/{TASK} ")){assert!(!lower.contains("x-account-group-id:"));return(200,cutout_task(&url,&mask));}
            if line.starts_with("GET /mask.png "){assert!(!lower.contains("x-token:"));return(200,mask.clone());}
            if line.starts_with(&format!("POST /v1/generation/tasks/{TASK}/deliveries/{FILE}/ack ")){
                assert!(!lower.contains("x-account-group-id:"));let body:serde_json::Value=serde_json::from_slice(body).unwrap();
                assert_eq!(body["sha256"],cutout_sha256_hex(&mask));assert_eq!(body["size_bytes"],mask.len() as u64);
                let data=writer.load_client_state_for_namespace(&lease).unwrap().unwrap();
                let asset=data.assets.iter().find(|asset|asset.origin=="image_cutout").expect("remote ack preceded real owned SQLite save");
                let derived=authority.read_image_source(Path::new(&asset.source_path),100*1024*1024).unwrap();
                assert_ne!(cutout_sha256_hex(&derived),cutout_sha256_hex(&mask),"remote mask hash was substituted for derived PNG");
                let pixels=image::load_from_memory(&derived).unwrap().to_rgba8();
                assert_eq!(pixels.get_pixel(0,0).0,[12,34,56,0]);assert_eq!(pixels.get_pixel(1,0).0,[12,34,56,255]);
                let rows=load_pending_generations_for_namespace(&authority).unwrap();assert_eq!(rows.len(),1);
                assert_eq!(rows[0].reference_paths.len(),1,"original mask source released before remote acknowledgment");
                acks.fetch_add(1,Ordering::Release);return(200,envelope(serde_json::json!({})));
            }
            panic!("unexpected cutout route");
        });
    }
    #[test]
    fn core_cutout_real_submit_persists_original_source_before_upload_and_keeps_saved_subject(){
        let server=Scenario::new();let(f,app)=fixture(Some(&server.url));let mut server=server;
        let _source=install_viewer_source(&f,&app);publish_group(&f,PAYER);
        let authority=f.authority.clone();let url=server.url.clone();
        server.start(move|headers,_|{
            let line=headers.lines().next().unwrap();let lower=headers.to_ascii_lowercase();
            let rows=load_pending_generations_for_namespace(&authority).unwrap();assert_eq!(rows.len(),1);
            assert_eq!(rows[0].reference_paths.len(),1);assert_eq!(rows[0].reference_sha256.len(),1);assert_eq!(rows[0].quality,"general");
            if line.starts_with("POST /v1/uploads/references "){
                assert!(!lower.contains("x-account-group-id:"));
                return(200,envelope(serde_json::json!({"file":{"id":INPUT},"upload":{"method":"POST","url":format!("{url}upload"),"fields":{},"file_field":"file"}})));
            }
            if line.starts_with("POST /upload "){return(200,Vec::new());}
            if line.starts_with(&format!("POST /v1/uploads/references/{INPUT}/complete ")){return(200,envelope(serde_json::json!({})));}
            assert!(line.starts_with("POST /v1/toolbox/image-cutouts "));assert!(lower.contains(&format!("x-account-group-id: {PAYER}")));
            (403,serde_json::to_vec(&serde_json::json!({"request_id":"private","data":null,"error":{"code":"account_group_not_selectable","message":"private","details":null},"meta":null})).unwrap())
        });
        app.global::<AppState>().invoke_submit_cutout("general".into());pump_until(||!app.global::<AppState>().get_cutout_processing());
        let first=load_pending_generations_for_namespace(&f.authority).unwrap().remove(0);
        assert!(Path::new(&first.reference_paths[0]).starts_with(f.persistence.lease().namespace.root()));
        publish_group(&f,TASK);app.global::<AppState>().invoke_submit_cutout("sky".into());
        pump_until(||!app.global::<AppState>().get_cutout_processing());server.finish();
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].client_request_id,first.client_request_id);
        assert_eq!(rows[0].reference_sha256,first.reference_sha256);assert_eq!(rows[0].quality,"general");assert_eq!(rows[0].billing_account_group_id,PAYER);
        let requests=server.requests.lock().unwrap();let posts=requests.iter().filter(|(head,_)|head.starts_with("POST /v1/toolbox/image-cutouts ")).collect::<Vec<_>>();
        assert_eq!(posts.len(),2);assert_eq!(posts[0].1,posts[1].1);
    }
    #[test]
    fn core_cutout_changed_retained_source_blocks_real_upload_without_removing_intent(){
        let(f,app)=fixture(None);install_viewer_source(&f,&app);publish_group(&f,PAYER);
        CUTOUT_TEST_AFTER_SOURCE.with(|hook|*hook.borrow_mut()=Some(Box::new(|authority|{
            let row=load_pending_generations_for_namespace(authority).unwrap().remove(0);
            std::fs::write(&row.reference_paths[0],b"source replaced after durable fingerprint").unwrap();
        })));
        app.global::<AppState>().invoke_submit_cutout("skin".into());drain_cutout_test_workers();pump_for(Duration::from_millis(100));
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(rows.len(),1);
        assert!(rows[0].uploaded_file_ids.is_empty());assert!(rows[0].server_task_id.is_empty());assert_eq!(rows[0].quality,"skin");
        assert!(!app.global::<AppState>().get_cutout_processing());assert_eq!(f.context.store.borrow().assets.len(),1);
    }
    #[test]
    fn core_cutout_derived_skin_png_is_durable_before_ack_of_original_remote_mask(){
        let server=Scenario::new();let(f,app)=fixture(Some(&server.url));let mut server=server;
        let source=install_viewer_source(&f,&app);let row=retained_with_source(&f,&source);
        let acks=Arc::new(std::sync::atomic::AtomicUsize::new(0));serve_cutout_delivery(&mut server,&f,acks.clone());
        resume_pending_image_cutout(&app,f.context.clone(),row);pump_until(||app.global::<AppState>().get_cutout_progress()==100);server.finish();
        assert_eq!(acks.load(Ordering::Acquire),1);let data=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert!(data.generations.is_empty());let outputs=data.assets.iter().filter(|asset|asset.origin=="image_cutout").collect::<Vec<_>>();assert_eq!(outputs.len(),1);
        let asset=outputs[0];assert_eq!(asset.title,"Original portrait 抠图");assert_eq!(asset.prompt,"智能抠图（皮肤）");
        assert_eq!(asset.category,"other");assert_eq!(asset.kind,"game");assert_eq!(asset.model,"智能抠图");assert!(asset.cutout_done);
        assert!(!asset.upscale_done && !asset.remove_black_done);assert_eq!((asset.width,asset.height),(80,80));
        assert!(!f.context.store.borrow().assets.iter().find(|staged|staged.id==asset.id).unwrap().is_new);
        assert_eq!(asset.reference_paths,vec![source]);assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
    }

    #[test]
    fn core_cutout_same_asset_id_replaced_source_saves_original_without_presenting_it(){
        let server=Scenario::new();let(f,app)=fixture(Some(&server.url));let mut server=server;
        let source=install_viewer_source(&f,&app);let row=retained_with_source(&f,&source);
        let replacement=image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(80,80,image::Rgba([90,80,70,255])));
        let replacement=persist_reference_image_for_namespace(&f.authority,&replacement).unwrap().to_string_lossy().into_owned();
        assert_ne!(source,replacement,"fixture must replace the actual owned source path");
        f.context.store.borrow_mut().assets.iter_mut().find(|asset|asset.id==SOURCE).unwrap().source_path=replacement.clone();
        app.global::<AppState>().set_viewer_source_path(replacement.clone().into());
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        let acks=Arc::new(std::sync::atomic::AtomicUsize::new(0));serve_cutout_delivery(&mut server,&f,acks.clone());
        resume_pending_image_cutout(&app,f.context.clone(),row);
        pump_until(||acks.load(Ordering::Acquire)==1 && !app.global::<AppState>().get_cutout_processing());
        server.finish();
        let data=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(data.assets.iter().filter(|asset|asset.origin=="image_cutout").count(),1);
        assert_eq!(data.assets.iter().find(|asset|asset.id==SOURCE).unwrap().source_path,replacement);
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
        assert_eq!(app.global::<AppState>().get_viewer_id(),SOURCE);
        assert_eq!(app.global::<AppState>().get_viewer_source_path(),replacement);
        assert!(app.global::<AppState>().get_cutout_result_path().is_empty(),"old task result was attached to a replaced same-ID source");
        assert!(app.global::<AppState>().get_cutout_message().contains("当前查看图片未改变"));
    }
    #[test]
    fn core_cutout_same_lease_distinct_persistence_rejects_original_late_completion(){
        let server=Http::new(403);let(f,app)=fixture(Some(&server.url));let mut server=server;
        let source=install_viewer_source(&f,&app);let row=retained_with_source(&f,&source);
        resume_pending_image_cutout(&app,f.context.clone(),row.clone());
        server.seen.recv_timeout(Duration::from_secs(3)).unwrap();
        let backend=f.context.backend.as_ref().unwrap();
        let replacement=PrivatePersistence::for_test_with_storage((*f.writer).clone(),f.persistence.lease().clone(),
            f.context.user_activity.clone(),backend.api.upgrade_latch().clone(),
            f.context.data_root_capability.clone().unwrap(),backend.api.clone(),f.context.file_index.clone().unwrap());
        assert_eq!(replacement.lease(),f.persistence.lease());
        assert!(!replacement.same_binding_metadata(&f.persistence));
        f.context.store.borrow_mut().private_persistence=Some(replacement);
        app.global::<AppState>().set_cutout_message("replacement binding protected".into());
        server.finish();drain_cutout_test_workers();pump_for(Duration::from_millis(100));
        assert_eq!(app.global::<AppState>().get_cutout_message(),"replacement binding protected");
        assert!(app.global::<AppState>().get_cutout_result_path().is_empty());
        assert_eq!(load_pending_generations_for_namespace(&f.authority).unwrap()[0].identity(),row.identity());
    }

    #[test]
    fn core_cutout_failed_derived_store_ack_retry_preserves_later_asset_edit(){
        let server=Scenario::new();let(f,app)=fixture(Some(&server.url));let mut server=server;
        let source=install_viewer_source(&f,&app);let row=retained_with_source(&f,&source);
        let root=f.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let sql=rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        sql.execute_batch("CREATE TRIGGER reject_cutout_save BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'controlled save failure'); END;").unwrap();
        let acks=Arc::new(std::sync::atomic::AtomicUsize::new(0));serve_cutout_delivery(&mut server,&f,acks.clone());
        resume_pending_image_cutout(&app,f.context.clone(),row.clone());pump_until(||app.global::<AppState>().get_cutout_message().contains("本地保存未确认"));
        assert_eq!(acks.load(Ordering::Acquire),0);assert_eq!(load_pending_generations_for_namespace(&f.authority).unwrap()[0].reference_sha256,row.reference_sha256);
        f.context.store.borrow_mut().assets.iter_mut().find(|asset|asset.origin=="image_cutout").unwrap().title="edited during failed save".into();
        sql.execute_batch("DROP TRIGGER reject_cutout_save").unwrap();resume_pending_image_cutout(&app,f.context.clone(),row);
        pump_until(||app.global::<AppState>().get_cutout_progress()==100);server.finish();
        let data=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(data.assets.iter().filter(|asset|asset.origin=="image_cutout").count(),1);
        assert_eq!(data.assets.iter().find(|asset|asset.origin=="image_cutout").unwrap().title,"edited during failed save");
        assert_eq!(acks.load(Ordering::Acquire),1);
    }
    struct JoinedCutoutTrip(Option<std::thread::JoinHandle<()>>);
    impl JoinedCutoutTrip{
        fn start(latch:UpgradeLatch)->Self{Self(Some(std::thread::spawn(move||{
            latch.trip(RequiredUpgrade{minimum_version:Some("99.0.0".into())});
        })))}
        fn finish(&mut self){if let Some(worker)=self.0.take(){worker.join().unwrap();}}
    }
    impl Drop for JoinedCutoutTrip{fn drop(&mut self){
        if let Some(worker)=self.0.take(){let result=worker.join();if !std::thread::panicking(){result.unwrap();}}
    }}

    #[test]
    fn core_cutout_late_saved_error_does_not_replace_the_new_viewer(){
        let server=Http::new(403);let(f,app)=fixture(Some(&server.url));let mut server=server;
        let source=install_viewer_source(&f,&app);let row=retained_with_source(&f,&source);
        resume_pending_image_cutout(&app,f.context.clone(),row.clone());
        server.seen.recv_timeout(Duration::from_secs(3)).unwrap();
        app.global::<AppState>().set_viewer_id("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".into());
        app.global::<AppState>().set_viewer_source_path("a different current viewer".into());
        app.global::<AppState>().set_cutout_message("new viewer protected".into());
        server.finish();drain_cutout_test_workers();pump_for(Duration::from_millis(100));
        assert_eq!(app.global::<AppState>().get_cutout_message(),"new viewer protected");
        assert!(app.global::<AppState>().get_cutout_result_path().is_empty());
        assert_eq!(load_pending_generations_for_namespace(&f.authority).unwrap()[0].reference_sha256,row.reference_sha256);
    }
    #[test]
    fn core_cutout_held_resource_error_after_upgrade_does_not_publish(){
        let server=Http::new(403);let(f,app)=fixture(Some(&server.url));let mut server=server;
        let source=install_viewer_source(&f,&app);let row=retained_with_source(&f,&source);
        resume_pending_image_cutout(&app,f.context.clone(),row);server.seen.recv_timeout(Duration::from_secs(3)).unwrap();
        app.global::<AppState>().set_cutout_message("upgrade protected result".into());
        let latch=f.persistence.upgrade_latch();let mut trip=JoinedCutoutTrip::start(latch.clone());
        let deadline=Instant::now()+Duration::from_secs(2);
        while !latch.is_tripped() && Instant::now()<deadline{std::thread::sleep(Duration::from_millis(2));}
        let tripped=latch.is_tripped();server.release();trip.finish();assert!(tripped);
        drain_cutout_test_workers();pump_for(Duration::from_millis(100));server.finish();
        assert_eq!(app.global::<AppState>().get_cutout_message(),"upgrade protected result");
        assert!(app.global::<AppState>().get_cutout_result_path().is_empty());assert_eq!(f.context.store.borrow().assets.len(),1);
    }
    #[test]
    fn core_cutout_new_reservation_survives_ui_busy_clear_and_saved_discovery(){
        let server=Http::new(403);let(f,app)=fixture(Some(&server.url));let mut server=server;
        install_viewer_source(&f,&app);publish_group(&f,PAYER);server.release();
        let(sent,ready)=mpsc::channel();let(release,wait)=mpsc::channel();
        let mut held=HeldCutout{ready,release:Some(release)};
        CUTOUT_TEST_AFTER_SOURCE.with(|hook|*hook.borrow_mut()=Some(Box::new(move|_|{
            let _=sent.send(());let _=wait.recv_timeout(Duration::from_secs(3));
        })));
        app.global::<AppState>().invoke_submit_cutout("skin".into());held.ready.recv_timeout(Duration::from_secs(3)).unwrap();
        let row=load_pending_generations_for_namespace(&f.authority).unwrap().remove(0);
        let request=CUTOUT_UI.with(|ui|ui.borrow().request);
        app.global::<AppState>().set_cutout_processing(false);app.global::<AppState>().invoke_submit_cutout("sky".into());
        resume_pending_image_cutout(&app,f.context.clone(),row.clone());
        assert_eq!(CUTOUT_UI.with(|ui|ui.borrow().request),request);assert_eq!(CUTOUT_WORKERS.with(|workers|workers.borrow().len()),1);
        held.release();drain_cutout_test_workers();pump_for(Duration::from_millis(100));server.finish();
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(rows.len(),1);
        assert_eq!(rows[0].client_request_id,row.client_request_id);assert_eq!(rows[0].quality,"skin");
    }
    #[test]
    fn core_cutout_subject_minimum_is_enforced_by_actual_submit(){
        let(f,app)=fixture(None);install_viewer_source(&f,&app);publish_group(&f,PAYER);
        let image=image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(32,80,image::Rgba([12,34,56,255])));
        let path=persist_reference_image_for_namespace(&f.authority,&image).unwrap().to_string_lossy().into_owned();
        f.context.store.borrow_mut().assets[0].source_path=path.clone();app.global::<AppState>().set_viewer_source_path(path.into());
        app.global::<AppState>().invoke_submit_cutout("general".into());drain_cutout_test_workers();pump_for(Duration::from_millis(100));
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
        assert!(app.global::<AppState>().get_cutout_message().contains("33"));assert!(!app.global::<AppState>().get_cutout_processing());
    }
    #[test]
    fn core_cutout_closed_window_cancels_and_reaps_without_live_ui_join(){
        let server=Http::new(403);let(f,app)=fixture(Some(&server.url));let mut server=server;
        server.release();let row=retained(&f);let mut held=HeldCutout::install();
        resume_pending_image_cutout(&app,f.context.clone(),row);held.ready.recv_timeout(Duration::from_secs(3)).unwrap();drop(app);
        let advanced=Rc::new(Cell::new(false));let observed=advanced.clone();
        slint::Timer::single_shot(Duration::ZERO,move||observed.set(true));pump_for(Duration::from_millis(120));
        assert!(advanced.get());assert_eq!(CUTOUT_WORKERS.with(|workers|workers.borrow().len()),1);
        assert!(CUTOUT_WORKERS.with(|workers|workers.borrow().iter().all(|worker|worker.cancel.load(Ordering::Acquire))));
        held.release();drain_cutout_test_workers();pump_for(Duration::from_millis(100));server.finish();
        assert!(CUTOUT_WORKERS.with(|workers|workers.borrow().is_empty()));
    }
    #[test]
    fn core_cutout_registered_panic_is_sticky_and_shutdown_still_drains(){
        let(mut f,app)=fixture(None);f.expected_failure=true;
        let capture=CutoutCapture::capture(&app,f.context.clone(),f.persistence.clone()).unwrap();
        spawn_cutout_work::<()>(&app,capture,false,|_,_,_|panic!("controlled cutout panic"),|_,_,_|panic!("panic result published"));
        pump_until(||CUTOUT_FAILED.with(Cell::get));assert!(CUTOUT_CLOSED.with(Cell::get));
        assert!(shutdown_cutout_workers().is_err());assert!(join_cutout_workers().is_err());
        app.global::<AppState>().set_cutout_message("closed after panic".into());app.global::<AppState>().invoke_submit_cutout("general".into());
        assert_eq!(app.global::<AppState>().get_cutout_message(),"closed after panic");
    }
    #[test]
    fn core_cutout_undelivered_real_timer_drops_safely_at_ui_thread_exit(){
        std::thread::spawn(||{
            let server=Http::new(403);let(f,app)=fixture(Some(&server.url));let mut server=server;server.release();
            let row=retained(&f);resume_pending_image_cutout(&app,f.context.clone(),row);
            drain_cutout_test_workers();server.finish();drop(app);drop(f);
        }).join().unwrap();
    }


    #[test]
    fn core_cutout_owned_canvas_reference_viewer_reaches_actual_upload_without_pixel_fallback(){
        let server=Http::new(403);let(f,app)=fixture(Some(&server.url));let mut server=server;server.release();
        let path=owned(&f).to_string_lossy().into_owned();
        f.context.store.borrow_mut().canvas_references.push(ReferenceData{id:SOURCE.into(),source_path:path.clone()});
        let state=app.global::<AppState>();state.set_page("canvas".into());state.set_viewer_source("reference".into());
        state.set_viewer_id(SOURCE.into());state.set_viewer_source_path(path.into());state.set_viewer_title("Reference image".into());
        state.set_viewer_open(false);state.set_cutout_open(true);publish_group(&f,PAYER);
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        state.invoke_submit_cutout("general".into());
        let request=server.seen.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(request.starts_with("POST /v1/uploads/references "));
        server.finish();drain_cutout_test_workers();pump_for(Duration::from_millis(100));
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].quality,"general");
        assert!(Path::new(&rows[0].reference_paths[0]).starts_with(f.persistence.lease().namespace.root()));
    }
    #[test]
    fn core_cutout_empty_viewer_path_cannot_borrow_cached_pixels_as_authority(){
        let(f,app)=fixture(None);install_viewer_source(&f,&app);publish_group(&f,PAYER);
        f.context.store.borrow_mut().assets[0].source_path=String::new();
        let state=app.global::<AppState>();state.set_viewer_source_path("".into());
        let pixels=slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(80,80);state.set_viewer_image(Image::from_rgba8(pixels));
        state.invoke_submit_cutout("general".into());drain_cutout_test_workers();pump_for(Duration::from_millis(100));
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());assert!(!state.get_cutout_processing());
    }
    #[test]
    fn core_cutout_verified_empty_terminal_releases_only_unambiguous_source(){
        let server=Scenario::new();let(f,app)=fixture(Some(&server.url));let mut server=server;
        let source=install_viewer_source(&f,&app);let mode=Arc::new(std::sync::atomic::AtomicUsize::new(0));let response_mode=mode.clone();
        server.start(move|headers,_|{
            assert!(headers.starts_with(&format!("GET /v1/generation/tasks/{TASK} ")));
            assert!(!headers.to_ascii_lowercase().contains("x-account-group-id:"));
            (200,envelope(serde_json::json!({"id":TASK,"billing_account_group_id":PAYER,"status":"failed","progress_percent":100,
                "success_count":if response_mode.load(Ordering::Acquire)==1{1}else{0},"failure_count":1,
                "failure":null,"prompt":null,"result_prompt":null,"request":{},"model":null,"quality":"skin","requested_count":1,"type":"image_cutout","items":[]})))
        });
        let row=retained_with_source(&f,&source);resume_pending_image_cutout(&app,f.context.clone(),row);
        pump_until(||!app.global::<AppState>().get_cutout_processing());
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
        mode.store(1,Ordering::Release);let row=retained_with_source(&f,&source);
        resume_pending_image_cutout(&app,f.context.clone(),row.clone());pump_until(||!app.global::<AppState>().get_cutout_processing());
        assert_eq!(load_pending_generations_for_namespace(&f.authority).unwrap()[0].reference_sha256,row.reference_sha256);
        mode.store(2,Ordering::Release);let mut pending=row.clone();
        pending.deliveries.push(PendingDeliveryRecord{file_id:FILE.into(),sha256:"a".repeat(64),size_bytes:1,..Default::default()});
        let scope=BillingScope{request:GroupRequestScope{session:f.context.current_account_session_scope().unwrap(),account_group_id:PAYER.into()},context_epoch:1};
        upsert_pending_generation_for_namespace(&f.authority,&scope,pending.clone()).unwrap();
        resume_pending_image_cutout(&app,f.context.clone(),pending);pump_until(||!app.global::<AppState>().get_cutout_processing());server.finish();
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].deliveries.len(),1);
        assert_eq!(rows[0].reference_paths,row.reference_paths);assert_eq!(rows[0].reference_sha256,row.reference_sha256);
    }

}
