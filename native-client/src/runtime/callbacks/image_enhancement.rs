use super::*;
use std::sync::atomic::{AtomicBool,AtomicI32};

fn sha256_hex(bytes:&[u8])->String {
    use sha2::Digest;
    format!("{:x}",sha2::Sha256::digest(bytes))
}

const ENHANCEMENT_MAX_INPUT_BYTES:u64=20*1024*1024;
const ENHANCEMENT_MAX_OWNED_BYTES:u64=100*1024*1024;
const ENHANCEMENT_MIN_EDGE:u32=64;
const ENHANCEMENT_MAX_LONG_EDGE:u32=5000;
const ENHANCEMENT_MAX_ASPECT_RATIO:u32=2;
const ENHANCEMENT_POLL_RETRY_LIMIT:usize=4;
type EnhancementPickerCompletion=Box<dyn FnOnce(Option<PathBuf>)>;
#[derive(Clone,Copy,Debug)]
enum EnhancementSourceError{Unsupported,TooLarge,Dimensions,AspectRatio}
impl std::fmt::Display for EnhancementSourceError{
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"enhancement input rejected")}
}
impl std::error::Error for EnhancementSourceError{}
#[derive(Debug)]
struct EnhancementNoOutput;
impl std::fmt::Display for EnhancementNoOutput{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"enhancement ended without output")}}
impl std::error::Error for EnhancementNoOutput{}
#[cfg(test)]
thread_local!{
    static ENHANCEMENT_TEST_PICKER:RefCell<Option<Box<dyn FnOnce(EnhancementPickerCompletion)>>>=const{RefCell::new(None)};
    static ENHANCEMENT_TEST_REVEAL:RefCell<Option<Box<dyn FnOnce(&Path)->Result<()>>>>=const{RefCell::new(None)};
    static ENHANCEMENT_TEST_AFTER_SEND:RefCell<Option<Box<dyn FnOnce()+Send>>>=const{RefCell::new(None)};
}
fn enhancement_pick_source(done:EnhancementPickerCompletion){
    #[cfg(test)]
    if let Some(picker)=ENHANCEMENT_TEST_PICKER.with(|hook|hook.borrow_mut().take()){picker(done);return;}
    let _=slint::spawn_local(async move{
        let path=rfd::AsyncFileDialog::new().add_filter("Images",&["jpg","jpeg","png","webp"]).pick_file().await.map(|file|file.path().to_path_buf());
        done(path);
    });
}
fn enhancement_reveal_source(path:&Path)->Result<()>{
    #[cfg(test)]
    if let Some(reveal)=ENHANCEMENT_TEST_REVEAL.with(|hook|hook.borrow_mut().take()){return reveal(path);}
    reveal_path_in_file_manager(path)
}
#[derive(Clone)]
struct EnhancementSourceProof{lease:NamespaceLease,path:String,sha256:String,size:u64}
#[derive(Default)]
struct EnhancementUi{
    context:Option<AppContext>,request:Option<Uuid>,source:Option<EnhancementSourceProof>,
    busy:Vec<(NamespaceLease,Uuid,String)>,
}
thread_local!{
    static ENHANCEMENT_UI:Rc<RefCell<EnhancementUi>>=Rc::new(RefCell::new(EnhancementUi::default()));
    static ENHANCEMENT_WORKERS:RefCell<Vec<EnhancementWorker>>=const{RefCell::new(Vec::new())};
    static ENHANCEMENT_CLOSED:Cell<bool>=const{Cell::new(false)};
    static ENHANCEMENT_FAILED:Cell<bool>=const{Cell::new(false)};
}
#[derive(Clone)]
struct EnhancementCapture{
    context:AppContext,persistence:PrivatePersistence,session:SessionScope,
    ui:Rc<RefCell<EnhancementUi>>,request:Uuid,source:String,
}
impl EnhancementCapture{
    fn binding_matches(&self)->bool{
        self.context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.lease()==self.persistence.lease())
            && self.context.active_namespace.lock().ok().is_some_and(|active|active.as_ref()==Some(self.persistence.lease()))
    }
    fn current(&self)->bool{
        !ENHANCEMENT_CLOSED.with(Cell::get) && self.binding_matches() && self.persistence.is_current()
            && self.context.backend.as_ref().is_some_and(|backend|backend.api.session().is_scope_current(&self.session))
    }
    fn presentation_matches(&self,app:&AppWindow)->bool{
        self.ui.borrow().request==Some(self.request) && app.global::<AppState>().get_enhance_source_path()==self.source
    }
    fn apply<R>(&self,app:&AppWindow,apply:impl FnOnce()->R)->Option<R>{
        if !self.current(){return None;}
        self.context.apply_user_completion(self.persistence.lease(),||{
            if !self.binding_matches() || !self.presentation_matches(app){return None;}
            Some(apply())
        }).ok().flatten()
    }
    fn capture(app:&AppWindow,context:AppContext,persistence:PrivatePersistence)->Option<Self>{
        let session=context.current_account_session_scope()?;
        let ui=ENHANCEMENT_UI.with(Clone::clone);
        let capture=Self{context,persistence,session,ui,request:Uuid::new_v4(),source:app.global::<AppState>().get_enhance_source_path().to_string()};
        if !capture.current(){return None;}
        capture.context.apply_user_completion(capture.persistence.lease(),||{
            if !capture.binding_matches(){return false;}
            capture.ui.borrow_mut().request=Some(capture.request);true
        }).ok().filter(|value|*value)?;
        Some(capture)
    }
}
struct EnhancementBusy{ui:Rc<RefCell<EnhancementUi>>,lease:NamespaceLease,id:Uuid}
impl Drop for EnhancementBusy{
    fn drop(&mut self){self.ui.borrow_mut().busy.retain(|(lease,id,_)|lease!=&self.lease || *id!=self.id);}
}
fn reserve_enhancement(capture:&EnhancementCapture,key:Option<&str>)->Option<EnhancementBusy>{
    let mut ui=capture.ui.borrow_mut();
    if ui.busy.iter().any(|(lease,_,active_key)|lease==capture.persistence.lease() && (active_key.is_empty() || key.map_or(true,|key|key==active_key))){return None;}
    let id=Uuid::new_v4();let lease=capture.persistence.lease().clone();ui.busy.push((lease.clone(),id,key.unwrap_or("").to_owned()));
    Some(EnhancementBusy{ui:capture.ui.clone(),lease,id})
}
fn enhancement_busy(persistence:&PrivatePersistence)->bool{
    ENHANCEMENT_UI.with(|ui|ui.borrow().busy.iter().any(|(lease,_,_)|lease==persistence.lease()))
}
fn enhancement_capture_from_store(app:&AppWindow,store:&Store)->Option<EnhancementCapture>{
    let persistence=store.private_persistence.clone()?;
    let context=ENHANCEMENT_UI.with(|ui|ui.borrow().context.clone())?;
    {let original=context.store.borrow();if !std::ptr::eq(&*original,store) || !original.private_persistence.as_ref().is_some_and(|bound|bound.lease()==persistence.lease()){return None;}}
    EnhancementCapture::capture(app,context,persistence)
}
fn enhancement_current_capture(app:&AppWindow,context:AppContext)->Option<EnhancementCapture>{
    let persistence=context.store.borrow().private_persistence.clone()?;
    EnhancementCapture::capture(app,context,persistence)
}
struct EnhancementWorker{id:Uuid,lease:NamespaceLease,cancel:Arc<AtomicBool>,handle:std::thread::JoinHandle<()>}
fn enhancement_worker_failed(){
    ENHANCEMENT_FAILED.with(|failed|failed.set(true));ENHANCEMENT_CLOSED.with(|closed|closed.set(true));
    cancel_enhancement_workers_for_upgrade();
}
fn reap_enhancement_workers(){
    let ready=ENHANCEMENT_WORKERS.with(|workers|{
        let mut workers=workers.borrow_mut();let mut ready=Vec::new();let mut index=0;
        while index<workers.len(){if workers[index].handle.is_finished(){ready.push(workers.remove(index));}else{index+=1;}}ready
    });
    for worker in ready{if worker.handle.join().is_err(){enhancement_worker_failed();}}
}
fn enhancement_worker_pending(id:Uuid)->bool{
    ENHANCEMENT_WORKERS.with(|workers|workers.borrow().iter().any(|worker|worker.id==id))
}
pub(super) fn cancel_enhancement_workers_for_retirement(lease:&NamespaceLease){
    ENHANCEMENT_WORKERS.with(|workers|for worker in workers.borrow().iter().filter(|worker|&worker.lease==lease){worker.cancel.store(true,Ordering::Release);});
}
pub(super) fn cancel_enhancement_workers_for_upgrade(){
    ENHANCEMENT_WORKERS.with(|workers|for worker in workers.borrow().iter(){worker.cancel.store(true,Ordering::Release);});
}
fn join_enhancement_workers()->Result<()>{
    let mut workers=ENHANCEMENT_WORKERS.with(|workers|std::mem::take(&mut *workers.borrow_mut())).into_iter();
    while let Some(worker)=workers.next(){if worker.handle.join().is_err(){
        enhancement_worker_failed();for pending in workers.as_slice(){pending.cancel.store(true,Ordering::Release);}
    }}
    anyhow::ensure!(!ENHANCEMENT_FAILED.with(Cell::get),"enhancement worker panicked");Ok(())
}
/// Owner UI thread, after event-loop exit, outside ordinary/short guards.
pub(super) fn shutdown_enhancement_workers()->Result<()>{
    ENHANCEMENT_CLOSED.with(|closed|closed.set(true));cancel_enhancement_workers_for_upgrade();join_enhancement_workers()
}
#[cfg(test)]
fn drain_enhancement_test_workers(){
    let result=join_enhancement_workers();if !std::thread::panicking(){result.unwrap();}
}
struct EnhancementJob<R>{id:Uuid,cancel:Arc<AtomicBool>,progress:Arc<AtomicI32>,receiver:mpsc::Receiver<Result<R>>,show_progress:bool}
impl<R> Drop for EnhancementJob<R>{fn drop(&mut self){self.cancel.store(true,Ordering::Release);}}
fn enhancement_orphan_poll(id:Uuid){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        reap_enhancement_workers();if enhancement_worker_pending(id){enhancement_orphan_poll(id);}
    });
}
fn spawn_enhancement_work<R:Send+'static>(
    app:&AppWindow,capture:EnhancementCapture,show_progress:bool,
    work:impl FnOnce(&PrivatePersistence,&Arc<AtomicBool>,&Arc<AtomicI32>)->Result<R>+Send+'static,
    complete:impl FnOnce(&AppWindow,&EnhancementCapture,Result<R>)+'static,
)->bool{
    if !capture.current(){return false;}
    let Ok(activity)=capture.persistence.begin_activity()else{return false;};
    let persistence=capture.persistence.clone();let cancel=Arc::new(AtomicBool::new(false));
    let worker_cancel=cancel.clone();let progress=Arc::new(AtomicI32::new(1));let worker_progress=progress.clone();
    let id=Uuid::new_v4();let(sender,receiver)=mpsc::channel();
    #[cfg(test)]
    let after_send=ENHANCEMENT_TEST_AFTER_SEND.with(|hook|hook.borrow_mut().take());
    let spawned=std::thread::Builder::new().name("enhancement-owned-work".into()).spawn(move||{
        let result=if worker_cancel.load(Ordering::Acquire) || activity.is_quiescing() || !persistence.is_current(){
            Err(anyhow!("enhancement work retired"))
        }else{work(&persistence,&worker_cancel,&worker_progress)};
        drop(activity);let _=sender.send(result);
        #[cfg(test)]
        if let Some(after_send)=after_send{after_send();}
    });
    let handle=match spawned{Ok(handle)=>handle,Err(_)=>{
        enhancement_error(app,&capture,anyhow!("enhancement worker unavailable"));return false;
    }};
    ENHANCEMENT_WORKERS.with(|workers|workers.borrow_mut().push(EnhancementWorker{id,lease:capture.persistence.lease().clone(),cancel:cancel.clone(),handle}));
    poll_enhancement_work(app.as_weak(),capture,Rc::new(RefCell::new(EnhancementJob{id,cancel,progress,receiver,show_progress})),complete);true
}
fn poll_enhancement_work<R:Send+'static>(
    weak:Weak<AppWindow>,capture:EnhancementCapture,job:Rc<RefCell<EnhancementJob<R>>>,
    complete:impl FnOnce(&AppWindow,&EnhancementCapture,Result<R>)+'static,
){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        reap_enhancement_workers();let id=job.borrow().id;
        let Some(app)=weak.upgrade()else{
            job.borrow().cancel.store(true,Ordering::Release);if enhancement_worker_pending(id){enhancement_orphan_poll(id);}return;
        };
        if !capture.current(){job.borrow().cancel.store(true,Ordering::Release);}
        if enhancement_worker_pending(id){
            if job.borrow().show_progress{
                let progress=job.borrow().progress.load(Ordering::Acquire).clamp(1,99);
                capture.apply(&app,||app.global::<AppState>().set_enhance_progress(progress));
            }
            poll_enhancement_work(weak,capture,job,complete);return;
        }
        let result=match job.borrow().receiver.try_recv(){
            Ok(result)=>result,Err(TryRecvError::Disconnected)=>Err(anyhow!("enhancement worker disconnected")),
            Err(TryRecvError::Empty)=>{poll_enhancement_work(weak,capture,job.clone(),complete);return;}
        };
        // Terminal-session dispatch must happen after the worker/activity has exited
        // even if session invalidation made ordinary current() false.
        if result.as_ref().err().is_some_and(|error|error.downcast_ref::<ApiError>().is_some_and(|error|error.is_terminal_session_error())){
            enhancement_error(&app,&capture,result.err().unwrap());return;
        }
        if capture.current(){complete(&app,&capture,result);}
    });
}
fn enhancement_error(app:&AppWindow,capture:&EnhancementCapture,error:anyhow::Error){
    if let Some(api)=error.downcast_ref::<ApiError>(){
        if api.is_terminal_session_error(){
            if capture.binding_matches() && terminal_auth_scope_matches_context(&capture.context,&capture.session){
                drop(error);sign_out_locally(app,&capture.context,true,Some(capture.session.auth_epoch));
            }
            return;
        }
    }
    capture.apply(app,||{
        let state=app.global::<AppState>();state.set_enhance_processing(false);state.set_enhance_progress(0);
        if let Some(source)=error.downcast_ref::<EnhancementSourceError>(){set_enhancement_source_error(app,*source);return;}
        let english=state.get_language().as_str()=="en";
        if error.is::<EnhancementNoOutput>(){
            state.set_enhance_message(if english{"The task ended without an enhanced image. You can submit a new task."}else{"原任务已结束且没有增强结果，可重新提交任务"}.into());return;
        }
        state.set_enhance_message(if english{"The operation was not confirmed. Retained work is preserved; retry to recover it."}else{"操作未确认，原任务和结果已保留，请重试恢复"}.into());
        if let Some(api_error)=error.downcast_ref::<ApiError>() {
            if let Some(message)=show_credit_rejection(&state,api_error) {
                state.set_enhance_message(message.into());
            }
        }
    });
}
fn normalized_enhancement_quality(value:&str)->Option<&'static str>{match value{"2K"=>Some("2K"),"4K"=>Some("4K"),_=>None}}
fn set_enhancement_source_error(app:&AppWindow,error:EnhancementSourceError){
    let state=app.global::<AppState>();let english=state.get_language().as_str()=="en";
    let message=match(error,english){
        (EnhancementSourceError::Unsupported,true)=>"Choose a supported JPG, PNG or WebP image",
        (EnhancementSourceError::Unsupported,false)=>"请选择受支持的 JPG、PNG 或 WebP 图片",
        (EnhancementSourceError::TooLarge,true)=>"The image must not exceed 20 MB",
        (EnhancementSourceError::TooLarge,false)=>"图片大小不能超过 20 MB",
        (EnhancementSourceError::Dimensions,true)=>"The image must be at least 64px and its longest edge must not exceed 5000px",
        (EnhancementSourceError::Dimensions,false)=>"图片不得小于 64 像素，且最长边不能超过 5000 像素",
        (EnhancementSourceError::AspectRatio,true)=>"The image aspect ratio must not exceed 2:1",
        (EnhancementSourceError::AspectRatio,false)=>"图片宽高比不能超过 2:1",
    };state.set_enhance_message(message.into());
}
fn decode_enhancement_input(bytes:&[u8])->Result<image::DynamicImage>{
    if bytes.len() as u64>ENHANCEMENT_MAX_INPUT_BYTES{return Err(EnhancementSourceError::TooLarge.into());}
    decode_enhancement_pixels(bytes)
}
fn decode_enhancement_pixels(bytes:&[u8])->Result<image::DynamicImage>{
    let format=image::guess_format(bytes).map_err(|_|EnhancementSourceError::Unsupported)?;
    if !matches!(format,image::ImageFormat::Jpeg|image::ImageFormat::Png|image::ImageFormat::WebP){return Err(EnhancementSourceError::Unsupported.into());}
    let reader=image::ImageReader::with_format(std::io::Cursor::new(bytes),format);
    let(width,height)=reader.into_dimensions().map_err(|_|EnhancementSourceError::Unsupported)?;
    if width.min(height)<ENHANCEMENT_MIN_EDGE || width.max(height)>ENHANCEMENT_MAX_LONG_EDGE{return Err(EnhancementSourceError::Dimensions.into());}
    if width.max(height)>width.min(height).saturating_mul(ENHANCEMENT_MAX_ASPECT_RATIO){return Err(EnhancementSourceError::AspectRatio.into());}
    decode_reference_bytes(bytes).map_err(|_|EnhancementSourceError::Unsupported.into())
}
struct PreparedEnhancementInput{path:String,name:String,preview:PreparedDeliveryPreview,sha256:String,size:u64}
fn prepare_enhancement_input(persistence:&PrivatePersistence,path:Option<&Path>,bytes:Option<Vec<u8>>,name:String)->Result<PreparedEnhancementInput>{
    let authority=persistence.storage_authority()?;
    let bytes=match bytes{Some(bytes)=>bytes,None=>authority.read_image_source(path.ok_or_else(||anyhow!("source missing"))?,ENHANCEMENT_MAX_INPUT_BYTES+1)?};
    let decoded=decode_enhancement_input(&bytes)?;
    let output=persist_reference_image_for_namespace(&authority,&decoded)?;
    let verified=authority.read_image_source(&output,ENHANCEMENT_MAX_OWNED_BYTES)?;
    let preview=prepare_owned_preview(persistence,&output,PreviewPurpose::Canvas)?;
    Ok(PreparedEnhancementInput{path:output.to_str().ok_or_else(||anyhow!("invalid source path"))?.to_owned(),name,preview,sha256:sha256_hex(&verified),size:verified.len() as u64})
}
fn apply_enhancement_input(app:&AppWindow,capture:&EnhancementCapture,result:Result<PreparedEnhancementInput>){
    let input=match result{Ok(input)=>input,Err(error)=>{enhancement_error(app,capture,error);return;}};
    let image=materialize_delivery_preview(&input.preview);
    capture.apply(app,||{
        capture.ui.borrow_mut().source=Some(EnhancementSourceProof{lease:capture.persistence.lease().clone(),path:input.path.clone(),sha256:input.sha256,size:input.size});
        let state=app.global::<AppState>();state.set_enhance_source_path(input.path.into());state.set_enhance_source_name(input.name.into());
        state.set_enhance_source_image(image);state.set_enhance_result_path("".into());state.set_enhance_result_name("".into());
        state.set_enhance_result_image(Image::default());state.set_enhance_processing(false);state.set_enhance_progress(0);
        if normalized_enhancement_quality(state.get_enhance_quality().as_str()).is_none(){state.set_enhance_quality("2K".into());}
        state.set_enhance_estimated_credits("20".into());state.set_enhance_message("".into());
    });
}
fn start_enhancement_paths(app:&AppWindow,capture:EnhancementCapture,paths:Vec<PathBuf>)->bool{
    if paths.is_empty() || enhancement_busy(&capture.persistence){return false;}
    spawn_enhancement_work(app,capture,false,move|persistence,cancel,_|{
        let mut failure=anyhow!(EnhancementSourceError::Unsupported);
        for path in paths{
            if cancel.load(Ordering::Acquire){anyhow::bail!("input superseded");}
            let name=path.file_name().and_then(|name|name.to_str()).unwrap_or("image").to_owned();
            match prepare_enhancement_input(persistence,Some(&path),None,name){Ok(input)=>return Ok(input),Err(error)=>failure=error}
        }Err(failure)
    },apply_enhancement_input)
}
pub(super) fn add_enhancement_paths_for_store(app:&AppWindow,paths:Vec<PathBuf>,store:&Store)->bool{
    let Some(persistence)=store.private_persistence.as_ref()else{return false;};
    if enhancement_busy(persistence){return false;}
    let Some(capture)=enhancement_capture_from_store(app,store)else{return false;};
    start_enhancement_paths(app,capture,paths)
}
pub(super) fn add_enhancement_from_drag_data_for_store(app:&AppWindow,mime_type:&str,data:&str,store:&Store)->bool{
    let Some(persistence)=store.private_persistence.as_ref()else{return false;};
    if enhancement_busy(persistence){return false;}
    if !matches!(mime_type,URI_LIST_MIME|TEXT_PLAIN_MIME|IMAGE_DRAG_MIME|"text/html"){return false;}
    let Some(capture)=enhancement_capture_from_store(app,store)else{return false;};
    if let Some(url)=external_image_url(data){
        capture.apply(app,||app.global::<AppState>().set_enhance_message(if app.global::<AppState>().get_language().as_str()=="en"{"Importing the dropped image..."}else{"正在导入拖入的图片..."}.into()));
        return spawn_enhancement_work(app,capture,false,move|persistence,_,_|{
            // The shared transfer self-retires on exact426; no outer blocking effect.
            let bytes=reference_callbacks::download_captured_reference_bytes(&url,persistence)?;
            prepare_enhancement_input(persistence,None,Some(bytes),"image".into())
        },apply_enhancement_input);
    }
    start_enhancement_paths(app,capture,drag_data_to_paths(data))
}
pub(super) fn wire_image_enhancement_callbacks(app:&AppWindow,context:AppContext){
    ENHANCEMENT_UI.with(|ui|ui.borrow_mut().context=Some(context.clone()));
    let state=app.global::<AppState>();
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_choose_enhance_source(move||{
            let Some(app)=weak.upgrade()else{return;};
            let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
            if enhancement_busy(&persistence){return;}
            let Some(capture)=enhancement_current_capture(&app,context.clone())else{return;};
            let Ok(effect)=capture.persistence.begin_effect()else{return;};let weak=app.as_weak();
            enhancement_pick_source(Box::new(move|path|{
                drop(effect);let Some(app)=weak.upgrade()else{return;};if !capture.current() || !capture.presentation_matches(&app){return;}
                if let Some(path)=path{start_enhancement_paths(&app,capture,vec![path]);}
            }));
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_add_enhance_source_from_drag(move|transfer|{
            let Some(app)=weak.upgrade()else{return false;};let Ok(data)=transfer.plain_text()else{return false;};
            add_enhancement_from_drag_data_for_store(&app,TEXT_PLAIN_MIME,data.as_str(),&context.store.borrow())
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_start_enhance(move|quality|{
            let Some(app)=weak.upgrade()else{return;};start_image_enhancement(&app,context.clone(),quality.as_str());
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_reveal_enhance_result(move||{
            let Some(app)=weak.upgrade()else{return;};
            let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
            if enhancement_busy(&persistence){return;}
            let Some(capture)=enhancement_current_capture(&app,context.clone())else{return;};
            let path=PathBuf::from(app.global::<AppState>().get_enhance_result_path().to_string());
            if !capture.persistence.owns_path(&path) || !context.store.borrow().assets.iter().any(|asset|asset.source_path==path.to_string_lossy() && asset.origin=="image_enhancement"){return;}
            let original=path.clone();
            spawn_enhancement_work(&app,capture,false,move|persistence,_,_|{
                let authority=persistence.storage_authority()?;
                let leaf=path.strip_prefix(authority.lease().namespace.output_dir())?.to_str().ok_or_else(||anyhow!("invalid result path"))?;
                let key=ManagedFileKey::new(ManagedUserArea::Output,leaf)?;
                let mut held=authority.open_optional_regular(&key)?.ok_or_else(||anyhow!("result missing"))?;
                authority.with_regular_reader(&mut held,|_|Ok(()))?;Ok((authority,held))
            },move|app,capture,result|{
                let(authority,mut held)=match result{Ok(value)=>value,Err(error)=>{enhancement_error(app,capture,error);return;}};
                if !capture.current() || !capture.presentation_matches(app) || app.global::<AppState>().get_enhance_result_path()!=original.to_string_lossy(){return;}
                let Ok(effect)=capture.persistence.begin_effect()else{return;};
                let result=authority.with_regular_reader(&mut held,|_|Ok(())).and_then(|_|{
                    anyhow::ensure!(capture.current() && capture.presentation_matches(app),"result retired");
                    enhancement_reveal_source(&original)
                });
                drop(held);drop(effect);
                capture.apply(app,||app.global::<AppState>().set_enhance_message(match(result.is_ok(),app.global::<AppState>().get_language().as_str()=="en"){
                    (true,true)=>"Opened the image folder",(true,false)=>"已打开图片所在文件夹",
                    (false,true)=>"The image folder could not be opened",(false,false)=>"无法打开图片所在文件夹",
                }.into()));
            });
        });
    }
}

fn start_image_enhancement(app:&AppWindow,context:AppContext,target_quality:&str){
    let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
    if enhancement_busy(&persistence){return;}
    let Some(capture)=enhancement_current_capture(app,context.clone())else{return;};
    if app.global::<AppState>().get_enhance_source_path().trim().is_empty(){
        capture.apply(app,||app.global::<AppState>().set_enhance_message(if app.global::<AppState>().get_language().as_str()=="en"{"Upload an image first"}else{"请先上传图片"}.into()));return;
    }
    let Some(quality)=normalized_enhancement_quality(target_quality)else{
        capture.apply(app,||app.global::<AppState>().set_enhance_message(if app.global::<AppState>().get_language().as_str()=="en"{"Choose 2K or 4K quality"}else{"请选择 2K 或 4K 清晰度"}.into()));return;
    };
    if app.global::<AppState>().get_session_state().as_str()!="online"{
        capture.apply(app,||{let state=app.global::<AppState>();state.set_auth_open(true);state.set_enhance_message(if state.get_language().as_str()=="en"{"Sign in and connect to the service before enhancing an image"}else{"请先登录并连接服务后再处理图片"}.into());});return;
    }
    let(scope,authority,activity)=match context.capture_billing_action(KnownCapability::Bill){
        Ok(value)=>value,Err(error)=>{enhancement_error(app,&capture,error.into());return;}
    };
    drop(activity);
    start_image_enhancement_with_billing_scope(app,context,authority,&scope,quality);
}
pub(super) fn start_image_enhancement_with_billing_scope(
    app:&AppWindow,context:AppContext,authority:Arc<NamespaceStorageAuthority>,scope:&BillingScope,target_quality:&str,
){
    let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
    if enhancement_busy(&persistence){return;}
    let Some(capture)=enhancement_current_capture(app,context.clone())else{return;};
    if capture.persistence.lease()!=authority.lease(){return;}
    let Some(quality)=normalized_enhancement_quality(target_quality)else{return;};
    let billing=match capture_billing_scope_for_submission(context.backend.as_deref(),&authority,scope){
        Ok(scope)=>scope,Err(error)=>{enhancement_error(app,&capture,error.into());return;}
    };
    let source=PathBuf::from(&capture.source);
    if !capture.persistence.owns_path(&source){enhancement_error(app,&capture,anyhow!(EnhancementSourceError::Unsupported));return;}
    let Some(original_source)=capture.ui.borrow().source.as_ref().filter(|original|original.lease==*capture.persistence.lease() && original.path==capture.source).cloned()else{
        enhancement_error(app,&capture,anyhow!(EnhancementSourceError::Unsupported));return;
    };
    let Some(busy)=reserve_enhancement(&capture,None)else{return;};
    let key=Uuid::new_v4().to_string();let local_task=Uuid::new_v4().to_string();let quality=quality.to_owned();
    let backend=context.backend.clone().unwrap();
    capture.apply(app,||{
        let state=app.global::<AppState>();state.set_enhance_processing(true);state.set_enhance_progress(1);
        state.set_enhance_quality(quality.clone().into());state.set_enhance_estimated_credits("20".into());
        state.set_enhance_message(if state.get_language().as_str()=="en"{"Preparing the original task..."}else{"正在准备原始任务..."}.into());
    });
    spawn_enhancement_work(app,capture,true,move|persistence,cancel,progress|{
        let authority=persistence.storage_authority()?;
        let retained=load_pending_generations_for_namespace(&authority)?.into_iter().filter(|record|record.task_type=="image_enhancement").collect::<Vec<_>>();
        anyhow::ensure!(retained.len()<=1,"multiple enhancement recovery records require explicit recovery");
        if let Some(record)=retained.into_iter().next(){
            // An earlier failed/uncertain request remains authoritative. No new key/body/payer.
            return run_enhancement_record(&backend,&authority,None,&billing.request.session,record,cancel,progress);
        }
        anyhow::ensure!(!cancel.load(Ordering::Acquire) && persistence.is_current(),"enhancement retired");
        // The original external input already passed its 20 MB policy. Its owned
        // normalized encoding can grow; only exact captured bytes may use this
        // existing 100 MB held-read bound. Recheck identity before decoding.
        let bytes=authority.read_image_source(&source,ENHANCEMENT_MAX_OWNED_BYTES)?;
        anyhow::ensure!(sha256_hex(&bytes)==original_source.sha256 && bytes.len() as u64==original_source.size,"selected enhancement input changed");
        let _decoded=decode_enhancement_pixels(&bytes)?;
        let record=new_enhancement_record(&billing,key,local_task,&source,&quality,&bytes)?;
        // Durable original request precedes both upload and the billable POST.
        upsert_pending_generation_for_namespace(&authority,&billing,record.clone())?;
        run_enhancement_record(&backend,&authority,Some(&billing),&billing.request.session,record,cancel,progress)
    },move|app,capture,result|finish_enhancement_work(app,capture,result,busy));
}
fn new_enhancement_record(scope:&BillingScope,key:String,local_task:String,source:&Path,quality:&str,bytes:&[u8])->Result<PendingGenerationRecord>{
    Ok(PendingGenerationRecord{
        source_asset_id:String::new(),video_request:None,schema_version:2,cancel_requested:false,
        created_at_epoch_ms:Local::now().timestamp_millis(),client_request_id:key,
        owner_user_id:scope.request.session.owner_user_id.clone(),billing_account_group_id:scope.request.account_group_id.clone(),auth_epoch:scope.request.session.auth_epoch,
        local_task_id:local_task,server_task_id:String::new(),raw_prompt:"图片清晰增强".into(),generation_prompt:String::new(),
        task_type:"image_enhancement".into(),category:"other".into(),mode:"game".into(),ratio:String::new(),quality:quality.into(),
        model_code:"aliyun_super_resolution".into(),conversation_id:String::new(),count:1,target_width:0,target_height:0,create_conversation:false,
        reference_paths:vec![source.to_str().ok_or_else(||anyhow!("invalid source path"))?.to_owned()],
        reference_sha256:vec![sha256_hex(bytes)],reference_size_bytes:vec![bytes.len() as u64],
        lineage_reference_paths:vec![source.to_str().unwrap().to_owned()],uploaded_file_ids:vec![],deliveries:vec![],terminal:false,expected_success_count:0,
        canvas_source_node_id:String::new(),canvas_ui_extraction:false,
    })
}
pub(super) fn resume_pending_image_enhancement(app:&AppWindow,context:AppContext,record:PendingGenerationRecord){
    let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
    let duplicate=ENHANCEMENT_UI.with(|ui|ui.borrow().busy.iter().any(|(lease,_,key)|lease==persistence.lease() && (key.is_empty() || key==&record.client_request_id)));
    if duplicate{return;}
    let Some(capture)=enhancement_current_capture(app,context.clone())else{return;};
    let Some(busy)=reserve_enhancement(&capture,Some(&record.client_request_id))else{return;};
    let Some(backend)=context.backend.clone()else{return;};let session=capture.session.clone();
    capture.apply(app,||{let state=app.global::<AppState>();state.set_enhance_processing(true);state.set_enhance_progress(5);
        state.set_enhance_message(if state.get_language().as_str()=="en"{"Recovering the original enhancement task..."}else{"正在恢复原清晰增强任务..."}.into());});
    spawn_enhancement_work(app,capture,true,move|persistence,cancel,progress|{
        let authority=persistence.storage_authority()?;
        run_enhancement_record(&backend,&authority,None,&session,record,cancel,progress)
    },move|app,capture,result|finish_enhancement_work(app,capture,result,busy));
}
fn finish_enhancement_work(app:&AppWindow,capture:&EnhancementCapture,result:Result<PreparedNamespaceDelivery>,busy:EnhancementBusy){
    let prepared=match result{Ok(prepared)=>prepared,Err(error)=>{drop(busy);enhancement_error(app,capture,error);return;}};
    if prepared.lease()!=capture.persistence.lease() || !capture.current(){return;}
    let path=prepared.source_path().to_owned();let original=capture.clone();let original_quality=prepared.record().quality.clone();
    let original_source_matches=prepared.record().lineage_reference_paths.first().or_else(||prepared.record().reference_paths.first()).is_some_and(|source|source==&capture.source);
    start_image_delivery_commit(app,capture.context.clone(),prepared,Local::now().format("%Y-%m-%d %H:%M").to_string(),move|app,result|{
        // This closure already runs under the shared original-lease short completion.
        // Only pure memory checks/setters; do not reacquire capture.current()/latch here.
        if original.binding_matches() && original.presentation_matches(app){
            let state=app.global::<AppState>();let english=state.get_language().as_str()=="en";
            state.set_enhance_processing(false);
            match result{
                Ok((image,_,ack))=>{
                    if !original_source_matches{
                        state.set_enhance_message(if english{"The earlier enhancement was saved to My Assets / Other; the current input was not changed"}else{"原增强结果已保存到“我的资产 / 其他”，当前输入未改变"}.into());
                        drop(busy);return;
                    }
                    let name=Path::new(&path).file_name().and_then(|name|name.to_str()).unwrap_or("Enhanced image");
                    state.set_enhance_quality(original_quality.clone().into());
                    state.set_enhance_result_path(path.clone().into());state.set_enhance_result_name(name.into());state.set_enhance_result_image(image);state.set_enhance_progress(100);
                    state.set_enhance_message(match(ack,english){
                        (true,true)=>"Enhanced image saved to My Assets / Other",(true,false)=>"处理完成，已保存到“我的资产 / 其他”",
                        (false,true)=>"Enhanced image saved locally; server acknowledgment is pending",
                        (false,false)=>"增强结果已本地保存，服务端确认待恢复",
                    }.into());
                },
                Err(_)=>state.set_enhance_message(if english{"Local save is unconfirmed; the original result is retained for retry"}else{"本地保存未确认，原结果已保留，请重试恢复"}.into()),
            }
        }
        drop(busy); // owned pure-memory reservation; no TLS or latch/permit Drop.
    });
}
fn enhancement_worker_current(backend:&BackendRuntime,authority:&NamespaceStorageAuthority,session:&SessionScope,cancel:&AtomicBool)->bool{
    !cancel.load(Ordering::Acquire) && authority.lease().auth_epoch==session.auth_epoch
        && authority.user_public_id()==session.owner_user_id && backend.api.user_work_is_current(session)
        && !backend.api.upgrade_latch().is_tripped()
}
fn enhancement_wait(backend:&BackendRuntime,authority:&NamespaceStorageAuthority,session:&SessionScope,cancel:&AtomicBool,duration:Duration)->bool{
    let deadline=Instant::now()+duration;
    while Instant::now()<deadline{
        if !enhancement_worker_current(backend,authority,session,cancel){return false;}
        std::thread::sleep(Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now())));
    }
    enhancement_worker_current(backend,authority,session,cancel)
}
fn validate_enhancement_detail(record:&PendingGenerationRecord,detail:&GenerationTaskDetail)->Result<()>{
    api::require_saved_group(&record.billing_account_group_id,&detail.billing_account_group_id)?;
    anyhow::ensure!(Uuid::parse_str(&detail.id).is_ok() && (record.server_task_id.is_empty() || record.server_task_id==detail.id),"enhancement task identity mismatch");
    anyhow::ensure!(detail.requested_count==1,"enhancement task count mismatch");Ok(())
}
fn run_enhancement_record(
    backend:&BackendRuntime,authority:&Arc<NamespaceStorageAuthority>,billing:Option<&BillingScope>,session:&SessionScope,
    expected:PendingGenerationRecord,cancel:&Arc<AtomicBool>,progress:&Arc<AtomicI32>,
)->Result<PreparedNamespaceDelivery>{
    let _activity=backend.api.begin_user_work(session)?;
    anyhow::ensure!(enhancement_worker_current(backend,authority,session,cancel),"enhancement retired");
    let mut record=load_pending_generations_for_namespace(authority)?.into_iter().find(|record|record.identity()==expected.identity()).ok_or_else(||anyhow!("original enhancement record changed"))?;
    anyhow::ensure!(record.task_type=="image_enhancement" && record.owner_user_id==session.owner_user_id && record.count==1
        && normalized_enhancement_quality(&record.quality).is_some() && !record.cancel_requested
        && record.canvas_source_node_id.is_empty() && !record.canvas_ui_extraction,"enhancement record unsupported");
    if let Some(billing)=billing{
        capture_billing_scope_for_submission(Some(backend),authority,billing)?;
        anyhow::ensure!(record.billing_account_group_id==billing.request.account_group_id && record.auth_epoch==billing.request.session.auth_epoch,"new enhancement payer changed");
    }
    let api=GenerationApi::new(backend.api.clone()).with_saved_group(&record.billing_account_group_id);
    // Incomplete original input proves no POST was possible: uploaded id was always
    // acknowledged before every create. Cross-epoch rebind changes only auth_epoch.
    if record.auth_epoch!=session.auth_epoch && record.server_task_id.is_empty() && record.uploaded_file_ids.is_empty(){
        anyhow::ensure!(!record.terminal && record.deliveries.is_empty() && record.expected_success_count==0
            && record.reference_paths.len()==1 && record.reference_sha256.len()==1 && record.reference_size_bytes.len()==1
            && generation_references_match_for_namespace(authority,&record),"incomplete old enhancement source is not verifiable");
        anyhow::ensure!(rebind_pending_generation_epoch_for_namespace(authority,&record.identity(),session.auth_epoch)?,"enhancement rebind refused");
        record.auth_epoch=session.auth_epoch;
    }
    if record.server_task_id.is_empty() && record.uploaded_file_ids.is_empty(){
        anyhow::ensure!(record.reference_paths.len()==1 && record.reference_sha256.len()==1 && record.reference_size_bytes.len()==1,"original enhancement input missing");
        anyhow::ensure!(enhancement_worker_current(backend,authority,session,cancel),"enhancement retired");
        let uploaded=api.upload_reference_for_namespace_checked(Path::new(&record.reference_paths[0]),authority,session,false,&record.reference_sha256[0],record.reference_size_bytes[0])?;
        let uploaded=vec![uploaded];
        anyhow::ensure!(apply_generation_patch_for_namespace(authority,&record.identity(),GenerationRecoveryPatch::UploadedFileIds(uploaded.clone()))?,"upload receipt not saved");
        record.uploaded_file_ids=uploaded;
    }
    anyhow::ensure!(enhancement_worker_current(backend,authority,session,cancel),"enhancement retired");
    let mut detail=if record.server_task_id.is_empty(){
        anyhow::ensure!(!record.terminal && record.uploaded_file_ids.len()==1,"original enhancement create body missing");
        if let Some(billing)=billing{
            api.create_image_enhancement_billing(&CreateImageEnhancement{
                client_request_id:record.client_request_id.clone(),reference_file_id:record.uploaded_file_ids[0].clone(),target_quality:record.quality.clone(),
            },billing)?
        }else{
            let replay=SavedReplayRequest::generation(authority.clone(),session,&record.client_request_id)?;
            backend.api.replay_saved::<GenerationTaskDetail>(&replay)?.data
        }
    }else{api.task_scoped(&record.server_task_id,session)?};
    validate_enhancement_detail(&record,&detail)?;
    if record.auth_epoch!=session.auth_epoch{
        anyhow::ensure!(rebind_pending_generation_epoch_for_namespace(authority,&record.identity(),session.auth_epoch)?,"enhancement rebind refused");
        record.auth_epoch=session.auth_epoch;
    }
    if record.server_task_id.is_empty(){
        anyhow::ensure!(apply_generation_patch_for_namespace(authority,&record.identity(),GenerationRecoveryPatch::Accepted{
            server_task_id:detail.id.clone(),uploaded_file_ids:record.uploaded_file_ids.clone(),clear_reference_inputs:true,
        })?,"enhancement task binding not saved");
        record.server_task_id=detail.id.clone();
    }
    if !record.reference_paths.is_empty() || !record.reference_sha256.is_empty() || !record.reference_size_bytes.is_empty(){
        anyhow::ensure!(apply_generation_patch_for_namespace(authority,&record.identity(),GenerationRecoveryPatch::ReleaseReferenceInputs)?,"enhancement inputs not released");
        record.reference_paths.clear();record.reference_sha256.clear();record.reference_size_bytes.clear();
    }
    progress.store(8,Ordering::Release);
    loop{
        anyhow::ensure!(enhancement_worker_current(backend,authority,session,cancel),"enhancement retired");
        validate_enhancement_detail(&record,&detail)?;progress.store(detail.progress_percent.clamp(8,99),Ordering::Release);
        if let Some(item)=detail.items.iter().find(|item|item.status=="succeeded"){
            return prepare_namespace_delivery(&api,authority.clone(),authority.delivery_index()?,&record.identity(),item.index).map_err(Into::into);
        }
        if detail.terminal(){
            // Only exact authoritative no-output terminal completion is releasable.
            // A malformed summary or any prior delivery keeps the recovery source.
            anyhow::ensure!(detail.success_count==0 && detail.items.iter().all(|item|item.status!="succeeded")
                && record.deliveries.is_empty(),"enhancement output remains unresolved");
            anyhow::ensure!(apply_generation_patch_for_namespace(authority,&record.identity(),GenerationRecoveryPatch::Terminal{expected_success_count:0})?,"terminal enhancement not retained");
            anyhow::ensure!(apply_generation_patch_for_namespace(authority,&record.identity(),GenerationRecoveryPatch::ReleaseReferenceInputs)?,"terminal inputs not released");
            let saved=load_pending_generations_for_namespace(authority)?.into_iter().find(|saved|saved.identity()==record.identity()).ok_or_else(||anyhow!("terminal enhancement changed"))?;
            anyhow::ensure!(saved.server_task_id==detail.id && saved.count==1 && saved.terminal && saved.expected_success_count==0
                && saved.deliveries.is_empty() && saved.reference_paths.is_empty() && saved.reference_sha256.is_empty()
                && saved.reference_size_bytes.is_empty(),"terminal enhancement cannot be released");
            anyhow::ensure!(remove_pending_generation_for_namespace(authority,&saved.identity())?,"terminal enhancement release not acknowledged");
            return Err(EnhancementNoOutput.into());
        }
        anyhow::ensure!(enhancement_wait(backend,authority,session,cancel,Duration::from_millis(IMAGE_POLL_INTERVAL_MS)),"enhancement retired");
        let mut retries=0;
        detail=loop{
            match api.task_scoped(&record.server_task_id,session){
                Ok(detail)=>break detail,
                Err(error)if error.should_preserve_generation_recovery() && retries<ENHANCEMENT_POLL_RETRY_LIMIT=>{
                    retries+=1;anyhow::ensure!(enhancement_wait(backend,authority,session,cancel,Duration::from_millis(IMAGE_POLL_INTERVAL_MS)),"enhancement retired");
                },
                Err(error)=>return Err(error.into()),
            }
        };
    }
}
/// Existing direct worker test boundary; production launch uses the registered
/// capture above. Signature preserves the original persisted-payer regression.
#[cfg(test)]
fn run_image_enhancement_worker(
    backend:Arc<BackendRuntime>,authority:Arc<NamespaceStorageAuthority>,billing:BillingScope,session:SessionScope,
    record:PendingGenerationRecord,sender:mpsc::Sender<Result<PreparedNamespaceDelivery>>,
){
    let result=run_enhancement_record(&backend,&authority,Some(&billing),&session,record,&Arc::new(AtomicBool::new(false)),&Arc::new(AtomicI32::new(1)));
    let _=sender.send(result);
}
#[cfg(test)]
mod billing_capture_tests {
    use super::*;
    #[test]
    fn billing_capture_enhancement_worker_keeps_persisted_payer() {
        backend_generation::billing_capture_test_support::assert_generation_worker(
            "image_enhancement",
            run_image_enhancement_worker,
        );
    }
}

#[cfg(test)]
mod core_enhancement_tests{
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
            },DeviceIdentity{id:Uuid::new_v4().to_string(),name:"enhancement-fixture".into(),platform:"macos".into()},
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
        state.set_page("image-enhancement".into());state.set_session_state("online".into());
        wire_image_enhancement_callbacks(&app,inner.context.clone());
        inner.persistence.save_store(local_store_data(&app,&inner.context.store.borrow())).unwrap();
        (Fixture{inner,expected_failure:false},app)
    }
    impl Drop for Fixture{fn drop(&mut self){
        {
            let mut active=self.context.active_namespace.lock().unwrap_or_else(|error|error.into_inner());
            if active.as_ref()==Some(self.persistence.lease()){*active=None;}
        }
        cancel_enhancement_workers_for_retirement(self.persistence.lease());
        let worker_result=join_enhancement_workers();
        let delivery=drain_delivery_commit_workers_for_lease_for_test(self.persistence.lease());
        let retired=self.context.user_activity.begin_quiesce(self.persistence.lease());
        let retired=match retired{Ok(guard)=>{guard.retire();Ok(())},Err(error)=>Err(error)};
        if !std::thread::panicking(){assert_eq!(worker_result.is_err(),self.expected_failure);delivery.unwrap();retired.unwrap();}
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
        assert!(ready(),"enhancement callback completion missing");
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
                        Err(error)=>panic!("controlled enhancement accept: {error}"),
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
    #[test]
    fn core_enhancement_missing_binding_denies_picker_start_and_reveal(){
        let(f,app)=fixture(None);let path=owned(&f);
        f.context.store.borrow_mut().private_persistence=None;
        let called=Rc::new(Cell::new(0));let observed=called.clone();
        ENHANCEMENT_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|done|{observed.set(observed.get()+1);done(None);})));
        let observed=called.clone();ENHANCEMENT_TEST_REVEAL.with(|hook|*hook.borrow_mut()=Some(Box::new(move|_|{observed.set(observed.get()+1);Ok(())})));
        let state=app.global::<AppState>();state.set_enhance_result_path(path.to_string_lossy().into_owned().into());
        state.set_enhance_message("unbound boundary".into());
        state.invoke_choose_enhance_source();state.invoke_start_enhance("2K".into());state.invoke_reveal_enhance_result();
        assert_eq!(called.get(),0,"unbound input or OS effect admitted");assert_eq!(state.get_enhance_message(),"unbound boundary");
        assert!(f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap().assets.is_empty());
    }
    #[test]
    fn core_enhancement_exact_upgrade_does_not_mutate_private_controls(){
        let(f,app)=fixture(None);f.persistence.upgrade_latch().trip(RequiredUpgrade{minimum_version:Some("99.0.0".into())});
        let calls=Rc::new(Cell::new(0));let observed=calls.clone();
        ENHANCEMENT_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|done|{observed.set(observed.get()+1);done(None);})));
        let state=app.global::<AppState>();state.set_enhance_message("upgrade boundary".into());state.set_enhance_progress(73);
        state.invoke_choose_enhance_source();state.invoke_start_enhance("2K".into());state.invoke_reveal_enhance_result();
        assert_eq!(calls.get(),0);assert_eq!(state.get_enhance_message(),"upgrade boundary");assert_eq!(state.get_enhance_progress(),73);
    }
    #[test]
    fn core_enhancement_picker_late_error_preserves_retired_ui(){
        let(f,app)=fixture(None);let external=tempfile::tempdir().unwrap();let path=external.path().join("not-an-image.png");
        std::fs::write(&path,b"controlled invalid image").unwrap();
        let pending=Rc::new(RefCell::new(None));let captured=pending.clone();
        ENHANCEMENT_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|done|*captured.borrow_mut()=Some(done))));
        app.global::<AppState>().invoke_choose_enhance_source();
        *f.context.active_namespace.lock().unwrap()=None;
        app.global::<AppState>().set_enhance_message("retired boundary".into());
        pending.borrow_mut().take().unwrap()(Some(path));drain_enhancement_test_workers();pump_for(Duration::from_millis(80));
        assert_eq!(app.global::<AppState>().get_enhance_message(),"retired boundary");
        assert_eq!(app.global::<AppState>().get_enhance_source_path(),"");
    }
    #[test]
    fn core_enhancement_url_late_error_preserves_retired_ui(){
        let(f,app)=fixture(None);let mut http=Http::new(500);
        assert!(add_enhancement_from_drag_data_for_store(&app,TEXT_PLAIN_MIME,&format!("{}input.png",http.url),&f.context.store.borrow()));
        let request=http.seen.recv_timeout(Duration::from_secs(3)).unwrap();assert!(request.starts_with("GET /input.png "));
        assert!(!request.to_ascii_lowercase().contains("x-account-group-id:"));assert!(!request.to_ascii_lowercase().contains("x-token:"));
        *f.context.active_namespace.lock().unwrap()=None;app.global::<AppState>().set_enhance_message("retired URL boundary".into());
        http.finish();drain_enhancement_test_workers();pump_for(Duration::from_millis(80));
        assert_eq!(app.global::<AppState>().get_enhance_message(),"retired URL boundary");
        assert_eq!(app.global::<AppState>().get_enhance_source_path(),"");
    }
    #[test]
    fn core_enhancement_retained_resume_uses_saved_payer_header_free_get(){
        let transport=Http::new(403);let(f,app)=fixture(Some(&transport.url));let mut http=transport;
        let scope=BillingScope{request:GroupRequestScope{session:f.context.current_account_session_scope().unwrap(),account_group_id:PAYER.into()},context_epoch:1};
        let mut record=backend_generation::billing_capture_test_support::generation_record(&scope,"image_enhancement");
        record.server_task_id=TASK.into();upsert_pending_generation_for_namespace(&f.authority,&scope,record.clone()).unwrap();
        resume_pending_image_enhancement(&app,f.context.clone(),record.clone());
        let request=http.seen.recv_timeout(Duration::from_millis(500));
        if request.is_err(){http.finish();}
        assert!(request.is_ok(),"retained enhancement callback did not fetch its saved task");
        let request=request.unwrap();assert!(request.starts_with(&format!("GET /v1/generation/tasks/{TASK} ")));
        assert!(!request.to_ascii_lowercase().contains("x-account-group-id:"));
        http.finish();drain_enhancement_test_workers();pump_for(Duration::from_millis(80));
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();
        assert_eq!(rows.len(),1);assert_eq!(rows[0].billing_account_group_id,PAYER);assert_eq!(rows[0].client_request_id,record.client_request_id);
        assert!(!app.global::<AppState>().get_enhance_message().contains("private provider"));
    }
    #[test]
    fn core_enhancement_foreign_store_wrapper_cannot_change_current_ui(){
        let(_f,app)=fixture(None);let external=tempfile::tempdir().unwrap();let path=external.path().join("invalid.png");
        std::fs::write(&path,b"invalid controlled image").unwrap();
        let foreign=Store::default();app.global::<AppState>().set_enhance_message("original Store boundary".into());
        assert!(!add_enhancement_paths_for_store(&app,vec![path],&foreign));
        assert_eq!(app.global::<AppState>().get_enhance_message(),"original Store boundary");
    }

    fn publish_group(f:&Fixture,group:&str){
        let manager=&f.context.billing_context;let session=f.context.current_account_session_scope().unwrap();
        if manager.confirmed_scope().is_none(){manager.bind_authenticated_session(session.clone()).unwrap();}
        let ticket=manager.begin_switch(&session,"enhancement-device",group,PreviousBillingAuthority::StillValid).unwrap();
        let snapshot:AccountSnapshot=serde_json::from_value(serde_json::json!({
            "user":{"id":OWNER,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
            "read_only":false,"capabilities":["bill"],"membership":null,"entitlement":{},"credits":null,"quota":null,
            "billing_group":{"group_id":group,"name":"fixture","group_status":"active","role":"owner","member_id":null,"relationship_status":null,
                "readable_context":true,"selectable":true,"group_version":"1","membership_version":null,"capabilities":["bill"],"quota":null}
        })).unwrap();
        let staged=manager.stage_confirmation(&ticket,snapshot.billing_group.clone(),snapshot).unwrap();
        f.writer.save_selected_group(OWNER,"enhancement-device",group).unwrap();manager.publish_persisted(ticket,staged);
    }
    fn select_owned_input(f:&Fixture,app:&AppWindow)->String{
        let path=owned(f);
        assert!(add_enhancement_paths_for_store(app,vec![path],&f.context.store.borrow()));
        pump_until(||!app.global::<AppState>().get_enhance_source_path().is_empty());
        app.global::<AppState>().get_enhance_source_path().to_string()
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
        fn start(&mut self,reply:impl FnMut(&str,&[u8])->(u16,Vec<u8>)+Send+'static){
            self.start_for(Duration::from_secs(15),reply);
        }
        fn start_for(&mut self,duration:Duration,mut reply:impl FnMut(&str,&[u8])->(u16,Vec<u8>)+Send+'static){
            let listener=self.listener.take().unwrap();let stop=self.stop.clone();let requests=self.requests.clone();
            self.worker=Some(std::thread::spawn(move||{
                let deadline=Instant::now()+duration;
                while !stop.load(Ordering::Acquire) && Instant::now()<deadline{
                    let mut stream=match listener.accept(){
                        Ok((stream,_))=>stream,Err(error)if error.kind()==std::io::ErrorKind::WouldBlock=>{std::thread::sleep(Duration::from_millis(2));continue;},
                        Err(error)=>panic!("enhancement scenario accept: {error}"),
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
        serde_json::to_vec(&serde_json::json!({"request_id":"enhancement-fixture","data":data,"error":null,"meta":null})).unwrap()
    }
    fn task_response(url:&str,bytes:&[u8])->Vec<u8>{
        envelope(serde_json::json!({
            "id":TASK,"billing_account_group_id":PAYER,"status":"completed","progress_percent":100,"success_count":1,"failure_count":0,
            "failure":null,"prompt":null,"result_prompt":null,"request":{},"model":null,"quality":"2K","requested_count":1,"task_type":"image_enhancement",
            "items":[{"index":0,"status":"succeeded","credit_cost":"20","failure":null,"file":{
                "id":FILE,"status":"available","mime_type":"image/png","size_bytes":bytes.len().to_string(),"sha256":sha256_hex(bytes),
                "width":80,"height":80,"download_url":format!("{url}output.png")
            }}]
        }))
    }
    fn retained(f:&Fixture,source:&str)->PendingGenerationRecord{
        let scope=BillingScope{request:GroupRequestScope{session:f.context.current_account_session_scope().unwrap(),account_group_id:PAYER.into()},context_epoch:1};
        let mut record=backend_generation::billing_capture_test_support::generation_record(&scope,"image_enhancement");
        record.server_task_id=TASK.into();record.raw_prompt="图片清晰增强".into();record.generation_prompt=String::new();
        record.model_code="aliyun_super_resolution".into();record.conversation_id=String::new();
        record.lineage_reference_paths=vec![source.into()];
        upsert_pending_generation_for_namespace(&f.authority,&scope,record.clone()).unwrap();record
    }
    fn serve_delivery(server:&mut Scenario,f:&Fixture,acks:Arc<std::sync::atomic::AtomicUsize>){
        let url=server.url.clone();let bytes=png();let writer=(*f.writer).clone();let lease=f.persistence.lease().clone();
        server.start(move|headers,_|{
            let line=headers.lines().next().unwrap();let lower=headers.to_ascii_lowercase();
            if line.starts_with(&format!("GET /v1/generation/tasks/{TASK} ")){assert!(!lower.contains("x-account-group-id:"));return(200,task_response(&url,&bytes));}
            if line.starts_with("GET /output.png "){assert!(!lower.contains("x-token:"));assert!(!lower.contains("x-account-group-id:"));return(200,bytes.clone());}
            if line.starts_with(&format!("POST /v1/generation/tasks/{TASK}/deliveries/{FILE}/ack ")){
                assert!(!lower.contains("x-account-group-id:"));
                let data=writer.load_client_state_for_namespace(&lease).unwrap().unwrap();
                assert!(data.assets.iter().any(|asset|asset.id==FILE && asset.origin=="image_enhancement" && asset.upscale_done),
                    "remote ack preceded actual owned SQLite metadata");
                acks.fetch_add(1,Ordering::Release);return(200,envelope(serde_json::json!({})));
            }
            panic!("unexpected enhancement route");
        });
    }
    #[test]
    fn core_enhancement_input_is_owned_preview_and_replacement_blocks_actual_submit(){
        let(f,app)=fixture(None);publish_group(&f,PAYER);let source=select_owned_input(&f,&app);
        assert!(Path::new(&source).starts_with(f.persistence.lease().namespace.root()));assert!(app.global::<AppState>().get_enhance_source_image().size().width>0);
        std::fs::write(&source,b"replaced original owned image").unwrap();
        app.global::<AppState>().invoke_start_enhance("2K".into());drain_enhancement_test_workers();pump_for(Duration::from_millis(80));
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
        assert!(!app.global::<AppState>().get_enhance_processing());assert!(f.context.store.borrow().assets.is_empty());
    }
    #[test]
    fn core_enhancement_url_success_preserves_format_size_and_dimensions_policy(){
        let transport=Scenario::new();let(f,app)=fixture(None);let mut server=transport;
        server.start(|_,_|(200,png()));
        assert!(add_enhancement_from_drag_data_for_store(&app,TEXT_PLAIN_MIME,&format!("{}input.png",server.url),&f.context.store.borrow()));
        pump_until(||!app.global::<AppState>().get_enhance_source_path().is_empty());
        let source=app.global::<AppState>().get_enhance_source_path().to_string();
        assert!(Path::new(&source).starts_with(f.persistence.lease().namespace.root()));
        assert_eq!(app.global::<AppState>().get_enhance_source_image().size().width,80);
        assert_eq!(app.global::<AppState>().get_enhance_quality(),"2K");server.finish();
        let external=tempfile::tempdir().unwrap();let path=external.path().join("too-small.png");
        let image=image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(63,80,image::Rgba([0,0,0,255])));
        image.save_with_format(&path,image::ImageFormat::Png).unwrap();
        assert!(add_enhancement_paths_for_store(&app,vec![path],&f.context.store.borrow()));
        drain_enhancement_test_workers();pump_for(Duration::from_millis(80));
        assert_eq!(app.global::<AppState>().get_enhance_source_path(),source);assert!(app.global::<AppState>().get_enhance_message().contains("64"));
        for (name,width,height,format) in [
            ("unsupported.bmp",80,80,image::ImageFormat::Bmp),
            ("too-wide.png",161,80,image::ImageFormat::Png),
            ("long-edge.png",5001,64,image::ImageFormat::Png),
        ]{
            let path=external.path().join(name);
            let image=image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(width,height,image::Rgba([0,0,0,255])));
            image.save_with_format(&path,format).unwrap();
            assert!(add_enhancement_paths_for_store(&app,vec![path],&f.context.store.borrow()));
            drain_enhancement_test_workers();pump_for(Duration::from_millis(80));
            assert_eq!(app.global::<AppState>().get_enhance_source_path(),source);
        }
        let oversized=external.path().join("oversized.png");let mut oversized_bytes=png();
        oversized_bytes.resize((ENHANCEMENT_MAX_INPUT_BYTES+1) as usize,0);std::fs::write(&oversized,oversized_bytes).unwrap();
        assert!(add_enhancement_paths_for_store(&app,vec![oversized],&f.context.store.borrow()));
        drain_enhancement_test_workers();pump_for(Duration::from_millis(80));
        assert_eq!(app.global::<AppState>().get_enhance_source_path(),source);
        assert!(app.global::<AppState>().get_enhance_message().contains("20"));
    }
    #[test]
    fn core_enhancement_retained_delivery_is_other_only_and_ack_follows_sqlite(){
        let transport=Scenario::new();let(f,app)=fixture(Some(&transport.url));let mut server=transport;
        let source=select_owned_input(&f,&app);let record=retained(&f,&source);
        let acks=Arc::new(std::sync::atomic::AtomicUsize::new(0));serve_delivery(&mut server,&f,acks.clone());
        resume_pending_image_enhancement(&app,f.context.clone(),record);
        pump_until(||app.global::<AppState>().get_enhance_progress()==100);server.finish();
        assert_eq!(acks.load(Ordering::Acquire),1);let store=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(store.assets.len(),1);assert!(store.generations.is_empty());let asset=&store.assets[0];
        assert_eq!(asset.id,FILE);assert_eq!(asset.category,"other");assert_eq!(asset.origin,"image_enhancement");assert_eq!(asset.model,"图片清晰");
        assert!(asset.upscale_done);assert!(!f.context.store.borrow().assets.iter().find(|asset|asset.id==FILE).unwrap().is_new);
        assert_eq!(asset.quality,"1K");assert_eq!(asset.prompt,"图片清晰增强");
        assert_eq!(asset.reference_paths,vec![source]);assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
        assert!(Path::new(&asset.source_path).starts_with(f.persistence.lease().namespace.output_dir()));
    }
    #[test]
    fn core_enhancement_failed_store_ack_retry_preserves_later_edit_and_single_asset(){
        let transport=Scenario::new();let(f,app)=fixture(Some(&transport.url));let mut server=transport;
        let source=select_owned_input(&f,&app);let record=retained(&f,&source);
        let root=f.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let sql=rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        sql.execute_batch("CREATE TRIGGER reject_enhancement_save BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'controlled save failure'); END;").unwrap();
        let acks=Arc::new(std::sync::atomic::AtomicUsize::new(0));serve_delivery(&mut server,&f,acks.clone());
        resume_pending_image_enhancement(&app,f.context.clone(),record.clone());
        pump_until(||app.global::<AppState>().get_enhance_message().contains("保存未确认"));
        assert_eq!(acks.load(Ordering::Acquire),0);assert_eq!(f.context.store.borrow().assets.len(),1);
        assert!(f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap().assets.is_empty());
        f.context.store.borrow_mut().assets[0].title="edited while save was unconfirmed".into();
        sql.execute_batch("DROP TRIGGER reject_enhancement_save").unwrap();
        resume_pending_image_enhancement(&app,f.context.clone(),record);
        pump_until(||app.global::<AppState>().get_enhance_progress()==100);server.finish();
        let data=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(data.assets.len(),1);assert_eq!(data.assets[0].title,"edited while save was unconfirmed");assert_eq!(acks.load(Ordering::Acquire),1);
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
    }
    #[test]
    fn core_enhancement_actual_new_then_retry_retains_key_quality_and_original_payer(){
        let transport=Scenario::new();let(f,app)=fixture(Some(&transport.url));let mut server=transport;
        let _source=select_owned_input(&f,&app);publish_group(&f,PAYER);
        let authority=f.authority.clone();let url=server.url.clone();
        server.start(move|headers,_|{
            let line=headers.lines().next().unwrap();let lower=headers.to_ascii_lowercase();
            let rows=load_pending_generations_for_namespace(&authority).unwrap();assert_eq!(rows.len(),1,"upload/POST preceded original durable row");assert_eq!(rows[0].billing_account_group_id,PAYER);
            if line.starts_with("POST /v1/uploads/references "){
                assert!(!lower.contains("x-account-group-id:"));
                return(200,envelope(serde_json::json!({"file":{"id":INPUT},"upload":{"method":"POST","url":format!("{url}upload"),"fields":{},"file_field":"file"}})));
            }
            if line.starts_with("POST /upload "){assert!(!lower.contains("x-token:"));return(200,Vec::new());}
            if line.starts_with(&format!("POST /v1/uploads/references/{INPUT}/complete ")){return(200,envelope(serde_json::json!({})));}
            assert!(line.starts_with("POST /v1/toolbox/image-enhancements "));assert!(lower.contains(&format!("x-account-group-id: {PAYER}")));
            (403,serde_json::to_vec(&serde_json::json!({"request_id":"private","data":null,"error":{"code":"account_group_not_selectable","message":"private error","details":null},"meta":null})).unwrap())
        });
        app.global::<AppState>().invoke_start_enhance("2K".into());
        pump_until(||!app.global::<AppState>().get_enhance_processing());
        let first=load_pending_generations_for_namespace(&f.authority).unwrap()[0].clone();
        publish_group(&f,TASK);app.global::<AppState>().invoke_start_enhance("4K".into());
        pump_until(||!app.global::<AppState>().get_enhance_processing());server.finish();
        let after=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(after.len(),1);
        assert_eq!(after[0].client_request_id,first.client_request_id);assert_eq!(after[0].quality,"2K");assert_eq!(after[0].billing_account_group_id,PAYER);
        let requests=server.requests.lock().unwrap();let creates=requests.iter().filter(|(head,_)|head.starts_with("POST /v1/toolbox/image-enhancements ")).collect::<Vec<_>>();
        assert_eq!(creates.len(),2);assert_eq!(creates[0].1,creates[1].1);
        assert!(requests.iter().filter(|(head,_)|head.starts_with("POST /v1/uploads/references ")).count()==1);
    }
    struct HeldEnhancement{ready:mpsc::Receiver<()>,release:Option<mpsc::Sender<()>>}
    impl HeldEnhancement{
        fn install()->Self{
            let(sent,ready)=mpsc::channel();let(release,wait)=mpsc::channel();
            ENHANCEMENT_TEST_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{let _=sent.send(());let _=wait.recv_timeout(Duration::from_secs(3));})));
            Self{ready,release:Some(release)}
        }
        fn release(&mut self){if let Some(release)=self.release.take(){let _=release.send(());}}
    }
    impl Drop for HeldEnhancement{fn drop(&mut self){self.release();}}
    #[test]
    fn core_enhancement_sent_result_waits_for_real_worker_exit_without_blocking_ui(){
        let(f,app)=fixture(None);let path=owned(&f);let mut held=HeldEnhancement::install();
        assert!(add_enhancement_paths_for_store(&app,vec![path],&f.context.store.borrow()));held.ready.recv_timeout(Duration::from_secs(3)).unwrap();
        let advanced=Rc::new(Cell::new(false));let seen=advanced.clone();
        slint::Timer::single_shot(Duration::ZERO,move||seen.set(true));pump_for(Duration::from_millis(80));
        assert!(advanced.get());assert!(app.global::<AppState>().get_enhance_source_path().is_empty());
        held.release();drain_enhancement_test_workers();pump_until(||!app.global::<AppState>().get_enhance_source_path().is_empty());
    }

    #[test]
    fn core_enhancement_closed_window_cancels_without_joining_live_worker(){
        let(f,app)=fixture(None);let path=owned(&f);let mut held=HeldEnhancement::install();
        assert!(add_enhancement_paths_for_store(&app,vec![path],&f.context.store.borrow()));
        held.ready.recv_timeout(Duration::from_secs(3)).unwrap();drop(app);
        let advanced=Rc::new(Cell::new(false));let observed=advanced.clone();
        slint::Timer::single_shot(Duration::ZERO,move||observed.set(true));
        pump_for(Duration::from_millis(80));
        assert!(advanced.get());assert!(ENHANCEMENT_WORKERS.with(|workers|workers.borrow().iter().all(|worker|worker.cancel.load(Ordering::Acquire))));
        assert_eq!(ENHANCEMENT_WORKERS.with(|workers|workers.borrow().len()),1);
        held.release();drain_enhancement_test_workers();pump_for(Duration::from_millis(80));
        assert!(ENHANCEMENT_WORKERS.with(|workers|workers.borrow().is_empty()));
        assert!(f.context.store.borrow().assets.is_empty());
    }
    #[test]
    fn core_enhancement_real_worker_panic_stays_sticky_and_closes_admission(){
        let(mut f,app)=fixture(None);f.expected_failure=true;
        let capture=enhancement_current_capture(&app,f.context.clone()).unwrap();
        spawn_enhancement_work::<()>(&app,capture,false,|_,_,_|panic!("controlled enhancement worker panic"),|_,_,_|panic!("panic published"));
        pump_until(||ENHANCEMENT_FAILED.with(Cell::get));
        assert!(ENHANCEMENT_CLOSED.with(Cell::get));assert!(shutdown_enhancement_workers().is_err());assert!(join_enhancement_workers().is_err());
        let calls=Rc::new(Cell::new(0));let observed=calls.clone();
        ENHANCEMENT_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|_|observed.set(observed.get()+1))));
        app.global::<AppState>().invoke_choose_enhance_source();assert_eq!(calls.get(),0);
    }
    #[test]
    fn core_enhancement_undelivered_actual_timer_drops_after_ui_thread_tls(){
        std::thread::spawn(||{
            let(f,app)=fixture(None);let path=owned(&f);
            assert!(add_enhancement_paths_for_store(&app,vec![path],&f.context.store.borrow()));
            drain_enhancement_test_workers();assert!(app.global::<AppState>().get_enhance_source_path().is_empty());
            drop(app);drop(f);
        }).join().unwrap();
    }


    #[test]
    fn core_enhancement_verified_empty_terminal_releases_only_safe_record(){
        let transport=Scenario::new();let(f,app)=fixture(Some(&transport.url));let mut server=transport;
        let source=select_owned_input(&f,&app);let mode=Arc::new(std::sync::atomic::AtomicUsize::new(0));let response_mode=mode.clone();
        server.start(move|headers,_|{
            assert!(headers.starts_with(&format!("GET /v1/generation/tasks/{TASK} ")));
            assert!(!headers.to_ascii_lowercase().contains("x-account-group-id:"));
            if response_mode.load(Ordering::Acquire)==1{return(403,serde_json::to_vec(&serde_json::json!({
                "request_id":"private","data":null,"error":{"code":"account_group_not_selectable","message":"private","details":null},"meta":null
            })).unwrap());}
            (200,envelope(serde_json::json!({
                "id":TASK,"billing_account_group_id":PAYER,"status":"failed","progress_percent":100,
                "success_count":if response_mode.load(Ordering::Acquire)==2{1}else{0},"failure_count":1,
                "failure":null,"prompt":null,"result_prompt":null,"request":{},"model":null,
                "quality":"2K","requested_count":1,"type":"image_enhancement","items":[]
            })))
        });
        let first=retained(&f,&source);resume_pending_image_enhancement(&app,f.context.clone(),first);
        pump_until(||!app.global::<AppState>().get_enhance_processing());
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
        assert!(app.global::<AppState>().get_enhance_message().contains("没有增强结果"));
        mode.store(1,Ordering::Release);let record=retained(&f,&source);
        resume_pending_image_enhancement(&app,f.context.clone(),record.clone());
        pump_until(||!app.global::<AppState>().get_enhance_processing());
        assert_eq!(load_pending_generations_for_namespace(&f.authority).unwrap()[0].client_request_id,record.client_request_id);
        mode.store(2,Ordering::Release);
        resume_pending_image_enhancement(&app,f.context.clone(),record.clone());
        pump_until(||!app.global::<AppState>().get_enhance_processing());
        assert_eq!(load_pending_generations_for_namespace(&f.authority).unwrap().len(),1);
        mode.store(3,Ordering::Release);
        let mut pending=load_pending_generations_for_namespace(&f.authority).unwrap().remove(0);
        pending.deliveries.push(PendingDeliveryRecord{file_id:FILE.into(),sha256:"a".repeat(64),size_bytes:1,..Default::default()});
        let scope=BillingScope{request:GroupRequestScope{session:f.context.current_account_session_scope().unwrap(),account_group_id:PAYER.into()},context_epoch:1};
        upsert_pending_generation_for_namespace(&f.authority,&scope,pending.clone()).unwrap();
        resume_pending_image_enhancement(&app,f.context.clone(),pending);
        pump_until(||!app.global::<AppState>().get_enhance_processing());server.finish();
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(rows.len(),1);
        assert_eq!(rows[0].client_request_id,record.client_request_id);assert_eq!(rows[0].deliveries.len(),1);
        assert!(!rows[0].terminal);assert!(f.context.store.borrow().assets.is_empty());
    }
    struct JoinedEnhancementTrip(Option<std::thread::JoinHandle<()>>);
    impl JoinedEnhancementTrip{
        fn start(latch:UpgradeLatch)->Self{Self(Some(std::thread::spawn(move||{
            latch.trip(RequiredUpgrade{minimum_version:Some("99.0.0".into())});
        })))}
        fn finish(&mut self){if let Some(worker)=self.0.take(){worker.join().unwrap();}}
    }
    impl Drop for JoinedEnhancementTrip{fn drop(&mut self){
        if let Some(worker)=self.0.take(){let result=worker.join();if !std::thread::panicking(){result.unwrap();}}
    }}
    #[test]
    fn core_enhancement_held_http_upgrade_rejects_late_error_and_joins(){
        let mut transport=Http::new(500);let(f,app)=fixture(None);
        assert!(add_enhancement_from_drag_data_for_store(&app,TEXT_PLAIN_MIME,&format!("{}input.png",transport.url),&f.context.store.borrow()));
        transport.seen.recv_timeout(Duration::from_secs(3)).unwrap();
        app.global::<AppState>().set_enhance_message("protected after upgrade".into());
        let latch=f.persistence.upgrade_latch().clone();let mut trip=JoinedEnhancementTrip::start(latch.clone());
        let end=Instant::now()+Duration::from_secs(2);
        while !latch.is_tripped() && Instant::now()<end{std::thread::sleep(Duration::from_millis(2));}
        // Release before an assertion can unwind through the trip's join.
        let tripped=latch.is_tripped();transport.release();trip.finish();assert!(tripped);
        drain_enhancement_test_workers();pump_for(Duration::from_millis(80));transport.finish();
        assert_eq!(app.global::<AppState>().get_enhance_message(),"protected after upgrade");
        assert!(app.global::<AppState>().get_enhance_source_path().is_empty());assert!(f.context.store.borrow().assets.is_empty());
        assert!(ENHANCEMENT_WORKERS.with(|workers|workers.borrow().is_empty()));
    }
    #[test]
    fn core_enhancement_repeated_recovery_cannot_steal_live_request_reservation(){
        let mut transport=Http::new(403);let(f,app)=fixture(Some(&transport.url));let source=owned(&f).to_string_lossy().to_string();
        let record=retained(&f,&source);
        resume_pending_image_enhancement(&app,f.context.clone(),record.clone());
        transport.seen.recv_timeout(Duration::from_secs(3)).unwrap();
        let request=ENHANCEMENT_UI.with(|ui|ui.borrow().request);
        resume_pending_image_enhancement(&app,f.context.clone(),record.clone());
        assert_eq!(ENHANCEMENT_UI.with(|ui|ui.borrow().request),request);
        assert_eq!(ENHANCEMENT_WORKERS.with(|workers|workers.borrow().len()),1);
        transport.release();drain_enhancement_test_workers();pump_for(Duration::from_millis(80));transport.finish();
        assert!(!app.global::<AppState>().get_enhance_processing());
        assert_eq!(load_pending_generations_for_namespace(&f.authority).unwrap()[0].client_request_id,record.client_request_id);
    }


    // Real indexed PNG: the source stores one palette index per pixel, while
    // normalization decodes transparency and persists full RGBA PNG pixels.
    // The source/normalized size assertions are fixture preconditions, not RED.
    fn indexed_expansion_png()->Vec<u8>{
        fn chunk(output:&mut Vec<u8>,kind:&[u8;4],data:&[u8]){
            output.extend_from_slice(&(data.len() as u32).to_be_bytes());
            output.extend_from_slice(kind);output.extend_from_slice(data);
            let mut table=[0u32;256];
            for (index,entry) in table.iter_mut().enumerate(){
                let mut crc=index as u32;for _ in 0..8{crc=if crc&1!=0{0xedb88320^(crc>>1)}else{crc>>1};}*entry=crc;
            }
            let mut crc=!0u32;
            for &byte in kind.iter().chain(data){crc=table[((crc^u32::from(byte))&255) as usize]^(crc>>8);}
            output.extend_from_slice(&(!crc).to_be_bytes());
        }
        let mut seed=0x7865_21adu32;
        let mut next=||{seed^=seed<<13;seed^=seed>>17;seed^=seed<<5;seed as u8};
        let mut palette=Vec::with_capacity(768);let mut alpha=Vec::with_capacity(256);
        for _ in 0..256{palette.extend_from_slice(&[next(),next(),next()]);alpha.push(next());}
        let edge=4500u32;let mut scanlines=Vec::with_capacity(((edge+1)*edge) as usize);
        let(mut a,mut b)=(1u32,0u32);
        for _ in 0..edge{
            scanlines.push(0);b=(b+a)%65521; // PNG filter byte participates in Adler32.
            for _ in 0..edge{
                let byte=next();scanlines.push(byte);a=(a+u32::from(byte))%65521;b=(b+a)%65521;
            }
        }
        let mut compressed=Vec::with_capacity(scanlines.len()+2048);compressed.extend_from_slice(&[0x78,0x01]);
        let count=(scanlines.len()+65534)/65535;
        for (index,block) in scanlines.chunks(65535).enumerate(){
            compressed.push(u8::from(index+1==count));let size=block.len() as u16;
            compressed.extend_from_slice(&size.to_le_bytes());compressed.extend_from_slice(&(!size).to_le_bytes());
            compressed.extend_from_slice(block);
        }
        compressed.extend_from_slice(&((b<<16)|a).to_be_bytes());
        let mut output=b"\x89PNG\r\n\x1a\n".to_vec();let mut header=Vec::new();
        header.extend_from_slice(&edge.to_be_bytes());header.extend_from_slice(&edge.to_be_bytes());
        header.extend_from_slice(&[8,3,0,0,0]);
        chunk(&mut output,b"IHDR",&header);chunk(&mut output,b"PLTE",&palette);
        chunk(&mut output,b"tRNS",&alpha);chunk(&mut output,b"IDAT",&compressed);chunk(&mut output,b"IEND",&[]);
        output
    }
    #[test]
    fn core_enhancement_accepted_source_may_expand_beyond_external_limit_before_submit(){
        let transport=Scenario::new();let(f,app)=fixture(Some(&transport.url));let mut server=transport;
        let external=tempfile::tempdir().unwrap();let path=external.path().join("indexed-alpha.png");
        let bytes=indexed_expansion_png();
        assert!(bytes.len() as u64<=20*1024*1024,"fixture must begin within the external input policy");
        std::fs::write(&path,&bytes).unwrap();drop(bytes);publish_group(&f,PAYER);
        ENHANCEMENT_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|done|done(Some(path)))));
        app.global::<AppState>().invoke_choose_enhance_source();
        // This one large real codec fixture has its own bounded CPU deadline.
        // Ordinary fixture pumps, API and business timeouts remain unchanged.
        let deadline=Instant::now()+Duration::from_secs(120);
        while app.global::<AppState>().get_enhance_source_path().is_empty() && Instant::now()<deadline{
            pump_for(Duration::from_millis(10));
        }
        let source=app.global::<AppState>().get_enhance_source_path().to_string();
        assert!(!source.is_empty(),"large source import did not complete within fixture deadline");
        drain_enhancement_test_workers();
        let owned=f.authority.read_image_source(Path::new(&source),100*1024*1024).unwrap();
        let size=owned.len() as u64;let sha=sha256_hex(&owned);drop(owned);
        assert!(size>20*1024*1024 && size<=100*1024*1024,"fixture must actually expand beyond external limit");
        let authority=f.authority.clone();
        server.start_for(Duration::from_secs(120),move|headers,body|{
            assert!(headers.starts_with("POST /v1/uploads/references "));
            assert!(!headers.to_ascii_lowercase().contains("x-account-group-id:"));
            let payload:serde_json::Value=serde_json::from_slice(body).unwrap();
            assert!(payload.get("sha256").and_then(|value|value.as_str()).is_some());
            let rows=load_pending_generations_for_namespace(&authority).unwrap();
            assert_eq!(rows.len(),1,"upload preparation preceded durable request");
            assert_eq!(rows[0].billing_account_group_id,PAYER);
            (403,serde_json::to_vec(&serde_json::json!({"request_id":"fixture","data":null,
                "error":{"code":"account_group_not_selectable","message":"private","details":null},"meta":null})).unwrap())
        });
        app.global::<AppState>().invoke_start_enhance("2K".into());
        let deadline=Instant::now()+Duration::from_secs(120);
        while app.global::<AppState>().get_enhance_processing() && Instant::now()<deadline{pump_for(Duration::from_millis(10));}
        assert!(!app.global::<AppState>().get_enhance_processing(),"submit did not finish within fixture deadline");
        drain_enhancement_test_workers();server.finish();
        assert_eq!(server.requests.lock().unwrap().len(),1,"valid external input was rejected only because internal encoding grew");
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(rows.len(),1);
        assert_eq!(rows[0].reference_size_bytes,vec![size]);assert_eq!(rows[0].reference_sha256,vec![sha]);
        assert_eq!(rows[0].reference_paths,vec![source]);assert_eq!(rows[0].quality,"2K");
        assert!(rows[0].uploaded_file_ids.is_empty());assert!(rows[0].server_task_id.is_empty());
    }

}
