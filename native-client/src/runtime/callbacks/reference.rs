use super::*;
use crate::platform::{self,ExternalDropPosition,ExternalImageDrop};
use std::io::Read;

const MAX_DROPPED_IMAGE_BYTES:u64=100*1024*1024;
const NATIVE_DRAG_POLL_INTERVAL:Duration=Duration::from_millis(1);
type ReferencePickerCompletion=Box<dyn FnOnce(Vec<PathBuf>)>;
#[cfg(test)]
thread_local!{
    static REFERENCE_TEST_PICKER:RefCell<Option<Box<dyn FnOnce(ReferencePickerCompletion)>>>=const{RefCell::new(None)};
    static REFERENCE_TEST_CLIPBOARD:RefCell<Option<Box<dyn FnOnce()->Option<arboard::ImageData<'static>>>>>=const{RefCell::new(None)};
    static REFERENCE_TEST_AFTER_SEND:RefCell<Option<Box<dyn FnOnce()+Send>>>=const{RefCell::new(None)};
}
fn reference_pick_files(completion:ReferencePickerCompletion){
    #[cfg(test)]
    if let Some(picker)=REFERENCE_TEST_PICKER.with(|hook|hook.borrow_mut().take()){picker(completion);return;}
    let _=slint::spawn_local(async move{
        if let Some(files)=rfd::AsyncFileDialog::new().add_filter("Images",crate::image_formats::picker_image_extensions()).pick_files().await{
            completion(files.into_iter().map(|file|file.path().to_path_buf()).collect());
        }
    });
}
fn reference_clipboard_image()->Option<arboard::ImageData<'static>>{
    #[cfg(test)]
    if let Some(read)=REFERENCE_TEST_CLIPBOARD.with(|hook|hook.borrow_mut().take()){return read();}
    let mut clipboard=arboard::Clipboard::new().ok()?;let image=clipboard.get_image().ok()?;
    Some(arboard::ImageData{width:image.width,height:image.height,bytes:std::borrow::Cow::Owned(image.bytes.into_owned())})
}
#[derive(Clone,Eq,PartialEq)]
enum ReferenceTarget{Category(String),Canvas(String),NativePage(String),NativeViewer{page:String,id:String,source:String,path:String}}
impl ReferenceTarget{
    fn capture(app:&AppWindow,context:&AppContext)->Option<Self>{
        let state=app.global::<AppState>();
        match state.get_page().as_str(){
            "generation"=>Some(Self::Category(resolve_category(state.get_asset_type().as_str(),""))),
            "canvas"=>Some(Self::Canvas(context.store.borrow().active_canvas_workspace_id.clone())),
            _=>None,
        }
    }
    fn rows<'a>(&self,store:&'a Store)->&'a Vec<ReferenceData>{match self{
        Self::Category(category)=>references_for_category(&store.references,category),Self::Canvas(_)=>&store.canvas_references,Self::NativePage(_)|Self::NativeViewer{..}=>&store.canvas_references,
    }}
    fn rows_mut<'a>(&self,store:&'a mut Store)->&'a mut Vec<ReferenceData>{match self{
        Self::Category(category)=>references_for_category_mut(&mut store.references,category),Self::Canvas(_)=>&mut store.canvas_references,Self::NativePage(_)|Self::NativeViewer{..}=>&mut store.canvas_references,
    }}
    fn limit(&self)->usize{match self{Self::Category(category)=>max_reference_images_for_category(category),Self::Canvas(_)|Self::NativePage(_)|Self::NativeViewer{..}=>MAX_REFERENCE_IMAGES}}
}
#[derive(Clone)]
struct ReferenceCapture{persistence:PrivatePersistence,scope:SessionScope,target:ReferenceTarget,require_target:bool}
impl ReferenceCapture{
    fn new(app:&AppWindow,context:&AppContext)->Option<Self>{
        let persistence=context.store.borrow().private_persistence.clone()?;
        let scope=context.current_account_session_scope()?;
        let capture=Self{persistence,scope,target:ReferenceTarget::capture(app,context)?,require_target:true};
        capture.current(app,context).then_some(capture)
    }
    fn native(app:&AppWindow,context:&AppContext)->Option<Self>{
        let persistence=context.store.borrow().private_persistence.clone()?;
        let scope=context.current_account_session_scope()?;
        let state=app.global::<AppState>();
        let page=state.get_page().to_string();
        let target=if state.get_viewer_open(){
            ReferenceTarget::NativeViewer{page,id:state.get_viewer_id().to_string(),source:state.get_viewer_source().to_string(),path:state.get_viewer_source_path().to_string()}
        }else{ReferenceTarget::NativePage(page)};
        let captured=Self{persistence,scope,target,require_target:true};
        captured.current(app,context).then_some(captured)
    }
    fn target_matches(&self,app:&AppWindow,context:&AppContext)->bool{match &self.target{
        ReferenceTarget::NativePage(page)=>app.global::<AppState>().get_page()==*page && !app.global::<AppState>().get_viewer_open(),
        ReferenceTarget::NativeViewer{page,id,source,path}=>{
            let state=app.global::<AppState>();state.get_viewer_open() && state.get_page()==*page
                && state.get_viewer_id()==*id && state.get_viewer_source()==*source && state.get_viewer_source_path()==*path
        },
        _=>ReferenceTarget::capture(app,context).as_ref()==Some(&self.target),
    }}
    fn binding_matches(&self,context:&AppContext)->bool{
        context.store.borrow().private_persistence.as_ref().is_some_and(|value|value.lease()==self.persistence.lease())
            && context.active_namespace.lock().ok().is_some_and(|active|active.as_ref()==Some(self.persistence.lease()))
            && self.persistence.lease().auth_epoch==self.scope.auth_epoch
            && self.persistence.lease().namespace.user_public_id()==self.scope.owner_user_id
    }
    fn current(&self,app:&AppWindow,context:&AppContext)->bool{
        !REFERENCE_CLOSED.with(|closed|closed.get()) && self.binding_matches(context) && self.persistence.is_current()
            && context.backend.as_ref().is_some_and(|backend|backend.api.session().is_scope_current(&self.scope))
            && (!self.require_target || self.target_matches(app,context))
    }
    fn apply<R>(&self,app:&AppWindow,context:&AppContext,apply:impl FnOnce()->R)->Option<R>{
        if !self.current(app,context){return None;}
        context.apply_user_completion(self.persistence.lease(),||{
            if !self.binding_matches(context) || (self.require_target && !self.target_matches(app,context)){return None;}
            Some(apply())
        }).ok().flatten()
    }
    fn status(&self,app:&AppWindow,context:&AppContext,message:&str){
        self.apply(app,context,||app.global::<AppState>().set_generation_status(message.into()));
    }
}
struct ReferenceWorker{id:Uuid,lease:NamespaceLease,cancel:Arc<std::sync::atomic::AtomicBool>,handle:std::thread::JoinHandle<()>}
#[derive(Clone)]
struct ReferenceSaveState{lease:NamespaceLease,target:ReferenceTarget,revision:Uuid,retry:bool}
#[derive(Default)]
struct ReferenceUiState{saves:Vec<ReferenceSaveState>,viewer:Option<Uuid>,edits:Vec<(NamespaceLease,ReferenceTarget,Uuid)>,native:Option<ReferenceNativeRequest>}
struct ReferenceNativeRequest{
    id:Uuid,lease:NamespaceLease,target:ReferenceTarget,path:PathBuf,
    preview_unclaimed:bool,cancel:Arc<std::sync::atomic::AtomicBool>,
}
struct PreparedLegacyNativeDrag{
    source:PreparedNativeFileDragSource,path:PathBuf,file_id:ManagedFileId,
}
impl Drop for ReferenceNativeRequest{
    fn drop(&mut self){self.cancel.store(true,Ordering::Release);}
}
#[derive(Clone)]
struct ReferenceNativeTicket{ui:Rc<RefCell<ReferenceUiState>>,id:Uuid,cancel:Arc<std::sync::atomic::AtomicBool>}
impl ReferenceNativeTicket{
    fn current(&self)->bool{
        !self.cancel.load(Ordering::Acquire)
            && self.ui.borrow().native.as_ref().is_some_and(|request|request.id==self.id)
    }
}
fn begin_reference_native_request(app:&AppWindow,context:&AppContext,capture:&ReferenceCapture,path:&Path,preview:bool)->Option<ReferenceNativeTicket>{
    let ui=REFERENCE_UI.with(Clone::clone);
    let ticket=capture.apply(app,context,||{
        let mut state=ui.borrow_mut();
        // thumbnail-card calls preview then file drag synchronously for one gesture.
        // Only that first matching file call in this UI turn may claim the preview.
        if !preview{
            if let Some(request)=state.native.as_mut().filter(|request|request.preview_unclaimed
                && request.lease==*capture.persistence.lease() && request.target==capture.target
                && request.path==path && !request.cancel.load(Ordering::Acquire)){
                request.preview_unclaimed=false;
                return ReferenceNativeTicket{ui:ui.clone(),id:request.id,cancel:request.cancel.clone()};
            }
        }
        let id=Uuid::new_v4();let cancel=Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Replacing the owned slot cancels the old worker token; Drop never consults TLS.
        state.native=Some(ReferenceNativeRequest{id,lease:capture.persistence.lease().clone(),
            target:capture.target.clone(),path:path.to_path_buf(),preview_unclaimed:preview,cancel:cancel.clone()});
        ReferenceNativeTicket{ui:ui.clone(),id,cancel}
    })?;
    if preview{
        let paired=ticket.clone();
        slint::Timer::single_shot(Duration::ZERO,move||{
            if let Some(request)=paired.ui.borrow_mut().native.as_mut().filter(|request|request.id==paired.id){
                request.preview_unclaimed=false;
            }
        });
    }
    Some(ticket)
}
thread_local!{
    static REFERENCE_WORKERS:RefCell<Vec<ReferenceWorker>>=const{RefCell::new(Vec::new())};
    static REFERENCE_JOIN_FAILED:Cell<bool>=const{Cell::new(false)};
    static REFERENCE_CLOSED:Cell<bool>=const{Cell::new(false)};
    static REFERENCE_UI:Rc<RefCell<ReferenceUiState>>=Rc::new(RefCell::new(ReferenceUiState::default()));
}
fn reference_worker_failed(){
    REFERENCE_JOIN_FAILED.with(|failed|failed.set(true));REFERENCE_CLOSED.with(|closed|closed.set(true));
    cancel_reference_workers_for_upgrade();
}
fn reap_reference_workers(){
    let ready=REFERENCE_WORKERS.with(|workers|{
        let mut workers=workers.borrow_mut();let mut ready=Vec::new();let mut index=0;
        while index<workers.len(){if workers[index].handle.is_finished(){ready.push(workers.remove(index));}else{index+=1;}}ready
    });
    for worker in ready{if worker.handle.join().is_err(){reference_worker_failed();}}
}
fn reference_worker_pending(id:Uuid)->bool{REFERENCE_WORKERS.with(|workers|workers.borrow().iter().any(|worker|worker.id==id))}
fn join_reference_workers()->Result<()>{
    let mut workers=REFERENCE_WORKERS.with(|workers|std::mem::take(&mut *workers.borrow_mut())).into_iter();
    while let Some(worker)=workers.next(){if worker.handle.join().is_err(){
        reference_worker_failed();for pending in workers.as_slice(){pending.cancel.store(true,Ordering::Release);}
    }}
    anyhow::ensure!(!REFERENCE_JOIN_FAILED.with(|failed|failed.get()),"reference worker panicked");Ok(())
}
pub(super) fn cancel_reference_workers_for_retirement(lease:&NamespaceLease){
    REFERENCE_WORKERS.with(|workers|for worker in workers.borrow().iter().filter(|worker|&worker.lease==lease){worker.cancel.store(true,Ordering::Release);});
}
pub(super) fn cancel_reference_workers_for_upgrade(){
    REFERENCE_WORKERS.with(|workers|for worker in workers.borrow().iter(){worker.cancel.store(true,Ordering::Release);});
}
/// Owning UI thread, after event-loop exit and outside ordinary/short guards.
pub(super) fn shutdown_reference_workers()->Result<()>{
    REFERENCE_CLOSED.with(|closed|closed.set(true));cancel_reference_workers_for_upgrade();join_reference_workers()
}
#[cfg(test)]
fn drain_reference_test_workers(){let result=join_reference_workers();if !std::thread::panicking(){result.unwrap();}}
struct ReferenceJob<R>{id:Uuid,cancel:Arc<std::sync::atomic::AtomicBool>,receiver:mpsc::Receiver<Result<R>>}
impl<R> Drop for ReferenceJob<R>{fn drop(&mut self){self.cancel.store(true,Ordering::Release);}}
fn reference_orphan_poll(id:Uuid){
    slint::Timer::single_shot(Duration::from_millis(40),move||{reap_reference_workers();if reference_worker_pending(id){reference_orphan_poll(id);}});
}
fn spawn_reference_work<R:Send+'static>(
    app:&AppWindow,context:AppContext,capture:ReferenceCapture,
    work:impl FnOnce(&PrivatePersistence,&Arc<std::sync::atomic::AtomicBool>,&UserActivityPermit)->Result<R>+Send+'static,
    complete:impl FnOnce(&AppWindow,&AppContext,&ReferenceCapture,Result<R>)+'static,
)->bool{
    spawn_reference_work_with_poll(app,context,capture,Duration::from_millis(40),work,complete)
}
fn spawn_reference_work_with_poll<R:Send+'static>(
    app:&AppWindow,context:AppContext,capture:ReferenceCapture,poll_interval:Duration,
    work:impl FnOnce(&PrivatePersistence,&Arc<std::sync::atomic::AtomicBool>,&UserActivityPermit)->Result<R>+Send+'static,
    complete:impl FnOnce(&AppWindow,&AppContext,&ReferenceCapture,Result<R>)+'static,
)->bool{
    if !capture.current(app,&context){return false;}
    let Ok(activity)=capture.persistence.begin_activity()else{return false;};
    let persistence=capture.persistence.clone();let cancel=Arc::new(std::sync::atomic::AtomicBool::new(false));let worker_cancel=cancel.clone();
    let id=Uuid::new_v4();let(sender,receiver)=mpsc::channel();
    #[cfg(test)]
    let after_send=REFERENCE_TEST_AFTER_SEND.with(|hook|hook.borrow_mut().take());
    let worker=std::thread::Builder::new().name("reference-owned-work".into()).spawn(move||{
        let result=if activity.is_quiescing() || worker_cancel.load(Ordering::Acquire) || !persistence.is_current(){Err(anyhow!("reference work retired"))}
            else{work(&persistence,&worker_cancel,&activity)};
        let result=if worker_cancel.load(Ordering::Acquire) || activity.is_quiescing() || !persistence.is_current(){Err(anyhow!("reference work retired"))}else{result};
        drop(activity);let _=sender.send(result);
        #[cfg(test)]
        if let Some(after_send)=after_send{after_send();}
    });
    let worker=match worker{Ok(worker)=>worker,Err(_)=>{capture.status(app,&context,"参考图操作未能启动，请重试");return false;}};
    REFERENCE_WORKERS.with(|workers|workers.borrow_mut().push(ReferenceWorker{id,lease:capture.persistence.lease().clone(),cancel:cancel.clone(),handle:worker}));
    poll_reference_work(app.as_weak(),context,capture,Rc::new(RefCell::new(ReferenceJob{id,cancel,receiver})),poll_interval,complete);true
}
fn poll_reference_work<R:Send+'static>(weak:Weak<AppWindow>,context:AppContext,capture:ReferenceCapture,
    job:Rc<RefCell<ReferenceJob<R>>>,poll_interval:Duration,
    complete:impl FnOnce(&AppWindow,&AppContext,&ReferenceCapture,Result<R>)+'static){
    slint::Timer::single_shot(poll_interval,move||{
        reap_reference_workers();let id=job.borrow().id;
        let Some(app)=weak.upgrade()else{job.borrow().cancel.store(true,Ordering::Release);if reference_worker_pending(id){reference_orphan_poll(id);}return;};
        if !capture.current(&app,&context){job.borrow().cancel.store(true,Ordering::Release);}
        if reference_worker_pending(id){poll_reference_work(weak,context,capture,job,poll_interval,complete);return;}
        let result=match job.borrow().receiver.try_recv(){
            Ok(result)=>result,Err(TryRecvError::Disconnected)=>Err(anyhow!("reference worker disconnected")),
            Err(TryRecvError::Empty)=>{poll_reference_work(weak,context,capture,job.clone(),poll_interval,complete);return;},
        };
        if capture.current(&app,&context){complete(&app,&context,&capture,result);}
    });
}
fn reference_projection(app:&AppWindow,context:&AppContext,capture:&ReferenceCapture){
    // Pure owned-memory projection. No shared detached preview worker or filesystem call.
    let state=app.global::<AppState>();let previous=state.get_references();
    let images=previous.iter().map(|row|((row.id.to_string(),row.source_path.to_string()),row.image)).collect::<BTreeMap<_,_>>();
    let rows=capture.target.rows(&context.store.borrow()).iter().take(capture.target.limit()).map(|item|{
        let image=if capture.persistence.owns_path(Path::new(&item.source_path)){
            images.get(&(item.id.clone(),item.source_path.clone())).cloned().unwrap_or_default()
        }else{Image::default()};
        ReferenceItem{id:item.id.clone().into(),source_path:item.source_path.clone().into(),image}
    }).collect::<Vec<_>>();
    state.set_references(ModelRc::new(VecModel::from(rows)));
}
fn start_reference_previews(app:&AppWindow,context:AppContext,capture:ReferenceCapture){
    let rows=capture.target.rows(&context.store.borrow()).clone();
    spawn_reference_work(app,context,capture,move|persistence,cancel,_activity|{
        let mut prepared=Vec::new();
        for row in rows{if cancel.load(Ordering::Acquire){break;}
            if let Ok(preview)=prepare_owned_preview(persistence,Path::new(&row.source_path),PreviewPurpose::Reference){prepared.push((row,preview));}
        }Ok(prepared)
    },|app,context,capture,result|{
        let Ok(prepared)=result else{return;};
        capture.apply(app,context,||{
            let rows=app.global::<AppState>().get_references();
            for (original,preview) in prepared{
                if !capture.target.rows(&context.store.borrow()).iter().any(|item|item==&original){continue;}
                for index in 0..rows.row_count(){if let Some(mut row)=rows.row_data(index){
                    if row.id==original.id && row.source_path==original.source_path{row.image=materialize_delivery_preview(&preview);rows.set_row_data(index,row);break;}
                }}
            }
        });
    });
}
enum ReferenceMutation{Append(Vec<ReferenceData>),Remove(String),Clear,Retry}
fn reference_edit_epoch(ui:&Rc<RefCell<ReferenceUiState>>,capture:&ReferenceCapture)->Option<Uuid>{
    ui.borrow().edits.iter().find(|(lease,target,_)|lease==capture.persistence.lease() && target==&capture.target).map(|(_,_,epoch)|*epoch)
}
fn reference_retry_before_action(app:&AppWindow,context:&AppContext,capture:&ReferenceCapture)->bool{
    let ui=REFERENCE_UI.with(Rc::clone);
    let retry=ui.borrow().saves.iter().any(|save|save.lease==*capture.persistence.lease() && save.target==capture.target && save.retry);
    if retry{stage_reference_change(app,context.clone(),capture.clone(),ReferenceMutation::Retry);true}else{false}
}
fn stage_reference_change(app:&AppWindow,context:AppContext,capture:ReferenceCapture,mutation:ReferenceMutation){
    if !capture.current(app,&context){return;}
    if let ReferenceMutation::Append(rows)=&mutation{
        if rows.is_empty(){return;}
        if capture.target.rows(&context.store.borrow()).len()>=capture.target.limit(){
            capture.status(app,&context,&reference_limit_message(capture.target.limit()));return;
        }
    }
    let mut prepared=match capture.persistence.prepare_ordered_save(){Ok(value)=>Some(value),Err(_)=>{capture.status(app,&context,"参考图状态未能安全保存，请重试");return;}};
    let ui=REFERENCE_UI.with(Rc::clone);let success=match &mutation{ReferenceMutation::Append(_)=>"已添加参考图",ReferenceMutation::Retry=>"参考图状态已保存",_=>"参考图状态已保存"};
    let invalidates_imports=matches!(&mutation,ReferenceMutation::Remove(_)|ReferenceMutation::Clear);
    let queued=capture.apply(app,&context,||{
        let revision=Uuid::new_v4();
        if invalidates_imports{
            ui.borrow_mut().edits.retain(|(lease,target,_)|lease!=capture.persistence.lease() || target!=&capture.target);
            ui.borrow_mut().edits.push((capture.persistence.lease().clone(),capture.target.clone(),Uuid::new_v4()));
        }
        {
            let mut store=context.store.borrow_mut();let rows=capture.target.rows_mut(&mut store);
            match mutation{
                ReferenceMutation::Append(values)=>for value in values{if rows.len()<capture.target.limit(){rows.push(value);}},
                ReferenceMutation::Remove(id)=>rows.retain(|row|row.id!=id),ReferenceMutation::Clear=>rows.clear(),ReferenceMutation::Retry=>{},
            }
        }
        ui.borrow_mut().saves.retain(|save|save.lease!=*capture.persistence.lease() || save.target!=capture.target);
        ui.borrow_mut().saves.push(ReferenceSaveState{lease:capture.persistence.lease().clone(),target:capture.target.clone(),revision,retry:false});
        reference_projection(app,&context,&capture);app.global::<AppState>().set_generation_status("正在保存参考图状态...".into());
        (revision,prepared.take().unwrap().enqueue(local_store_data(app,&context.store.borrow())))
    });
    // The whole guarded enqueue Result and unused prepared value are handled outside the latch.
    drop(prepared);
    let Some((revision,queued))=queued else{return;};
    let receiver=match queued{
        Ok(receiver)=>receiver,Err(error)=>{
            drop(error);if let Some(save)=ui.borrow_mut().saves.iter_mut().find(|save|save.revision==revision){save.retry=true;}
            capture.status(app,&context,"参考图状态未能安全保存，请重试");return;
        }
    };
    let failure_ui=ui.clone();let failure_capture=capture.clone();let visible_capture=capture.clone();
    // A save receipt settles its original namespace even while another page is visible.
    // Only the bookkeeping capture omits the presentation target; UI keeps the original target.
    let mut ack_capture=capture.clone();ack_capture.require_target=false;
    let launched=spawn_reference_work(app,context.clone(),ack_capture,move|_,cancel,_activity|{
        loop{match receiver.recv_timeout(Duration::from_millis(50)){
            Ok(result)=>return result.map_err(|_|anyhow!("reference Store rejected")),
            Err(mpsc::RecvTimeoutError::Disconnected)=>return Err(anyhow!("reference Store disconnected")),
            Err(mpsc::RecvTimeoutError::Timeout) if cancel.load(Ordering::Acquire)=>return Err(anyhow!("reference save observer retired")),
            Err(mpsc::RecvTimeoutError::Timeout)=>{},
        }}
    },move|app,context,capture,result|{
        if !ui.borrow().saves.iter().any(|save|save.revision==revision){return;}
        let ok=result.is_ok();
        if capture.apply(app,context,||{
            if ok{ui.borrow_mut().saves.retain(|save|save.revision!=revision);}
            else if let Some(save)=ui.borrow_mut().saves.iter_mut().find(|save|save.revision==revision){save.retry=true;}
        }).is_none(){return;}
        visible_capture.status(app,context,if ok{success}else{"参考图状态未能安全保存，请重试"});
        if ok && visible_capture.current(app,context){start_reference_previews(app,context.clone(),visible_capture);}
    });
    if !launched{
        if let Some(save)=failure_ui.borrow_mut().saves.iter_mut().find(|save|save.revision==revision){save.retry=true;}
        failure_capture.status(app,&context,"参考图保存确认未完成，请重试");
    }
}
enum ReferenceSource{Paths(Vec<PathBuf>),Pixels(arboard::ImageData<'static>),Url(String)}
fn start_reference_import(app:&AppWindow,context:AppContext,capture:ReferenceCapture,source:ReferenceSource){
    if matches!(&source,ReferenceSource::Paths(paths) if paths.is_empty()){return;}
    if !capture.current(app,&context) || reference_retry_before_action(app,&context,&capture){return;}
    if capture.target.rows(&context.store.borrow()).len()>=capture.target.limit(){capture.status(app,&context,&reference_limit_message(capture.target.limit()));return;}
    let ui=REFERENCE_UI.with(Rc::clone);
    let Some(epoch)=capture.apply(app,&context,||{
        if let Some(epoch)=reference_edit_epoch(&ui,&capture){epoch}else{
            let epoch=Uuid::new_v4();ui.borrow_mut().edits.push((capture.persistence.lease().clone(),capture.target.clone(),epoch));epoch
        }
    })else{return;};
    capture.status(app,&context,"正在导入参考图...");
    let remaining=capture.target.limit().saturating_sub(capture.target.rows(&context.store.borrow()).len());
    spawn_reference_work(app,context,capture,move|persistence,cancel,_activity|{
        let authority=persistence.storage_authority()?;let mut rows=Vec::new();
        let mut save=|image:image::DynamicImage|->Result<()>{
            anyhow::ensure!(!cancel.load(Ordering::Acquire) && persistence.is_current(),"reference import retired");
            let path=persist_reference_image_for_namespace(&authority,&image)?;
            rows.push(ReferenceData{id:Uuid::new_v4().to_string(),source_path:path.to_str().ok_or_else(||anyhow!("unsupported reference path"))?.into()});Ok(())
        };
        match source{
            ReferenceSource::Url(url)=>{
                // Never hold an outer blocking effect over transfer/self-426.
                let bytes=download_captured_reference_bytes(&url,persistence)?;
                save(decode_reference_bytes(&bytes)?)?;
            }
            ReferenceSource::Pixels(pixels)=>{
                let width=u32::try_from(pixels.width)?;let height=u32::try_from(pixels.height)?;
                anyhow::ensure!(width>0 && height>0 && u64::from(width)*u64::from(height)<=100_000_000,"clipboard image exceeds policy");
                save(image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(width,height,pixels.bytes.into_owned()).ok_or_else(||anyhow!("invalid clipboard pixels"))?))?;
            }
            ReferenceSource::Paths(paths)=>for path in paths.into_iter().take(remaining){
                anyhow::ensure!(!cancel.load(Ordering::Acquire),"reference import retired");
                // Decode and publish each held source independently: never retain all decoded images.
                save(decode_owned_reference_source(&authority,&path)?)?;
            },
        }Ok(rows)
    },move|app,context,capture,result|{
        if reference_edit_epoch(&ui,capture)!=Some(epoch){return;}
        match result{
            Ok(rows)=>stage_reference_change(app,context.clone(),capture.clone(),ReferenceMutation::Append(rows)),
            Err(_)=>capture.status(app,context,"参考图未能安全导入，请重试；原文件保持不变"),
        }
    });
}
fn start_reference_url_for_context(app:&AppWindow,context:AppContext,url:String){
    if let Some(capture)=ReferenceCapture::new(app,&context){start_reference_import(app,context,capture,ReferenceSource::Url(url));}
}
pub(super) fn start_reference_paths_for_context(app:&AppWindow,context:AppContext,paths:Vec<PathBuf>)->bool{
    let Some(capture)=ReferenceCapture::new(app,&context)else{return false;};
    start_reference_import(app,context,capture,ReferenceSource::Paths(paths));true
}

fn open_captured_reference(app:&AppWindow,context:AppContext,id:String){
    let Some(capture)=ReferenceCapture::new(app,&context)else{return;};
    let Some(item)=capture.target.rows(&context.store.borrow()).iter().find(|item|item.id==id).cloned()else{return;};
    let request=Uuid::new_v4();let ui=REFERENCE_UI.with(Rc::clone);
    let selected=item.clone();
    if capture.apply(app,&context,||{
        ui.borrow_mut().viewer=Some(request);let state=app.global::<AppState>();
        state.set_viewer_id(item.id.clone().into());state.set_viewer_source("reference".into());state.set_viewer_source_path(item.source_path.clone().into());
        state.set_viewer_image(Image::default());state.set_viewer_title("参考图".into());state.set_viewer_prompt("".into());state.set_viewer_prompt_lines(1);
        state.set_viewer_time("".into());state.set_viewer_ratio("".into());state.set_viewer_quality("".into());state.set_viewer_model("".into());
        state.set_viewer_width(0);state.set_viewer_height(0);state.set_viewer_cutout_done(false);state.set_viewer_remove_black_done(false);state.set_viewer_upscale_done(false);state.set_viewer_open(true);
    }).is_none(){return;}
    spawn_reference_work(app,context,capture,move|persistence,_,_activity|prepare_owned_preview(persistence,Path::new(&item.source_path),PreviewPurpose::Viewer),
        move|app,context,capture,result|{
            capture.apply(app,context,||{
                let state=app.global::<AppState>();
                if ui.borrow().viewer!=Some(request) || !state.get_viewer_open() || state.get_viewer_source()!="reference"
                    || state.get_viewer_id()!=selected.id || state.get_viewer_source_path()!=selected.source_path
                    || !capture.target.rows(&context.store.borrow()).iter().any(|item|item==&selected){return;}
                match result{
                    Ok(preview)=>state.set_viewer_image(materialize_delivery_preview(&preview)),
                    Err(_)=>state.set_generation_status("参考图预览未能安全加载，请重试".into()),
                }
            });
        });
}


#[cfg(test)]
thread_local!{
    static REFERENCE_TEST_POINTER_EXIT:RefCell<Option<Box<dyn FnOnce()>>>=const{RefCell::new(None)};
    static REFERENCE_TEST_FILE_DRAG:RefCell<Option<Box<dyn FnOnce(CapturedNativeFileDrag)->bool>>>=const{RefCell::new(None)};
    static REFERENCE_TEST_DRAG_PREVIEW:RefCell<Option<Box<dyn FnOnce(image::RgbaImage)->bool+Send>>>=const{RefCell::new(None)};
}
fn reference_native_file_drag(drag:CapturedNativeFileDrag)->bool{
    #[cfg(test)]
    if let Some(effect)=REFERENCE_TEST_FILE_DRAG.with(|hook|hook.borrow_mut().take()){return effect(drag);}
    drag_preview::start_thumbnail_file_drag_captured(drag)
}
fn reference_native_path_drag(path:PathBuf)->bool{
    #[cfg(test)]
    { let _=path; return false; }
    #[cfg(not(test))]
    drag_preview::start_thumbnail_file_drag_path(path)
}
fn reference_pointer_exit(app:&AppWindow){
    #[cfg(test)]
    if let Some(effect)=REFERENCE_TEST_POINTER_EXIT.with(|hook|hook.borrow_mut().take()){effect();return;}
    let _=app.window().try_dispatch_event(slint::platform::WindowEvent::PointerExited);
}
fn reference_path_in_store(context:&AppContext,path:&Path)->bool{
    let store=context.store.borrow();
    store.assets.iter().chain(store.generations.iter()).any(|asset|crate::directory_migration::same_path(Path::new(&asset.source_path),path))
        || [&store.references.character,&store.references.scene,&store.references.ui,&store.references.effect,&store.canvas_references]
            .into_iter().any(|rows|rows.iter().any(|row|crate::directory_migration::same_path(Path::new(&row.source_path),path)))
}
fn rewrite_path(value:&mut String,original:&Path,replacement:&Path)->bool{
    if !crate::directory_migration::same_path(Path::new(value),original){return false;}
    *value=display_directory_path(replacement);true
}
fn rewrite_native_drag_store_path(store:&mut Store,original:&Path,replacement:&Path)->bool{
    let mut changed=false;
    for asset in store.assets.iter_mut().chain(&mut store.generations){
        changed|=rewrite_path(&mut asset.source_path,original,replacement);
        for path in &mut asset.reference_paths{changed|=rewrite_path(path,original,replacement);}
    }
    for rows in [&mut store.references.character,&mut store.references.scene,&mut store.references.ui,
        &mut store.references.effect,&mut store.canvas_references]{
        for row in rows{changed|=rewrite_path(&mut row.source_path,original,replacement);}
    }
    for note in &mut store.canvas_notes{changed|=rewrite_path(&mut note.image_path,original,replacement);}
    for workspace in store.canvas_workspaces.values_mut(){
        for note in &mut workspace.notes{changed|=rewrite_path(&mut note.image_path,original,replacement);}
        for row in &mut workspace.references{changed|=rewrite_path(&mut row.source_path,original,replacement);}
    }
    for profile in store.custom_prompt_profiles.values_mut(){
        changed|=rewrite_path(&mut profile.reference_path,original,replacement);
        for path in &mut profile.reference_paths{changed|=rewrite_path(path,original,replacement);}
    }
    changed
}
fn rewrite_native_drag_ui_path(app:&AppWindow,original:&Path,replacement:&Path){
    let state=app.global::<AppState>();let replacement_text=display_directory_path(replacement);
    for model in [state.get_assets(),state.get_generations()]{
        for index in 0..model.row_count(){if let Some(mut item)=model.row_data(index){
            if crate::directory_migration::same_path(Path::new(item.source_path.as_str()),original){
                item.source_path=replacement_text.clone().into();item.drag_uri=file_uri_for_path(&replacement_text).into();model.set_row_data(index,item);
            }
        }}
    }
    let references=state.get_references();
    for index in 0..references.row_count(){if let Some(mut item)=references.row_data(index){
        if crate::directory_migration::same_path(Path::new(item.source_path.as_str()),original){
            item.source_path=replacement_text.clone().into();references.set_row_data(index,item);
        }
    }}
    if state.get_viewer_open() && crate::directory_migration::same_path(Path::new(state.get_viewer_source_path().as_str()),original){
        state.set_viewer_source_path(replacement_text.into());
    }
}
fn legacy_native_drag_extension(path:&Path,bytes:&[u8])->String{
    path.extension().and_then(|value|value.to_str()).map(str::to_ascii_lowercase)
        .filter(|extension|crate::image_formats::picker_image_extensions().contains(&extension.as_str()))
        .unwrap_or_else(||image_extension(bytes).into())
}
fn prepare_legacy_native_drag_source(persistence:&PrivatePersistence,path:&Path)->Result<PreparedLegacyNativeDrag>{
    let authority=persistence.storage_authority()?;let bytes=authority.read_image_source(path,MAX_DROPPED_IMAGE_BYTES)?;
    let(decoded,_)=decode_image_bytes(path,&bytes)?;
    anyhow::ensure!(decoded.width()>0 && decoded.height()>0 && u64::from(decoded.width())*u64::from(decoded.height())<=100_000_000,
        "legacy native drag image dimensions invalid");
    let stem=path.file_stem().and_then(|value|value.to_str()).map(sanitize_filename).filter(|value|!value.is_empty()).unwrap_or_else(||"legacy-image".into());
    let leaf=format!("{}-migrated-{}.{}",stem,Uuid::new_v4(),legacy_native_drag_extension(path,&bytes));
    let key=ManagedFileKey::new(ManagedUserArea::Output,&leaf)?;let _mutation=authority.begin_ordinary_mutation()?;
    let mut temporary=authority.create_temporary_regular_for(&key)?;
    authority.write_new_regular_from(&mut temporary,&mut std::io::Cursor::new(&bytes))?;authority.sync_regular(&mut temporary)?;
    authority.publish_regular(&mut temporary,NamespaceManagedPublication::Absent(&key))?;
    let file=authority.open_existing_regular(&key)?;
    let registration=NamespacedManagedFileRegistration::new(&authority,file,"image","user")?;
    let indexed=match authority.delivery_index()?.register_file_for_namespace(&authority,&registration){
        Ok(indexed)=>indexed,Err(error)=>{drop(registration);if let Ok(file)=authority.open_existing_regular(&key){let _=authority.unlink_regular(file);}return Err(error.into());}
    };
    drop(registration);
    let migrated=authority.lease().namespace.path(ManagedUserArea::Output).join(&leaf);
    let source=match prepare_native_file_drag_source(persistence,&migrated){
        Ok(source)=>source,Err(error)=>{
            if let Ok(file)=authority.open_existing_regular(&key){
                if authority.delivery_index()?.delete_file_for_namespace(&authority,indexed.id).unwrap_or(false){let _=authority.unlink_regular(file);}
            }
            return Err(error);
        }
    };
    Ok(PreparedLegacyNativeDrag{source,path:migrated,file_id:indexed.id})
}
fn discard_prepared_legacy_native_drag(persistence:&PrivatePersistence,prepared:PreparedLegacyNativeDrag)->Result<()>{
    let PreparedLegacyNativeDrag{source,path,file_id}=prepared;drop(source);
    let authority=persistence.storage_authority()?;
    let relative=path.strip_prefix(authority.lease().namespace.path(ManagedUserArea::Output))?
        .to_str().ok_or_else(||anyhow!("legacy native drag path encoding"))?;
    let key=ManagedFileKey::new(ManagedUserArea::Output,relative)?;let file=authority.open_existing_regular(&key)?;
    anyhow::ensure!(authority.delivery_index()?.delete_file_for_namespace(&authority,file_id)?,"legacy native drag index missing");
    authority.unlink_regular(file)
}
fn schedule_legacy_native_drag_cleanup(app:&AppWindow,context:AppContext,mut capture:ReferenceCapture,prepared:PreparedLegacyNativeDrag){
    capture.require_target=false;
    let _=spawn_reference_work(app,context,capture,move|persistence,_,_|discard_prepared_legacy_native_drag(persistence,prepared),|_,_,_,_|{});
}
fn rebind_reference_native_request(ticket:&ReferenceNativeTicket,capture:&ReferenceCapture,path:&Path)->bool{
    if !ticket.current(){return false;}let mut ui=ticket.ui.borrow_mut();
    let Some(request)=ui.native.as_mut().filter(|request|request.id==ticket.id && request.lease==*capture.persistence.lease())else{return false;};
    request.target=capture.target.clone();request.path=path.to_path_buf();request.preview_unclaimed=false;true
}
fn reference_reset_pointer(app:&AppWindow,context:AppContext,capture:ReferenceCapture,ticket:ReferenceNativeTicket){
    if !ticket.current(){return;}
    capture.apply(app,&context,||{
        if ticket.current(){app.global::<AppState>().set_thumbnail_drag_preview_visible(false);}
    });
    let weak=app.as_weak();
    slint::Timer::single_shot(Duration::ZERO,move||{
        let Some(app)=weak.upgrade()else{return;};
        if !ticket.current() || !capture.current(&app,&context){return;}
        let Ok(effect)=capture.persistence.begin_effect()else{return;};
        if ticket.current() && capture.current(&app,&context){reference_pointer_exit(&app);}
        drop(effect);
    });
}
fn poll_legacy_native_drag_save(
    weak:Weak<AppWindow>,context:AppContext,account_capture:ReferenceCapture,visible_capture:ReferenceCapture,
    ticket:ReferenceNativeTicket,original:PathBuf,prepared:PreparedLegacyNativeDrag,
    receiver:mpsc::Receiver<client_state::WriteResult>,
){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        let Some(app)=weak.upgrade()else{return;};
        if !account_capture.current(&app,&context){return;}
        let saved=match receiver.try_recv(){
            Ok(result)=>result.is_ok(),
            Err(TryRecvError::Empty)=>{
                poll_legacy_native_drag_save(weak,context,account_capture,visible_capture,ticket,original,prepared,receiver);return;
            }
            Err(TryRecvError::Disconnected)=>false,
        };
        if !saved{
            ticket.cancel.store(true,Ordering::Release);let migrated=prepared.path.clone();
            let can_cleanup=account_capture.apply(&app,&context,||{
                {let mut store=context.store.borrow_mut();rewrite_native_drag_store_path(&mut store,&migrated,&original);}
                !reference_path_in_store(&context,&migrated)
            }).unwrap_or(false);
            visible_capture.status(&app,&context,"作品原图路径未能安全迁移，请重试");
            if can_cleanup{schedule_legacy_native_drag_cleanup(&app,context,account_capture,prepared);}
            return;
        }
        let should_launch=ticket.current() && visible_capture.current(&app,&context);
        let migrated=prepared.path.clone();
        let published=account_capture.apply(&app,&context,||{
            if !reference_path_in_store(&context,&migrated){return false;}
            rewrite_native_drag_ui_path(&app,&original,&migrated);true
        }).unwrap_or(false);
        if !published || !should_launch{ticket.cancel.store(true,Ordering::Release);return;}
        let Some(next_capture)=ReferenceCapture::native(&app,&context)else{return;};
        if !rebind_reference_native_request(&ticket,&next_capture,&migrated){return;}
        let Ok(drag)=bind_native_file_drag(&context,prepared.source)else{return;};
        let current_path=migrated.clone();let weak=app.as_weak();let current_context=context.clone();
        let target=next_capture.clone();let queued_ticket=ticket.clone();
        let drag=drag.with_presentation_check(move||queued_ticket.current() && weak.upgrade().is_some_and(|app|
            target.current(&app,&current_context) && reference_path_in_store(&current_context,&current_path)));
        if !ticket.current() || !next_capture.current(&app,&context){return;}
        let _=reference_native_file_drag(drag);
        reference_reset_pointer(&app,context,next_capture,ticket);
    });
}
fn save_legacy_native_drag_path(
    app:&AppWindow,context:AppContext,mut account_capture:ReferenceCapture,visible_capture:ReferenceCapture,
    ticket:ReferenceNativeTicket,original:PathBuf,prepared:PreparedLegacyNativeDrag,
){
    account_capture.require_target=false;
    let mut write=match account_capture.persistence.prepare_ordered_save(){
        Ok(write)=>Some(write),Err(_)=>{
            ticket.cancel.store(true,Ordering::Release);visible_capture.status(app,&context,"作品原图路径未能安全迁移，请重试");
            schedule_legacy_native_drag_cleanup(app,context,account_capture,prepared);return;
        }
    };
    let migrated=prepared.path.clone();
    if !ticket.current() || !visible_capture.current(app,&context){
        ticket.cancel.store(true,Ordering::Release);
        schedule_legacy_native_drag_cleanup(app,context,account_capture,prepared);return;
    }
    let queued=account_capture.apply(app,&context,||{
        if !ticket.current() || !visible_capture.target_matches(app,&context){return None;}
        let changed={let mut store=context.store.borrow_mut();rewrite_native_drag_store_path(&mut store,&original,&migrated)};
        if !changed{return None;}
        Some(write.take().unwrap().enqueue(local_store_data(app,&context.store.borrow())))
    });
    drop(write);
    let receiver=match queued.flatten(){
        Some(Ok(receiver))=>receiver,
        Some(Err(error))=>{
            drop(error);ticket.cancel.store(true,Ordering::Release);
            account_capture.apply(app,&context,||{let mut store=context.store.borrow_mut();rewrite_native_drag_store_path(&mut store,&migrated,&original);});
            visible_capture.status(app,&context,"作品原图路径未能安全迁移，请重试");
            schedule_legacy_native_drag_cleanup(app,context,account_capture,prepared);return;
        }
        None=>{
            ticket.cancel.store(true,Ordering::Release);
            schedule_legacy_native_drag_cleanup(app,context,account_capture,prepared);return;
        }
    };
    poll_legacy_native_drag_save(app.as_weak(),context,account_capture,visible_capture,ticket,original,prepared,receiver);
}
fn start_legacy_reference_native_drag(
    app:&AppWindow,context:AppContext,capture:ReferenceCapture,ticket:ReferenceNativeTicket,path:PathBuf,
)->bool{
    let visible_capture=capture.clone();let completion=ticket.clone();let native_cancel=ticket.cancel.clone();
    let worker_path=path.clone();let mut worker_capture=capture;worker_capture.require_target=false;
    spawn_reference_work(app,context,worker_capture,move|persistence,_,_|{
        if native_cancel.load(Ordering::Acquire){anyhow::bail!("legacy native drag superseded");}
        let prepared=prepare_legacy_native_drag_source(persistence,&worker_path)?;
        if native_cancel.load(Ordering::Acquire){anyhow::bail!("legacy native drag superseded");}
        Ok(prepared)
    },move|app,context,account_capture,result|{
        let Ok(prepared)=result else{visible_capture.status(app,context,"原图文件不可用，无法拖拽");return;};
        if !completion.current() || !visible_capture.current(app,context) || !reference_path_in_store(context,&path){
            schedule_legacy_native_drag_cleanup(app,context.clone(),account_capture.clone(),prepared);return;
        }
        save_legacy_native_drag_path(app,context.clone(),account_capture.clone(),visible_capture,completion,path,prepared);
    })
}
fn prepare_reference_native_drag(app:&AppWindow,context:&AppContext,data:String)->bool{
    let _=(app,context);
    !data.is_empty()
}
fn launch_prepared_reference_native_drag(
    app:&AppWindow,context:&AppContext,capture:ReferenceCapture,ticket:ReferenceNativeTicket,
    original:PathBuf,source:PreparedNativeFileDragSource,
)->bool{
    if !ticket.current() || !capture.current(app,context) || !reference_path_in_store(context,&original){return false;}
    let Ok(drag)=bind_native_file_drag(context,source)else{return false;};
    let weak=app.as_weak();let current_context=context.clone();let target=capture.clone();let queued_ticket=ticket.clone();
    let drag=drag.with_presentation_check(move||queued_ticket.current() && weak.upgrade().is_some_and(|app|target.current(&app,&current_context)));
    if !ticket.current() || !capture.current(app,context){return false;}
    let started=reference_native_file_drag(drag);
    reference_reset_pointer(app,context.clone(),capture,ticket);
    started
}
fn start_reference_native_drag(app:&AppWindow,context:AppContext,data:String,preview:bool)->bool{
    let Some(capture)=ReferenceCapture::native(app,&context)else{return false;};
    let Some(path)=drag_data_to_path(&data)else{return false;};
    if matches!(&capture.target,ReferenceTarget::NativeViewer{path:original,..} if !crate::directory_migration::same_path(Path::new(original),&path)){return false;}
    if !reference_path_in_store(&context,&path){return false;}
    if preview && !cfg!(windows){return false;}
    let owned=capture.persistence.owns_path(&path);
    if preview && !owned{return false;}
    if !preview && owned{
        if let Some(ticket)=begin_reference_native_request(app,&context,&capture,&path,false){
            let started=reference_native_path_drag(path.clone());
            if started{
                reference_reset_pointer(app,context,capture,ticket);
                return true;
            }
            ticket.cancel.store(true,Ordering::Release);
        }
    }
    let Some(ticket)=begin_reference_native_request(app,&context,&capture,&path,preview)else{return false;};
    if !owned{
        let started=start_legacy_reference_native_drag(app,context,capture,ticket.clone(),path);
        if !started{ticket.cancel.store(true,Ordering::Release);}return started;
    }
    let native_cancel=ticket.cancel.clone();
    let started=if preview{
        #[cfg(test)]
        let test_effect=REFERENCE_TEST_DRAG_PREVIEW.with(|hook|hook.borrow_mut().take());
        spawn_reference_work(app,context,capture,move|persistence,cancel,activity|{
            if native_cancel.load(Ordering::Acquire){anyhow::bail!("native preview superseded");}
            let authority=persistence.storage_authority()?;let decoded=decode_owned_reference_source(&authority,&path)?;
            let pixels=decoded.thumbnail(220,220).to_rgba8();
            let effect=persistence.upgrade_latch().begin_ordinary_external_worker().map_err(|required|anyhow!(required.as_error().user_message()))?;
            let keep_running=||!native_cancel.load(Ordering::Acquire) && !cancel.load(Ordering::Acquire)
                && !effect.is_cancelled() && !activity.is_quiescing();
            if !keep_running(){anyhow::bail!("native preview retired");}
            #[cfg(test)]
            if let Some(test_effect)=test_effect{return Ok(test_effect(pixels));}
            Ok(drag_preview::run_thumbnail_drag_preview_owned(pixels,keep_running))
        },|_,_,_,_|{})
    }else{
        let original=path.clone();let completion=ticket.clone();
        spawn_reference_work_with_poll(app,context,capture,NATIVE_DRAG_POLL_INTERVAL,move|persistence,_,_activity|{
            if native_cancel.load(Ordering::Acquire){anyhow::bail!("native drag superseded");}
            let source=prepare_native_file_drag_source(persistence,&path)?;
            if native_cancel.load(Ordering::Acquire){anyhow::bail!("native drag superseded");}
            Ok(source)
        },move|app,context,capture,result|{
            let Ok(source)=result else{return;};
            let _=launch_prepared_reference_native_drag(app,context,capture.clone(),completion,original,source);
        })
    };
    if !started{ticket.cancel.store(true,Ordering::Release);}
    started
}
pub(super) fn wire_reference_callbacks(app:&AppWindow,context:AppContext){
    let state=app.global::<AppState>();
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_add_reference(move||{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=ReferenceCapture::new(&app,&context)else{return;};
            if reference_retry_before_action(&app,&context,&capture){return;}
            let Ok(effect)=capture.persistence.begin_effect()else{return;};
            if !capture.current(&app,&context){drop(effect);return;}
            let weak=app.as_weak();let context=context.clone();
            // The async dialog owns its counted guard until return/cancellation; then releases
            // outside the short completion before admitting any worker or Store write.
            reference_pick_files(Box::new(move|paths|{
                drop(effect);
                if let Some(app)=weak.upgrade(){if capture.current(&app,&context){start_reference_import(&app,context,capture,ReferenceSource::Paths(paths));}}
            }));
        });
    }
    {
        let weak = app.as_weak(); let context = context.clone();
        state.on_add_reference_from_asset(move |id| {
            let Some(app) = weak.upgrade() else { return false; };
            let Some(capture) = ReferenceCapture::new(&app, &context) else { return false; };
            if reference_retry_before_action(&app, &context, &capture) { return true; }
            let path = capture.apply(&app, &context, || context.store.borrow().assets.iter()
                .find(|asset| asset.id == id.as_str())
                .map(|asset| PathBuf::from(&asset.source_path))).flatten();
            let Some(path) = path else { return false; };
            start_reference_import(&app, context.clone(), capture, ReferenceSource::Paths(vec![path]));
            true
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_paste_reference(move||{
            let Some(app)=weak.upgrade()else{return false;};let Some(capture)=ReferenceCapture::new(&app,&context)else{return false;};
            if reference_retry_before_action(&app,&context,&capture){return true;}
            let Ok(effect)=capture.persistence.begin_effect()else{return false;};
            let pixels=if capture.current(&app,&context){reference_clipboard_image()}else{None};drop(effect);
            let Some(pixels)=pixels else{return false;};
            if !capture.current(&app,&context){return false;}
            start_reference_import(&app,context.clone(),capture,ReferenceSource::Pixels(pixels));true
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_add_reference_from_transfer(move|transfer|{
            let Some(app)=weak.upgrade()else{return false;};let Some(capture)=ReferenceCapture::new(&app,&context)else{return false;};
            let Ok(effect)=capture.persistence.begin_effect()else{return false;};
            let data=transfer.plain_text();drop(effect);let Ok(data)=data else{return false;};
            if !capture.current(&app,&context){return false;}
            if let Some(url)=external_image_url(data.as_str()){start_reference_import(&app,context.clone(),capture,ReferenceSource::Url(url));true}
            else{let paths=drag_data_to_paths(data.as_str());if paths.is_empty(){return false;}start_reference_import(&app,context.clone(),capture,ReferenceSource::Paths(paths));true}
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_remove_reference(move|id|{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=ReferenceCapture::new(&app,&context)else{return;};
            if !reference_retry_before_action(&app,&context,&capture){stage_reference_change(&app,context.clone(),capture,ReferenceMutation::Remove(id.to_string()));}
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_clear_references(move||{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=ReferenceCapture::new(&app,&context)else{return;};
            if !reference_retry_before_action(&app,&context,&capture){stage_reference_change(&app,context.clone(),capture,ReferenceMutation::Clear);}
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_open_reference(move|id|if let Some(app)=weak.upgrade(){open_captured_reference(&app,context.clone(),id.to_string());});
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_prepare_thumbnail_file_drag(move|data|weak.upgrade().is_some_and(|app|prepare_reference_native_drag(&app,&context,data.to_string())));
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_start_thumbnail_drag_preview(move|data|weak.upgrade().is_some_and(|app|start_reference_native_drag(&app,context.clone(),data.to_string(),true)));
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_start_thumbnail_file_drag(move|data|weak.upgrade().is_some_and(|app|start_reference_native_drag(&app,context.clone(),data.to_string(),false)));
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_process_external_image_drops(move||if let Some(app)=weak.upgrade(){process_captured_external_image_drops(&app,context.clone());});
    }
}
fn process_captured_external_image_drops(app:&AppWindow,context:AppContext){
    // Drain platform delivery once. Refusal never moves these paths into another namespace.
    let drops=platform::take_external_image_drops();
    if app.global::<AppState>().get_directory_migration_open(){return;}
    let Some(capture)=ReferenceCapture::native(app,&context)else{return;};
    let page=app.global::<AppState>().get_page().to_string();
    for drop in drops{
        if !capture.current(app,&context){return;}
        match drop{
            ExternalImageDrop::Paths(paths,position)=>{
                match page.as_str(){
                    "generation"|"canvas" if external_drop_inside_reference_input(app,position.as_ref())=>{
                        start_reference_paths_for_context(app,context.clone(),paths);
                    }
                    "toolbox-compress"=>toolbox_callbacks::add_compression_paths_for_store(app,&context.store,paths),
                    "toolbox-convert"=>toolbox_callbacks::add_conversion_paths_for_store(app,&context.store,paths),
                    "toolbox-crop"=>{toolbox_callbacks::add_crop_paths_for_store(app,&context.store,paths);}
                    "toolbox-watermark"=>{toolbox_callbacks::add_watermark_paths_for_store(app,&context.store,paths);}
                    "toolbox-colorize"=>{toolbox_callbacks::add_colorization_paths_for_store(app,&context.store,paths);}
                    "toolbox-enhance"=>{image_enhancement_callbacks::add_enhancement_paths_for_store(app,paths,&context.store.borrow());}
                    _=>{},
                }
            }
            #[cfg(windows)]
            ExternalImageDrop::Text(data,position)=>{
                match page.as_str(){
                    "generation"|"canvas" if external_drop_inside_reference_input(app,position.as_ref())=>{
                        if let Some(url)=external_image_url(&data){start_reference_url_for_context(app,context.clone(),url);}
                        else{start_reference_paths_for_context(app,context.clone(),drag_data_to_paths(&data));}
                    }
                    "toolbox-compress"=>{toolbox_callbacks::add_compression_drag_for_store(app,&context.store,TEXT_PLAIN_MIME,&data);}
                    "toolbox-convert"=>{toolbox_callbacks::add_conversion_drag_for_store(app,&context.store,TEXT_PLAIN_MIME,&data);}
                    "toolbox-crop"=>{toolbox_callbacks::add_crop_drag_for_store(app,&context.store,TEXT_PLAIN_MIME,&data);}
                    "toolbox-watermark"=>{toolbox_callbacks::add_watermark_drag_for_store(app,&context.store,TEXT_PLAIN_MIME,&data);}
                    "toolbox-colorize"=>{toolbox_callbacks::add_colorization_drag_for_store(app,&context.store,TEXT_PLAIN_MIME,&data);}
                    "toolbox-enhance"=>{image_enhancement_callbacks::add_enhancement_from_drag_data_for_store(app,TEXT_PLAIN_MIME,&data,&context.store.borrow());}
                    _=>{},
                }
            }
        }
    }
}

fn external_drop_inside_reference_input(
    app: &AppWindow,
    position: Option<&ExternalDropPosition>,
) -> bool {
    let Some(position) = position else {
        return false;
    };
    let state = app.global::<AppState>();
    let scale = if position.physical {
        app.window().scale_factor().max(f32::EPSILON)
    } else {
        1.0
    };
    let x = position.x / scale;
    let y = position.y / scale;
    let left = state.get_reference_drop_x();
    let top = state.get_reference_drop_y();
    let width = state.get_reference_drop_width();
    let height = state.get_reference_drop_height();
    width > 0.0 && height > 0.0 && x >= left && x <= left + width && y >= top && y <= top + height
}

pub(super) fn download_captured_reference_bytes(url: &str, persistence: &PrivatePersistence) -> Result<Vec<u8>> {
    let latch = persistence.upgrade_latch();
    let transfer = latch.begin_ordinary_transfer().map_err(|required| anyhow!(required.as_error().user_message()))?;
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5)).build()?;
    let mut response = client.get(url).send()?;
    let status = response.status();
    anyhow::ensure!(response.content_length().unwrap_or(0) <= MAX_DROPPED_IMAGE_BYTES, "image exceeds limit");
    let mut bytes=Vec::new();let mut block=[0u8;64*1024];
    loop{
        anyhow::ensure!(!transfer.is_cancelled() && persistence.is_current(),"image transfer retired");
        let remaining=(MAX_DROPPED_IMAGE_BYTES+1).saturating_sub(bytes.len() as u64);
        anyhow::ensure!(remaining>0,"image exceeds limit");
        let capacity=remaining.min(block.len() as u64) as usize;
        let count=response.read(&mut block[..capacity])?;
        if count==0{break;}bytes.extend_from_slice(&block[..count]);
    }
    anyhow::ensure!(bytes.len() as u64 <= MAX_DROPPED_IMAGE_BYTES, "image exceeds limit");
    if status.as_u16() == 426 {
        if let Ok(envelope) = serde_json::from_slice::<api::ApiEnvelope<serde_json::Value>>(&bytes) {
            if let Some(problem) = envelope.error {
                let error = ApiError::Http { status: 426, code: problem.code, message: problem.message, details: problem.details, request_id: None };
                if let Some(required) = RequiredUpgrade::from_error(&error) {
                    latch.trip_from_ordinary_transfer(transfer, required, || drop(bytes));
                    anyhow::bail!("当前客户端版本过旧，必须更新后继续使用");
                }
            }
        }
    }
    anyhow::ensure!(status.is_success() && !latch.is_tripped(), "image download rejected");
    Ok(bytes)
}

pub(super) fn download_external_reference(url: &str) -> std::result::Result<PathBuf, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("ElunviCanvas/1.0")
        .build()
        .map_err(|_| "无法创建图片下载请求".to_string())?;
    let response = client
        .get(url)
        .send()
        .map_err(|_| "无法下载拖入的网页图片".to_string())?
        .error_for_status()
        .map_err(|_| "网页图片地址不可访问".to_string())?;
    if response.content_length().unwrap_or(0) > MAX_DROPPED_IMAGE_BYTES {
        return Err("拖入的图片超过 100 MB 安全限制".to_string());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_DROPPED_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取网页图片失败".to_string())?;
    if bytes.len() as u64 > MAX_DROPPED_IMAGE_BYTES {
        return Err("拖入的图片超过 100 MB 安全限制".to_string());
    }
    let format = image::guess_format(&bytes).map_err(|_| "拖入的网址不是有效图片".to_string())?;
    image::load_from_memory(&bytes).map_err(|_| "拖入的网址不是受支持的图片".to_string())?;
    let extension = match format {
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::WebP => "webp",
        image::ImageFormat::Gif => "gif",
        image::ImageFormat::Bmp => "bmp",
        image::ImageFormat::Tiff => "tiff",
        _ => "png",
    };
    let directory = app_data_dir().join("references").join("imports");
    if !ensure_managed_subdirectory(&directory) {
        return Err("无法创建参考图目录".to_string());
    }
    let destination = directory.join(format!("dragged-{}.{}", Uuid::new_v4(), extension));
    atomic_write_file(&destination, &bytes).map_err(|_| "无法保存拖入的图片".to_string())?;
    Ok(destination)
}

const STALE_REFERENCE_IMPORT_AGE: Duration = Duration::from_secs(24 * 60 * 60);

fn reference_import_dir() -> PathBuf {
    app_data_dir().join("references").join("imports")
}

fn is_managed_reference_import_path(path: &Path) -> bool {
    is_managed_reference_import_path_in(path,&reference_import_dir())
}
fn is_managed_reference_import_path_in(path:&Path,directory:&Path)->bool{
    path.parent().is_some_and(|parent| parent == directory)
        && path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with("dragged-") && !name.ends_with(".part"))
}

fn remove_managed_reference_import(path: &Path) {
    if is_managed_reference_import_path(path)
        && safe_managed_subdirectory(&reference_import_dir())
        && fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
    {
        let _ = fs::remove_file(path);
    }
}

pub(super) fn cleanup_stale_reference_imports() {
    let now = std::time::SystemTime::now();
    let directory = reference_import_dir();
    if !safe_managed_subdirectory(&directory) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_managed_reference_import_path(&path) {
            continue;
        }
        let stale = fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= STALE_REFERENCE_IMPORT_AGE);
        if stale {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod managed_import_tests {
    use super::*;

    #[test]
    fn managed_reference_import_filter_never_accepts_external_paths() {
        let root=tempfile::tempdir().unwrap();let directory=root.path().join("imports");
        let managed = directory.join("dragged-example.png");
        assert!(is_managed_reference_import_path_in(&managed,&directory));
        assert!(!is_managed_reference_import_path_in(
            &root.path().join("out").join("dragged-example.png"),&directory
        ));
        assert!(!is_managed_reference_import_path_in(
            &directory.join("unrelated.png"),&directory
        ));
        assert!(!is_managed_reference_import_path_in(
            &directory.join("dragged-example.png.part"),&directory
        ));
    }
}


#[cfg(test)]
mod core_reference_tests{
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool,Ordering};
    const OWNER:&str="11111111-1111-4111-8111-111111111111";
    struct Fixture{writer:client_state::tests::Fixture,context:AppContext,persistence:PrivatePersistence,external:tempfile::TempDir,expected_join_failure:bool}
    fn fixture()->(Fixture,AppWindow){
        i_slint_backend_testing::init_no_event_loop();
        let writer=client_state::tests::Fixture::new(false,false);
        let session=Arc::new(SessionManager::new(Arc::new(api::test_support::MemoryRefreshTokenStore::default())));
        let scope=session.install_tokens_for_user(&TokenSet{access_token:"fixture-access".into(),access_expires_in_seconds:1800,
            refresh_token:"fixture-refresh".into(),refresh_expires_at:"2099-01-01T00:00:00Z".into(),token_type:"X-Token".into()},OWNER).unwrap();
        let lease=writer.lease(OWNER,scope.auth_epoch,1);writer.activate(lease.clone()).unwrap();
        let root=writer.data_root_capability_arc();
        let authority=NamespaceStorageAuthority::open(root.clone(),&lease).unwrap();
        let index=FileIndex::initialize(lease.namespace.root().parent().unwrap().parent().unwrap().join("reference-index.sqlite3")).unwrap();
        let backend=Arc::new(BackendRuntime{api:ApiClient::new(ApiClientConfig{base_url:reqwest::Url::parse("http://127.0.0.1:9/").unwrap(),
            app_version:"999.0.0".into(),timeout:Duration::from_secs(2)},
            DeviceIdentity{id:"22222222-2222-4222-8222-222222222222".into(),name:"fixture".into(),platform:"macos".into()},session).unwrap()});
        let context=AppContext{data_root_capability:Some(root.clone()),file_index:Some(index.clone()),backend:Some(backend.clone()),
            current_user_id:Arc::new(Mutex::new(Some(OWNER.into()))),account_snapshot_scope:Arc::new(Mutex::new(Some(scope))),..Default::default()};
        context.user_activity.activate(lease.clone()).unwrap();*context.active_namespace.lock().unwrap()=Some(lease.clone());
        backend.api.bind_user_work(UserWorkAdmission::new(context.active_namespace.clone(),context.user_activity.clone())).unwrap();
        let transition=context.namespace_operations.try_begin_transition().unwrap();
        let phase=transition.begin_prepublication_recovery(&lease).unwrap();phase.verify_no_unsupported_imports(&authority).unwrap();
        let recovered=phase.finish().unwrap();transition.prepare_publication(&lease,recovered).unwrap().publish();
        let persistence=PrivatePersistence::for_test_with_storage((*writer).clone(),lease,context.user_activity.clone(),backend.api.upgrade_latch().clone(),root,backend.api.clone(),index);
        context.store.borrow_mut().private_persistence=Some(persistence.clone());
        let app=AppWindow::new().unwrap();let state=app.global::<AppState>();state.set_page("generation".into());state.set_asset_type("character".into());
        wire_reference_callbacks(&app,context.clone());
        (Fixture{writer,context,persistence,external:tempfile::tempdir().unwrap(),expected_join_failure:false},app)
    }
    impl Drop for Fixture{fn drop(&mut self){
        {
            let mut active=self.context.active_namespace.lock().unwrap_or_else(|poison|poison.into_inner());
            if active.as_ref()==Some(self.persistence.lease()){*active=None;}
        }
        cancel_reference_workers_for_retirement(self.persistence.lease());
        let joined=join_reference_workers();
        let retired=self.context.user_activity.begin_quiesce(self.persistence.lease());
        let retirement=match retired{Ok(retired)=>{retired.retire();Ok(())},Err(error)=>Err(error)};
        if !std::thread::panicking(){
            assert_eq!(joined.is_err(),self.expected_join_failure,"reference join outcome");
            retirement.expect("fixture activity did not retire");
        }
    }}
    fn png()->Vec<u8>{
        let image=image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2,2,image::Rgba([12,34,56,255])));
        let mut bytes=std::io::Cursor::new(Vec::new());image.write_to(&mut bytes,image::ImageFormat::Png).unwrap();bytes.into_inner()
    }
    fn owned(f:&Fixture)->ReferenceData{
        let authority=f.persistence.storage_authority().unwrap();
        let decoded=decode_reference_bytes(&png()).unwrap();let path=persist_reference_image_for_namespace(&authority,&decoded).unwrap();
        ReferenceData{id:Uuid::new_v4().to_string(),source_path:path.to_str().unwrap().into()}
    }
    fn seed(f:&Fixture,app:&AppWindow)->ReferenceData{
        let item=owned(f);f.context.store.borrow_mut().references.character.push(item.clone());
        f.persistence.save_store(local_store_data(app,&f.context.store.borrow())).unwrap();
        item
    }
    fn legacy_asset(id:&str,path:&Path)->AssetData{
        AssetData{id:id.into(),conversation_id:String::new(),title:"legacy image".into(),category:"other".into(),kind:"game".into(),
            time:String::new(),prompt:String::new(),ratio:"1:1".into(),quality:String::new(),model:String::new(),origin:"legacy".into(),
            width:2,height:2,source_path:path.to_string_lossy().into_owned(),reference_paths:vec![],cutout_done:false,remove_black_done:false,
            upscale_done:false,is_new:false,delivery_recoverable:false,delivery_downloading:false}
    }
    fn saved(f:&Fixture)->LocalStoreData{f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap()}
    fn pump_until(mut predicate:impl FnMut()->bool){
        let end=Instant::now()+Duration::from_secs(5);
        while !predicate() && Instant::now()<end{i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));slint::platform::update_timers_and_animations();std::thread::sleep(Duration::from_millis(2));}
        assert!(predicate(),"reference callback did not reach expected state");
    }
    fn pump_for(duration:Duration){
        let end=Instant::now()+duration;
        while Instant::now()<end{i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));slint::platform::update_timers_and_animations();std::thread::sleep(Duration::from_millis(2));}
    }
    struct Http{url:String,seen:mpsc::Receiver<()>,release:Option<mpsc::Sender<Vec<u8>>>,stop:Arc<AtomicBool>,worker:Option<std::thread::JoinHandle<()>>}
    impl Http{
        fn new(status:u16)->Self{
            let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
            let url=format!("http://{}/reference.png",listener.local_addr().unwrap());let(stop,seen_pair,release_pair)=(Arc::new(AtomicBool::new(false)),mpsc::channel(),mpsc::channel::<Vec<u8>>());
            let cancellation=stop.clone();
            let worker=std::thread::spawn(move||{
                let end=Instant::now()+Duration::from_secs(8);
                let mut stream=loop{if cancellation.load(Ordering::Acquire){return;}
                    match listener.accept(){Ok((stream,_))=>break stream,Err(error) if error.kind()==std::io::ErrorKind::WouldBlock && Instant::now()<end=>std::thread::sleep(Duration::from_millis(2)),Err(error)=>panic!("fixture accept: {error}")}};
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut bytes=Vec::new();let mut block=[0u8;512];
                while !bytes.windows(4).any(|window|window==b"\r\n\r\n"){
                    assert!(bytes.len()<16384);let read=stream.read(&mut block).unwrap();assert!(read>0);bytes.extend_from_slice(&block[..read]);
                }
                assert!(std::str::from_utf8(&bytes).unwrap().starts_with("GET /reference.png "));
                assert!(!std::str::from_utf8(&bytes).unwrap().to_ascii_lowercase().contains("x-token:"));
                seen_pair.0.send(()).unwrap();
                let body=release_pair.1.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
                let _=write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                let _=stream.write_all(&body);
            });
            Self{url,seen:seen_pair.1,release:Some(release_pair.0),stop,worker:Some(worker)}
        }
        fn wait(&self){self.seen.recv_timeout(Duration::from_secs(3)).unwrap();}
        fn reply(&mut self,bytes:Vec<u8>){self.release.take().unwrap().send(bytes).unwrap();}
        fn finish(mut self){self.stop.store(true,Ordering::Release);self.release.take();self.worker.take().unwrap().join().unwrap();}
    }
    impl Drop for Http{fn drop(&mut self){self.stop.store(true,Ordering::Release);self.release.take();if let Some(worker)=self.worker.take(){let joined=worker.join();if !std::thread::panicking(){assert!(joined.is_ok());}}}}
    #[test]
    fn core_reference_asset_picker_saves_owned_reference_and_rejects_retired_account() {
        let (f, app) = fixture(); let item = owned(&f);
        f.context.store.borrow_mut().assets.push(AssetData { id: "picker-asset".into(),
            source_path: item.source_path.clone(), conversation_id: String::new(), title: "Asset".into(),
            category: "character".into(), kind: "game".into(), time: String::new(), prompt: String::new(),
            ratio: "1:1".into(), quality: String::new(), model: String::new(), origin: String::new(),
            width: 2, height: 2, reference_paths: vec![], cutout_done: false, remove_black_done: false,
            upscale_done: false, is_new: false, delivery_recoverable: false, delivery_downloading: false });
        assert!(app.global::<AppState>().invoke_add_reference_from_asset("picker-asset".into()));
        pump_until(|| !f.context.store.borrow().references.character.is_empty());
        drain_reference_test_workers(); pump_for(Duration::from_millis(80));
        let references = saved(&f).references.character;
        assert_eq!(references.len(), 1);
        assert!(f.persistence.owns_path(Path::new(&references[0].source_path)));
        *f.context.active_namespace.lock().unwrap() = None;
        assert!(!app.global::<AppState>().invoke_add_reference_from_asset("picker-asset".into()));
        assert_eq!(f.context.store.borrow().references.character, references);
    }
    #[test]
    fn side_scroll_map_reference_import_appends_like_other_creation_workflows(){
        let(f,app)=fixture();let original=owned(&f);
        app.global::<AppState>().set_page("canvas".into());
        {
            let mut store=f.context.store.borrow_mut();store.active_canvas_workspace_id="side-scroll-map".into();
            store.canvas_references.push(original.clone());
        }
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        let source=f.external.path().join("additional.png");std::fs::write(&source,png()).unwrap();

        assert!(start_reference_paths_for_context(&app,f.context.clone(),vec![source]));
        pump_until(||app.global::<AppState>().get_generation_status().as_str()=="已添加参考图");
        drain_reference_test_workers();pump_for(Duration::from_millis(80));

        let rows=f.context.store.borrow().canvas_references.clone();
        assert_eq!(rows.len(),2);assert_eq!(rows[0],original);assert_ne!(rows[1].id,rows[0].id);
        assert!(f.persistence.owns_path(Path::new(&rows[1].source_path)));
        assert_eq!(saved(&f).canvas_workspaces["side-scroll-map"].references,rows);
    }
    #[test]
    fn side_scroll_map_failed_add_keeps_the_existing_reference(){
        let(f,app)=fixture();let original=owned(&f);
        app.global::<AppState>().set_page("canvas".into());
        {
            let mut store=f.context.store.borrow_mut();store.active_canvas_workspace_id="side-scroll-map".into();
            store.canvas_references.push(original.clone());
        }
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        let source=f.external.path().join("invalid.png");std::fs::write(&source,b"not an image").unwrap();

        assert!(start_reference_paths_for_context(&app,f.context.clone(),vec![source]));
        pump_until(||app.global::<AppState>().get_generation_status().contains("原文件保持不变"));
        drain_reference_test_workers();pump_for(Duration::from_millis(80));

        assert_eq!(f.context.store.borrow().canvas_references,vec![original.clone()]);
        assert_eq!(saved(&f).canvas_workspaces["side-scroll-map"].references,vec![original]);
    }
    #[test]
    fn core_reference_retired_binding_remove_and_clear_do_not_mutate(){
        let(f,app)=fixture();let item=seed(&f,&app);let state=app.global::<AppState>();state.set_generation_status("replacement boundary".into());
        *f.context.active_namespace.lock().unwrap()=None;
        state.invoke_remove_reference(item.id.clone().into());state.invoke_clear_references();
        assert_eq!(f.context.store.borrow().references.character,vec![item.clone()]);
        assert_eq!(state.get_generation_status(),"replacement boundary");assert_eq!(saved(&f).references.character,vec![item]);
    }
    #[test]
    fn core_reference_picker_return_cannot_write_a_different_target(){
        let(f,app)=fixture();let path=f.external.path().join("source.png");std::fs::write(&path,png()).unwrap();
        let pending=Rc::new(RefCell::new(None));let out=pending.clone();
        REFERENCE_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|done|*out.borrow_mut()=Some(done))));
        app.global::<AppState>().invoke_add_reference();assert!(pending.borrow().is_some());
        app.global::<AppState>().set_asset_type("scene".into());app.global::<AppState>().set_generation_status("new target".into());
        pending.borrow_mut().take().unwrap()(vec![path]);drain_reference_test_workers();pump_for(Duration::from_millis(80));
        assert!(f.context.store.borrow().references.character.is_empty());assert!(f.context.store.borrow().references.scene.is_empty());
        assert_eq!(app.global::<AppState>().get_generation_status(),"new target");
    }
    #[test]
    fn core_reference_clipboard_completion_rechecks_original_binding(){
        let(f,app)=fixture();let context=f.context.clone();
        REFERENCE_TEST_CLIPBOARD.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{
            *context.active_namespace.lock().unwrap()=None;
            Some(arboard::ImageData{width:1,height:1,bytes:std::borrow::Cow::Owned(vec![1,2,3,255])})
        })));
        app.global::<AppState>().set_generation_status("retired clipboard".into());let _=app.global::<AppState>().invoke_paste_reference();
        drain_reference_test_workers();pump_for(Duration::from_millis(80));
        assert!(f.context.store.borrow().references.character.is_empty());assert_eq!(app.global::<AppState>().get_generation_status(),"retired clipboard");
    }
    #[test]
    fn core_reference_url_late_result_does_not_cross_category(){
        let mut http=Http::new(200);let(f,app)=fixture();
        start_reference_url_for_context(&app,f.context.clone(),http.url.clone());http.wait();
        app.global::<AppState>().set_asset_type("scene".into());app.global::<AppState>().set_generation_status("new target".into());
        http.reply(png());http.finish();drain_reference_test_workers();pump_for(Duration::from_millis(80));
        assert!(f.context.store.borrow().references.character.is_empty());assert!(f.context.store.borrow().references.scene.is_empty());
        assert_eq!(app.global::<AppState>().get_generation_status(),"new target");
    }
    #[test]
    fn core_reference_url_success_is_owned_indexed_and_saved_before_success(){
        let mut http=Http::new(200);let(f,app)=fixture();
        start_reference_url_for_context(&app,f.context.clone(),http.url.clone());http.wait();http.reply(png());http.finish();
        pump_until(||app.global::<AppState>().get_generation_status().as_str()=="已添加参考图");
        let item=f.context.store.borrow().references.character[0].clone();
        assert!(Path::new(&item.source_path).starts_with(f.persistence.lease().namespace.root()));
        assert_eq!(saved(&f).references.character,vec![item.clone()]);
        let authority=f.persistence.storage_authority().unwrap();
        assert!(f.context.file_index.as_ref().unwrap().find_file_by_path_for_namespace(&authority,
            ManagedUserArea::ReferencesLibrary,Path::new(&item.source_path).file_name().unwrap().to_str().unwrap()).unwrap().is_some());
        assert!(prepare_owned_preview(&f.persistence,Path::new(&item.source_path),PreviewPurpose::Reference).is_ok());
        assert_eq!(app.global::<AppState>().get_references().row_count(),1);
    }
    #[test]
    fn core_reference_exact_upgrade_response_preserves_store_and_late_ui(){
        let mut http=Http::new(426);let(f,app)=fixture();let before=seed(&f,&app);
        start_reference_url_for_context(&app,f.context.clone(),http.url.clone());http.wait();
        http.reply(serde_json::json!({"request_id":"fixture","data":null,"meta":null,"error":{"code":"client_upgrade_required","message":"private","details":null}}).to_string().into_bytes());http.finish();
        drain_reference_test_workers();assert!(f.persistence.upgrade_latch().is_tripped());
        app.global::<AppState>().set_generation_status("upgrade boundary".into());pump_for(Duration::from_millis(80));
        assert_eq!(f.context.store.borrow().references.character,vec![before]);assert_eq!(app.global::<AppState>().get_generation_status(),"upgrade boundary");
    }
    #[test]
    fn core_reference_failed_store_retry_never_replays_clear_over_later_edits(){
        let(f,app)=fixture();let original=seed(&f,&app);
        let root=f.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let connection=rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_reference_save BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'fixture rejected'); END;").unwrap();
        app.global::<AppState>().invoke_clear_references();
        pump_until(||app.global::<AppState>().get_generation_status().contains("重试"));
        assert!(f.context.store.borrow().references.character.is_empty());assert_eq!(saved(&f).references.character,vec![original]);
        // A later valid in-memory edit must survive retry of the already-staged clear.
        let later=owned(&f);f.context.store.borrow_mut().references.character.push(later.clone());
        connection.execute_batch("DROP TRIGGER reject_reference_save").unwrap();
        app.global::<AppState>().invoke_clear_references();
        pump_until(||app.global::<AppState>().get_generation_status().as_str()=="参考图状态已保存");
        assert_eq!(f.context.store.borrow().references.character,vec![later.clone()]);assert_eq!(saved(&f).references.character,vec![later]);
    }

    #[test]
    fn core_reference_normal_clipboard_and_remove_use_owned_acknowledged_state(){
        let(f,app)=fixture();
        REFERENCE_TEST_CLIPBOARD.with(|hook|*hook.borrow_mut()=Some(Box::new(||Some(arboard::ImageData{
            width:1,height:1,bytes:std::borrow::Cow::Owned(vec![10,20,30,255])}))));
        assert!(app.global::<AppState>().invoke_paste_reference());
        pump_until(||app.global::<AppState>().get_generation_status().as_str()=="已添加参考图");
        let row=f.context.store.borrow().references.character[0].clone();assert_eq!(saved(&f).references.character,vec![row.clone()]);
        app.global::<AppState>().invoke_remove_reference(row.id.into());
        pump_until(||app.global::<AppState>().get_generation_status().as_str()=="参考图状态已保存");
        assert!(saved(&f).references.character.is_empty());assert!(Path::new(&row.source_path).is_file(),"removal must not delete owned source bytes");
    }
    #[test]
    fn core_reference_open_preview_cannot_reopen_closed_viewer(){
        let(f,app)=fixture();let item=seed(&f,&app);
        app.global::<AppState>().invoke_open_reference(item.id.into());
        assert!(app.global::<AppState>().get_viewer_open());app.global::<AppState>().set_viewer_open(false);
        drain_reference_test_workers();pump_for(Duration::from_millis(80));
        assert!(!app.global::<AppState>().get_viewer_open());assert_eq!(app.global::<AppState>().get_viewer_image().size().width,0);
    }
    #[cfg(unix)]
    #[test]
    fn core_reference_picker_symlink_replacement_never_imports_source(){
        let(f,app)=fixture();let source=f.external.path().join("source.png");let other=f.external.path().join("other.png");
        std::fs::write(&source,png()).unwrap();std::fs::write(&other,png()).unwrap();
        let pending=Rc::new(RefCell::new(None));let out=pending.clone();
        REFERENCE_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|done|*out.borrow_mut()=Some(done))));
        app.global::<AppState>().invoke_add_reference();
        std::fs::remove_file(&source).unwrap();std::os::unix::fs::symlink(&other,&source).unwrap();
        pending.borrow_mut().take().unwrap()(vec![source]);
        pump_until(||app.global::<AppState>().get_generation_status().contains("重试"));
        assert!(f.context.store.borrow().references.character.is_empty());assert_eq!(std::fs::read(other).unwrap(),png());
    }
    #[test]
    fn core_reference_native_drag_late_prepare_and_retired_entry_never_launch(){
        let(f,app)=fixture();let item=seed(&f,&app);let called=Rc::new(Cell::new(0));let observed=called.clone();
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(move|_drag|{observed.set(observed.get()+1);true})));
        let(sent,seen)=mpsc::channel();let(release,wait)=mpsc::channel();
        REFERENCE_TEST_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{sent.send(()).unwrap();let _=wait.recv_timeout(Duration::from_secs(3));})));
        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(item.source_path.clone().into()));
        seen.recv_timeout(Duration::from_secs(3)).unwrap();*f.context.active_namespace.lock().unwrap()=None;
        release.send(()).unwrap();drain_reference_test_workers();pump_for(Duration::from_millis(80));
        assert_eq!(called.get(),0);
        assert!(!app.global::<AppState>().invoke_start_thumbnail_file_drag(item.source_path.into()));assert_eq!(called.get(),0);
    }
    #[test]
    fn core_reference_native_drag_normal_callback_uses_final_os_seam(){
        let(f,app)=fixture();let item=seed(&f,&app);let called=Rc::new(Cell::new(false));let observed=called.clone();
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(move|_drag|{observed.set(true);true})));
        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(item.source_path.into()));
        pump_until(||called.get());assert!(!app.global::<AppState>().get_thumbnail_drag_preview_visible());
    }
    #[test]
    fn core_reference_native_drag_does_not_wait_for_the_general_worker_poll(){
        let(f,app)=fixture();let item=seed(&f,&app);let called=Rc::new(Cell::new(false));let observed=called.clone();
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(move|_drag|{observed.set(true);true})));
        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(item.source_path.into()));
        drain_reference_test_workers();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(5));
        slint::platform::update_timers_and_animations();
        assert!(called.get(),"native drag still waited for the 40 ms reference-worker poll");
    }
    #[test]
    fn core_native_file_drop_inside_prompt_adds_a_reference(){
        let(f,app)=fixture();let source=f.external.path().join("prompt-drop.png");std::fs::write(&source,png()).unwrap();
        let state=app.global::<AppState>();state.set_reference_drop_x(20.0);state.set_reference_drop_y(30.0);
        state.set_reference_drop_width(400.0);state.set_reference_drop_height(220.0);
        let position=ExternalDropPosition{x:120.0,y:100.0,physical:false};
        assert!(external_drop_inside_reference_input(&app,Some(&position)));

        assert!(start_reference_paths_for_context(&app,f.context.clone(),vec![source.clone()]));
        pump_until(||state.get_generation_status().as_str()=="已添加参考图");
        drain_reference_test_workers();pump_for(Duration::from_millis(80));

        let imported=PathBuf::from(&f.context.store.borrow().references.character[0].source_path);
        assert!(f.persistence.owns_path(&imported));assert_eq!(std::fs::read(imported).unwrap(),png());
        assert_eq!(std::fs::read(source).unwrap(),png());
    }
    #[test]
    fn legacy_visible_asset_drag_migrates_to_current_account_before_os_drag(){
        let(f,app)=fixture();app.global::<AppState>().set_page("assets".into());
        let original=f.external.path().join("legacy-visible.png");std::fs::write(&original,png()).unwrap();
        f.context.store.borrow_mut().assets.push(legacy_asset("legacy-visible",&original));
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        let dragged=Rc::new(RefCell::new(None));let observed=dragged.clone();
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(move|drag|{
            *observed.borrow_mut()=Some(drag.consume(Path::to_path_buf).unwrap());true
        })));

        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(file_uri_for_path(original.to_string_lossy().as_ref()).into()));
        pump_until(||dragged.borrow().is_some());drain_reference_test_workers();pump_for(Duration::from_millis(80));

        let migrated=dragged.borrow().clone().unwrap();assert_ne!(migrated,original);
        assert!(f.persistence.owns_path(&migrated));assert_eq!(std::fs::read(&migrated).unwrap(),png());assert_eq!(std::fs::read(&original).unwrap(),png());
        assert!(crate::directory_migration::same_path(Path::new(&f.context.store.borrow().assets[0].source_path),&migrated));
        assert!(crate::directory_migration::same_path(Path::new(&saved(&f).assets[0].source_path),&migrated));
        let authority=f.persistence.storage_authority().unwrap();
        assert!(f.context.file_index.as_ref().unwrap().find_file_by_path_for_namespace(&authority,ManagedUserArea::Output,
            migrated.file_name().unwrap().to_str().unwrap()).unwrap().is_some());
    }
    #[cfg(windows)]
    #[test]
    fn windows_verbatim_legacy_asset_drag_migrates_before_os_drag(){
        let(f,app)=fixture();app.global::<AppState>().set_page("assets".into());
        let original=f.external.path().join("旧作品.png");std::fs::write(&original,png()).unwrap();
        let verbatim=PathBuf::from(format!(r"\\?\{}",original.display()));
        f.context.store.borrow_mut().assets.push(legacy_asset("legacy-verbatim",&verbatim));
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        let dragged=Rc::new(RefCell::new(None));let observed=dragged.clone();
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(move|drag|{
            *observed.borrow_mut()=Some(drag.consume(Path::to_path_buf).unwrap());true
        })));

        let uri=file_uri_for_path(verbatim.to_string_lossy().as_ref());
        assert!(!uri.contains("%3F"));
        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(uri.into()));
        pump_until(||dragged.borrow().is_some());drain_reference_test_workers();pump_for(Duration::from_millis(80));

        let migrated=dragged.borrow().clone().unwrap();
        assert!(f.persistence.owns_path(&migrated));assert_eq!(std::fs::read(&migrated).unwrap(),png());
        assert_eq!(std::fs::read(&verbatim).unwrap(),png());
        assert!(crate::directory_migration::same_path(Path::new(&saved(&f).assets[0].source_path),&migrated));
    }
    #[test]
    fn legacy_asset_drag_save_failure_keeps_old_path_and_never_starts_os_drag(){
        let(f,app)=fixture();app.global::<AppState>().set_page("assets".into());
        let original=f.external.path().join("legacy-save-failure.png");std::fs::write(&original,png()).unwrap();
        f.context.store.borrow_mut().assets.push(legacy_asset("legacy-save-failure",&original));
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        let root=f.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let connection=rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_legacy_drag BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'fixture rejected'); END;").unwrap();
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(|_|panic!("failed migration started OS drag"))));

        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(file_uri_for_path(original.to_string_lossy().as_ref()).into()));
        pump_until(||app.global::<AppState>().get_generation_status().contains("路径未能安全迁移"));
        drain_reference_test_workers();pump_for(Duration::from_millis(80));

        assert_eq!(f.context.store.borrow().assets[0].source_path,original.to_string_lossy());
        assert_eq!(saved(&f).assets[0].source_path,original.to_string_lossy());assert_eq!(std::fs::read(&original).unwrap(),png());
        let output=f.persistence.lease().namespace.path(ManagedUserArea::Output);
        assert_eq!(std::fs::read_dir(output).unwrap().count(),0);
    }
    #[test]
    fn missing_legacy_asset_drag_never_creates_an_owned_copy(){
        let(f,app)=fixture();app.global::<AppState>().set_page("assets".into());
        let original=f.external.path().join("missing.png");f.context.store.borrow_mut().assets.push(legacy_asset("missing",&original));
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(|_|panic!("missing source started OS drag"))));

        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(file_uri_for_path(original.to_string_lossy().as_ref()).into()));
        pump_until(||app.global::<AppState>().get_generation_status().contains("原图文件不可用"));
        drain_reference_test_workers();

        assert_eq!(f.context.store.borrow().assets[0].source_path,original.to_string_lossy());
        assert_eq!(std::fs::read_dir(f.persistence.lease().namespace.path(ManagedUserArea::Output)).unwrap().count(),0);
    }
    #[test]
    fn core_reference_exact_upgrade_rejects_all_entries_before_native_effect(){
        let(f,app)=fixture();let item=seed(&f,&app);
        f.persistence.upgrade_latch().trip(RequiredUpgrade{minimum_version:Some("99.0.0".into())});
        REFERENCE_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(|_|panic!("dialog after426"))));
        REFERENCE_TEST_CLIPBOARD.with(|hook|*hook.borrow_mut()=Some(Box::new(||panic!("clipboard after426"))));
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(|_|panic!("drag after426"))));
        let state=app.global::<AppState>();state.set_generation_status("upgrade boundary".into());
        state.invoke_add_reference();assert!(!state.invoke_paste_reference());state.invoke_remove_reference(item.id.clone().into());
        state.invoke_clear_references();state.invoke_open_reference(item.id.clone().into());assert!(!state.invoke_start_thumbnail_file_drag(item.source_path.clone().into()));
        assert_eq!(state.get_generation_status(),"upgrade boundary");assert_eq!(f.context.store.borrow().references.character,vec![item]);
    }
    #[test]
    fn core_reference_actual_worker_panic_closes_admission_and_stays_sticky(){
        let(mut f,app)=fixture();f.expected_join_failure=true;
        let capture=ReferenceCapture::new(&app,&f.context).unwrap();
        spawn_reference_work::<()>(&app,f.context.clone(),capture,|_,_,_|panic!("controlled reference panic"),|_,_,_,_|panic!("panic delivered"));
        pump_until(||REFERENCE_JOIN_FAILED.with(|failed|failed.get()));
        assert!(REFERENCE_CLOSED.with(|closed|closed.get()));assert!(REFERENCE_WORKERS.with(|workers|workers.borrow().is_empty()));
        REFERENCE_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(|_|panic!("admitted after worker panic"))));
        app.global::<AppState>().invoke_add_reference();assert!(shutdown_reference_workers().is_err());assert!(join_reference_workers().is_err());
    }
    #[test]
    fn core_reference_shutdown_joins_real_http_without_timer_publication(){
        let mut http=Http::new(200);let(f,app)=fixture();
        start_reference_url_for_context(&app,f.context.clone(),http.url.clone());http.wait();http.reply(png());http.finish();
        shutdown_reference_workers().unwrap();assert!(REFERENCE_WORKERS.with(|workers|workers.borrow().is_empty()));
        app.global::<AppState>().set_generation_status("shutdown boundary".into());pump_for(Duration::from_millis(80));
        assert!(f.context.store.borrow().references.character.is_empty());assert_eq!(app.global::<AppState>().get_generation_status(),"shutdown boundary");
    }
    #[test]
    fn core_reference_real_undelivered_timer_can_drop_after_ui_thread_tls(){
        // Isolate this exact libtest for RED/first execution: TLS destructor panic can abort.
        let worker=std::thread::spawn(||{
            let mut http=Http::new(200);let(f,app)=fixture();
            start_reference_url_for_context(&app,f.context.clone(),http.url.clone());http.wait();http.reply(png());http.finish();
            drain_reference_test_workers();assert!(REFERENCE_WORKERS.with(|workers|workers.borrow().is_empty()));
            assert!(f.context.store.borrow().references.character.is_empty());
            drop(app);drop(f); // Do not dispatch the real Slint poll before TLS teardown.
        });
        worker.join().unwrap();
    }

    #[test]
    fn core_reference_hidden_save_failure_is_retryable_without_replaying_mutation(){
        let(f,app)=fixture();let original=seed(&f,&app);
        let root=f.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let connection=rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_hidden_reference BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'fixture rejected'); END;").unwrap();
        app.global::<AppState>().invoke_clear_references();
        let revision=REFERENCE_UI.with(|ui|ui.borrow().saves.iter().find(|save|
            save.lease==*f.persistence.lease() && save.target==ReferenceTarget::Category("character".into())
        ).expect("original checked save must be queued").revision);
        app.global::<AppState>().set_asset_type("scene".into());app.global::<AppState>().set_generation_status("visible scene".into());
        pump_until(||REFERENCE_UI.with(|ui|ui.borrow().saves.iter().any(|save|
            save.lease==*f.persistence.lease() && save.target==ReferenceTarget::Category("character".into())
                && save.revision==revision && save.retry)));
        assert_eq!(app.global::<AppState>().get_generation_status(),"visible scene");
        assert_eq!(saved(&f).references.character,vec![original]);
        let later=owned(&f);f.context.store.borrow_mut().references.character.push(later.clone());
        connection.execute_batch("DROP TRIGGER reject_hidden_reference").unwrap();app.global::<AppState>().set_asset_type("character".into());
        app.global::<AppState>().invoke_clear_references();
        pump_until(||app.global::<AppState>().get_generation_status().as_str()=="参考图状态已保存");
        assert_eq!(f.context.store.borrow().references.character,vec![later.clone()]);assert_eq!(saved(&f).references.character,vec![later]);
    }
    #[test]
    fn core_reference_weak_window_does_not_join_live_worker_on_ui(){
        let mut http=Http::new(200);let(f,app)=fixture();let(sent,seen)=mpsc::channel();let(release,wait)=mpsc::channel();
        REFERENCE_TEST_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{sent.send(()).unwrap();let _=wait.recv_timeout(Duration::from_secs(3));})));
        start_reference_url_for_context(&app,f.context.clone(),http.url.clone());http.wait();http.reply(png());seen.recv_timeout(Duration::from_secs(3)).unwrap();http.finish();
        let(progress,advanced)=mpsc::channel();let release=UiRelease::new(advanced,release);
        slint::Timer::single_shot(Duration::from_millis(60),move||{let _=progress.send(());});drop(app);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(20));slint::platform::update_timers_and_animations();
        assert!(release.finish(),"weak-window disposal joined live reference work on UI");drain_reference_test_workers();
        assert!(f.context.store.borrow().references.character.is_empty());assert!(REFERENCE_WORKERS.with(|workers|workers.borrow().is_empty()));
    }

    #[test]
    fn core_reference_clear_invalidates_earlier_held_import_without_readding(){
        let mut http=Http::new(200);let(f,app)=fixture();let _original=seed(&f,&app);
        start_reference_url_for_context(&app,f.context.clone(),http.url.clone());http.wait();
        app.global::<AppState>().invoke_clear_references();
        pump_until(||app.global::<AppState>().get_generation_status().as_str()=="参考图状态已保存");
        http.reply(png());http.finish();drain_reference_test_workers();pump_for(Duration::from_millis(80));
        assert!(f.context.store.borrow().references.character.is_empty());assert!(saved(&f).references.character.is_empty());
        assert_eq!(app.global::<AppState>().get_generation_status(),"参考图状态已保存");
    }

    // Hold the actual worker only after it has prepared the held source and sent it.
    // The owner of release is declared after Fixture, so unwind releases before join.
    struct NativePreparationHold{ready:mpsc::Receiver<()>,release:Option<mpsc::Sender<()>>}
    impl NativePreparationHold{
        fn new()->(Self,Box<dyn FnOnce()+Send>){
            let(sent,ready)=mpsc::channel();let(release,wait)=mpsc::channel();
            (Self{ready,release:Some(release)},Box::new(move||{
                let _=sent.send(());let _=wait.recv_timeout(Duration::from_secs(3));
            }))
        }
        fn wait(&self){self.ready.recv_timeout(Duration::from_secs(3)).unwrap();}
        fn release(&mut self){if let Some(release)=self.release.take(){let _=release.send(());}}
    }
    impl Drop for NativePreparationHold{fn drop(&mut self){self.release();}}
    fn viewer_native_target_change(replace:bool) {
        let(f,app)=fixture();let original=owned(&f);
        f.context.store.borrow_mut().assets.push(AssetData {
            id:original.id.clone(),conversation_id:String::new(),title:"original viewer".into(),category:"other".into(),kind:"game".into(),
            time:String::new(),prompt:String::new(),ratio:"1:1".into(),quality:String::new(),model:String::new(),origin:String::new(),
            width:2,height:2,source_path:original.source_path.clone(),reference_paths:vec![],cutout_done:false,remove_black_done:false,
            upscale_done:false,is_new:false,delivery_recoverable:false,delivery_downloading:false,
        });
        wire_viewer_callbacks(&app,f.context.clone());
        let state=app.global::<AppState>();state.set_viewer_open(true);state.set_viewer_source("asset".into());
        state.set_viewer_id(original.id.into());state.set_viewer_source_path(original.source_path.into());
        let(mut held,hook)=NativePreparationHold::new();
        REFERENCE_TEST_AFTER_SEND.with(|after|*after.borrow_mut()=Some(hook));
        let calls=Rc::new(Cell::new(0));let observed=calls.clone();
        REFERENCE_TEST_FILE_DRAG.with(|effect|*effect.borrow_mut()=Some(Box::new(move|_|{observed.set(observed.get()+1);true})));
        let exits=Rc::new(Cell::new(0));let observed=exits.clone();
        REFERENCE_TEST_POINTER_EXIT.with(|effect|*effect.borrow_mut()=Some(Box::new(move||observed.set(observed.get()+1))));
        assert!(state.invoke_start_viewer_file_drag());held.wait();
        if replace {
            let next=seed(&f,&app);state.invoke_open_reference(next.id.into());
        } else {state.invoke_close_viewer();}
        state.set_thumbnail_drag_preview_visible(true);
        held.release();drain_reference_test_workers();pump_for(Duration::from_millis(80));
        assert_eq!(calls.get(),0,"original viewer drag launched after its view was replaced/closed");
        assert_eq!(exits.get(),0,"original viewer pointer reset reached the replacement view");
        assert!(state.get_thumbnail_drag_preview_visible());
    }
    #[test]
    fn core_viewer_close_rejects_held_original_native_drag() {viewer_native_target_change(false);}
    #[test]
    fn core_viewer_replacement_rejects_held_original_native_drag() {viewer_native_target_change(true);}
    #[test]
    fn core_viewer_queued_drag_rechecks_presentation_at_actual_consume() {
        let(f,app)=fixture();let item=seed(&f,&app);
        let state=app.global::<AppState>();state.set_viewer_open(true);state.set_viewer_source("reference".into());
        state.set_viewer_id(item.id.into());state.set_viewer_source_path(item.source_path.clone().into());
        let queued:Rc<RefCell<Option<CapturedNativeFileDrag>>>=Rc::new(RefCell::new(None));let captured=queued.clone();
        REFERENCE_TEST_FILE_DRAG.with(|effect|*effect.borrow_mut()=Some(Box::new(move|drag|{*captured.borrow_mut()=Some(drag);true})));
        assert!(state.invoke_start_thumbnail_file_drag(item.source_path.into()));
        pump_until(||queued.borrow().is_some());drain_reference_test_workers();
        state.set_viewer_open(false);
        let calls=Cell::new(0);
        let result=queued.borrow_mut().take().unwrap().consume(|_|calls.set(calls.get()+1));
        assert!(result.is_err(),"queued native payload consumed after original viewer closed");
        assert_eq!(calls.get(),0);
    }
    #[test]
    fn core_reference_page_drag_held_before_viewer_open_never_launches_into_viewer() {
        let(f,app)=fixture();let item=seed(&f,&app);let state=app.global::<AppState>();
        assert!(!state.get_viewer_open());
        let(mut held,hook)=NativePreparationHold::new();
        REFERENCE_TEST_AFTER_SEND.with(|after|*after.borrow_mut()=Some(hook));
        let calls=Rc::new(Cell::new(0));let observed=calls.clone();
        REFERENCE_TEST_FILE_DRAG.with(|effect|*effect.borrow_mut()=Some(Box::new(move|_|{observed.set(observed.get()+1);true})));
        let exits=Rc::new(Cell::new(0));let observed=exits.clone();
        REFERENCE_TEST_POINTER_EXIT.with(|effect|*effect.borrow_mut()=Some(Box::new(move||observed.set(observed.get()+1))));
        assert!(state.invoke_start_thumbnail_file_drag(item.source_path.into()));held.wait();
        state.invoke_open_reference(item.id.into());state.set_thumbnail_drag_preview_visible(true);
        held.release();drain_reference_test_workers();pump_for(Duration::from_millis(80));
        assert_eq!(calls.get(),0,"page drag launched after viewer became active");
        assert_eq!(exits.get(),0);assert!(state.get_thumbnail_drag_preview_visible());
    }
    #[test]
    fn core_reference_queued_page_drag_denies_actual_consume_after_viewer_open() {
        let(f,app)=fixture();let item=seed(&f,&app);let state=app.global::<AppState>();
        assert!(!state.get_viewer_open());
        let queued:Rc<RefCell<Option<CapturedNativeFileDrag>>>=Rc::new(RefCell::new(None));let captured=queued.clone();
        REFERENCE_TEST_FILE_DRAG.with(|effect|*effect.borrow_mut()=Some(Box::new(move|drag|{*captured.borrow_mut()=Some(drag);true})));
        assert!(state.invoke_start_thumbnail_file_drag(item.source_path.into()));
        pump_until(||queued.borrow().is_some());drain_reference_test_workers();
        state.invoke_open_reference(item.id.into());
        let calls=Cell::new(0);let result=queued.borrow_mut().take().unwrap().consume(|_|calls.set(calls.get()+1));
        assert!(result.is_err(),"queued page payload crossed into a newly opened viewer");assert_eq!(calls.get(),0);
        drain_reference_test_workers();
    }
    #[test]
    fn core_reference_new_native_request_rejects_older_prepared_completion(){
        let(f,app)=fixture();let item=seed(&f,&app);
        let(mut old,old_hook)=NativePreparationHold::new();let(mut new,new_hook)=NativePreparationHold::new();
        REFERENCE_TEST_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(old_hook));
        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(item.source_path.clone().into()));old.wait();
        REFERENCE_TEST_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(new_hook));
        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(item.source_path.into()));new.wait();
        let calls=Rc::new(Cell::new(0));let observed=calls.clone();
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(move|_|{observed.set(observed.get()+1);true})));
        new.release();pump_until(||calls.get()==1);
        let observed=calls.clone();
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(move|_|{observed.set(observed.get()+1);true})));
        old.release();drain_reference_test_workers();pump_for(Duration::from_millis(80));
        assert_eq!(calls.get(),1,"older held-source completion launched after a newer request");
    }
    #[test]
    fn core_reference_reentrant_native_request_preserves_new_pointer_state(){
        let(f,app)=fixture();let item=seed(&f,&app);
        let(mut new,new_hook)=NativePreparationHold::new();
        let weak=app.as_weak();let path=item.source_path.clone();let launched=Rc::new(Cell::new(false));let observed=launched.clone();
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(move|_|{
            let app=weak.upgrade().unwrap();
            REFERENCE_TEST_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(new_hook));
            assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(path.into()));
            app.global::<AppState>().set_thumbnail_drag_preview_visible(true);observed.set(true);true
        })));
        let exits=Rc::new(Cell::new(0));let observed=exits.clone();
        REFERENCE_TEST_POINTER_EXIT.with(|hook|*hook.borrow_mut()=Some(Box::new(move||observed.set(observed.get()+1))));
        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(item.source_path.into()));
        pump_until(||launched.get());new.wait();pump_for(Duration::from_millis(80));
        assert!(app.global::<AppState>().get_thumbnail_drag_preview_visible(),"old drag reset the newer request");
        assert_eq!(exits.get(),0,"old drag dispatched PointerExited for a newer request");
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(|_|true)));
        new.release();drain_reference_test_workers();pump_for(Duration::from_millis(80));
    }
    #[test]
    fn core_reference_zero_timer_reset_rejects_later_native_request(){
        let(f,app)=fixture();let item=seed(&f,&app);
        let(mut new,new_hook)=NativePreparationHold::new();
        let completed=Rc::new(Cell::new(false));let observed=completed.clone();
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(move|_|{
            observed.set(true);true
        })));
        let exits=Rc::new(Cell::new(0));let observed=exits.clone();
        REFERENCE_TEST_POINTER_EXIT.with(|hook|*hook.borrow_mut()=Some(Box::new(move||observed.set(observed.get()+1))));
        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(item.source_path.clone().into()));
        let original=REFERENCE_UI.with(|ui|ui.borrow().native.as_ref().unwrap().id);
        // mock_elapsed_time already dispatches one expired-timer snapshot.
        // Stop after A's real callback queues its reset; do not dispatch that
        // newly queued timer by calling update_timers_and_animations here.
        let end=Instant::now()+Duration::from_secs(5);
        while !completed.get() && Instant::now()<end {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(completed.get(),"original native callback did not complete");
        assert_eq!(exits.get(),0,"original reset must still be queued");
        REFERENCE_TEST_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(new_hook));
        assert!(app.global::<AppState>().invoke_start_thumbnail_file_drag(item.source_path.into()));
        let replacement=REFERENCE_UI.with(|ui|ui.borrow().native.as_ref().unwrap().id);
        assert_ne!(original,replacement,"B must own a different real request");
        app.global::<AppState>().set_thumbnail_drag_preview_visible(true);
        new.wait();pump_for(Duration::from_millis(80));
        assert_eq!(exits.get(),0,"old zero-timer dispatched a pointer effect after request B");
        assert!(app.global::<AppState>().get_thumbnail_drag_preview_visible());
        REFERENCE_TEST_FILE_DRAG.with(|hook|*hook.borrow_mut()=Some(Box::new(|_|true)));
        new.release();drain_reference_test_workers();pump_for(Duration::from_millis(80));
    }

    struct UiRelease(Option<std::thread::JoinHandle<bool>>);
    impl UiRelease{
        fn new(progress:mpsc::Receiver<()>,release:mpsc::Sender<()>)->Self{Self(Some(std::thread::spawn(move||{let ok=progress.recv_timeout(Duration::from_millis(250)).is_ok();let _=release.send(());ok})))}
        fn finish(mut self)->bool{self.0.take().unwrap().join().unwrap()}
    }
    impl Drop for UiRelease{fn drop(&mut self){if let Some(worker)=self.0.take(){let _=worker.join();}}}
    #[test]
    fn core_reference_result_before_worker_exit_does_not_block_ui(){
        let mut http=Http::new(200);let(f,app)=fixture();let(sent,seen)=mpsc::channel();let(release,wait)=mpsc::channel();
        REFERENCE_TEST_AFTER_SEND.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{sent.send(()).unwrap();let _=wait.recv_timeout(Duration::from_secs(3));})));
        start_reference_url_for_context(&app,f.context.clone(),http.url.clone());http.wait();http.reply(png());seen.recv_timeout(Duration::from_secs(3)).unwrap();http.finish();
        let(progress,advanced)=mpsc::channel();let release=UiRelease::new(advanced,release);
        slint::Timer::single_shot(Duration::from_millis(60),move||{let _=progress.send(());});
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));slint::platform::update_timers_and_animations();
        let early=f.context.store.borrow().references.character.len();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(20));slint::platform::update_timers_and_animations();
        assert!(release.finish(),"poll joined a still-running reference worker on UI");assert_eq!(early,0);
        drain_reference_test_workers();
    }
}
