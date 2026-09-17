use super::*;

struct DeliveryCommitWorker {
    lease:NamespaceLease,cancel:Arc<std::sync::atomic::AtomicBool>,handle:std::thread::JoinHandle<()>,
}
thread_local! {
    static DELIVERY_COMMIT_WORKERS:RefCell<Vec<DeliveryCommitWorker>>=const {RefCell::new(Vec::new())};
    static DELIVERY_COMMIT_CLOSING:Cell<bool>=const {Cell::new(false)};
    static DELIVERY_COMMIT_FAILED:Cell<bool>=const {Cell::new(false)};
}
#[cfg(test)]
thread_local! {
    static DELIVERY_PREPARATION_AFTER_SEND:RefCell<Option<Box<dyn FnOnce()+Send>>>=const {RefCell::new(None)};
}
/// Test-only final worker boundary, consumed by the next real preparation spawn.
#[cfg(test)]
pub(super) fn set_delivery_preparation_after_send_for_test(hook:impl FnOnce()+Send+'static) {
    DELIVERY_PREPARATION_AFTER_SEND.with(|slot| {
        assert!(slot.borrow().is_none(),"delivery after-send hook already installed");
        *slot.borrow_mut()=Some(Box::new(hook));
    });
}
pub(super) fn cancel_delivery_commit_workers(lease:&NamespaceLease) {
    DELIVERY_COMMIT_WORKERS.with(|workers|for worker in workers.borrow().iter().filter(|worker|&worker.lease==lease) {
        worker.cancel.store(true,Ordering::SeqCst);
    });
}
fn reap_delivery_commit_workers() {
    let ready=DELIVERY_COMMIT_WORKERS.with(|workers|{
        let mut workers=workers.borrow_mut();let mut ready=Vec::new();let mut index=0;
        while index<workers.len() {if workers[index].handle.is_finished(){ready.push(workers.swap_remove(index));}else{index+=1;}}
        ready
    });
    for worker in ready {if worker.handle.join().is_err(){DELIVERY_COMMIT_FAILED.with(|failed|failed.set(true));}}
}

pub(super) fn spawn_delivery_preparation<T:Send+'static>(
    persistence:&PrivatePersistence,
    work:impl FnOnce(&PrivatePersistence,&UserActivityPermit,&Arc<std::sync::atomic::AtomicBool>)
        -> std::result::Result<T,DeliveryRetryError> + Send+'static,
)->Result<(Arc<std::sync::atomic::AtomicBool>,mpsc::Receiver<std::result::Result<T,DeliveryRetryError>>)> {
    reap_delivery_commit_workers();
    anyhow::ensure!(!DELIVERY_COMMIT_CLOSING.with(Cell::get),"delivery worker admission closed");
    anyhow::ensure!(!DELIVERY_COMMIT_FAILED.with(Cell::get),"delivery worker previously failed");
    let activity=persistence.begin_activity()?;
    let captured=persistence.clone();
    let cancel=Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_cancel=cancel.clone();
    let (sender,receiver)=mpsc::channel();
    #[cfg(test)]
    let after_send=DELIVERY_PREPARATION_AFTER_SEND.with(|slot|slot.borrow_mut().take());
    let handle=std::thread::Builder::new().name("delivery-prepare".into()).spawn(move||{
        let result=if worker_cancel.load(Ordering::SeqCst) || activity.is_quiescing() || !captured.is_current() {
            Err(DeliveryRetryError::AuthenticationRequired)
        } else { work(&captured,&activity,&worker_cancel) };
        let _=sender.send(result);
        #[cfg(test)]
        if let Some(after_send)=after_send {after_send();}
        drop(activity);
    })?;
    DELIVERY_COMMIT_WORKERS.with(|workers|workers.borrow_mut().push(DeliveryCommitWorker {
        lease:persistence.lease().clone(),cancel:cancel.clone(),handle,
    }));
    Ok((cancel,receiver))
}
pub(super) fn delivery_preparation_pending(cancel:&Arc<std::sync::atomic::AtomicBool>)->bool {
    reap_delivery_commit_workers();
    DELIVERY_COMMIT_WORKERS.with(|workers|workers.borrow().iter().any(|worker|Arc::ptr_eq(&worker.cancel,cancel)))
}
/// A sent payload is not a completed worker. Call outside short completion;
/// only Ok(false) permits consuming a success. Sticky failure survives reaping.
pub(super) fn finish_delivery_preparation(cancel:&Arc<std::sync::atomic::AtomicBool>)->Result<bool> {
    reap_delivery_commit_workers();
    anyhow::ensure!(!DELIVERY_COMMIT_FAILED.with(Cell::get),"delivery worker previously failed");
    Ok(DELIVERY_COMMIT_WORKERS.with(|workers|workers.borrow().iter().any(|worker|Arc::ptr_eq(&worker.cancel,cancel))))
}
pub(super) fn drain_delivery_commit_workers_for_shutdown()->Result<()> {
    DELIVERY_COMMIT_CLOSING.with(|closing|closing.set(true));
    let workers=DELIVERY_COMMIT_WORKERS.with(|workers|std::mem::take(&mut *workers.borrow_mut()));
    for worker in &workers {worker.cancel.store(true,Ordering::SeqCst);}
    for worker in workers {if worker.handle.join().is_err(){DELIVERY_COMMIT_FAILED.with(|failed|failed.set(true));}}
    anyhow::ensure!(!DELIVERY_COMMIT_FAILED.with(Cell::get),"delivery commit worker failed");
    Ok(())
}
#[cfg(test)]
pub(super) fn drain_delivery_commit_workers_for_lease_for_test(lease:&NamespaceLease)->Result<()> {
    let workers=DELIVERY_COMMIT_WORKERS.with(|workers|{
        let mut workers=workers.borrow_mut();let mut matching=Vec::new();let mut index=0;
        while index<workers.len(){
            if &workers[index].lease==lease {matching.push(workers.swap_remove(index));}else{index+=1;}
        }
        matching
    });
    for worker in &workers {worker.cancel.store(true,Ordering::SeqCst);}
    for worker in workers {if worker.handle.join().is_err(){DELIVERY_COMMIT_FAILED.with(|failed|failed.set(true));}}
    anyhow::ensure!(!DELIVERY_COMMIT_FAILED.with(Cell::get),"delivery commit worker failed");
    Ok(())
}
/// Completion is UI-only and original-lease guarded; the result distinguishes
/// actual local acknowledgment from remote acknowledgment without deleting either intent.
pub(super) fn start_image_delivery_commit(
    app:&AppWindow,context:AppContext,prepared:PreparedNamespaceDelivery,time:String,
    complete:impl FnOnce(&AppWindow,Result<(Image,String,bool)>)+'static,
) {
    start_image_delivery_commit_with_binding(app,context,None,prepared,time,complete);
}
pub(super) fn start_image_delivery_commit_captured(
    app:&AppWindow,context:AppContext,persistence:PrivatePersistence,prepared:PreparedNamespaceDelivery,time:String,
    complete:impl FnOnce(&AppWindow,Result<(Image,String,bool)>)+'static,
) {
    start_image_delivery_commit_with_binding(app,context,Some(persistence),prepared,time,complete);
}
fn start_image_delivery_commit_with_binding(
    app:&AppWindow,context:AppContext,expected:Option<PrivatePersistence>,prepared:PreparedNamespaceDelivery,time:String,
    complete:impl FnOnce(&AppWindow,Result<(Image,String,bool)>)+'static,
) {
    let lease=prepared.lease().clone();
    let mut complete=Some(complete);
    let prepared_write=(||->Result<_>{
        reap_delivery_commit_workers();
        anyhow::ensure!(!DELIVERY_COMMIT_CLOSING.with(Cell::get),"delivery worker admission closed");
        anyhow::ensure!(!DELIVERY_COMMIT_FAILED.with(Cell::get),"delivery worker previously failed");
        let persistence=context.store.borrow().private_persistence.clone().ok_or_else(||anyhow!("Store not activated"))?;
        anyhow::ensure!(persistence.lease()==&lease && persistence.is_current()
            && expected.as_ref().is_none_or(|original|original.same_binding_metadata(&persistence)),"delivery Store changed");
        let write=persistence.prepare_ordered_save()?;
        let activity=persistence.begin_activity()?;
        Ok((persistence,write,activity))
    })();
    let (persistence,write,activity)=match prepared_write {
        Ok(value)=>value,
        Err(error)=>{
            let _=context.apply_user_completion(&lease,||{
                if expected.as_ref().is_none_or(|original|context.store.borrow().private_persistence.as_ref()
                    .is_some_and(|current|current.same_binding_metadata(original))){
                    complete.take().unwrap()(app,Err(error));
                }
            });
            return;
        }
    };
    let canvas_target=(!prepared.record().canvas_source_node_id.is_empty()).then(||
        (prepared.record().canvas_source_node_id.clone(),prepared.record().local_task_id.clone()));
    let image=materialize_delivery_preview(prepared.preview());
    let mut write=Some(write);let mut prepared=Some(prepared);
    let outcome=context.apply_user_completion(&lease,||{
        if !context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.same_binding_metadata(&persistence)){return None;}
        let history=canvas_target.as_ref().and_then(|(source,_)|{
            let store=context.store.borrow();
            (store.canvas_notes.iter().any(|note|note.id==*source)
                && !store.assets.iter().any(|asset|asset.id==prepared.as_ref().unwrap().confirmation().file_id))
                .then(||CanvasSnapshot{notes:store.canvas_notes.clone(),links:store.canvas_links.clone()})
        });
        let asset_id=prepared.as_ref().unwrap().confirmation().file_id.clone();
        let outcome=write.take().unwrap().enqueue_delivery(app,&mut context.store.borrow_mut(),prepared.take().unwrap(),&time);
        if let Some(history)=history {
            if context.store.borrow().assets.iter().any(|asset|asset.id==asset_id){context.canvas_history.borrow_mut().record(history);}
        }
        Some(outcome)
    });
    drop(write);drop(prepared);
    let pending=match outcome.ok().flatten() {
        Some(Ok(pending))=>pending,
        Some(Err(error))=>{
            let message=error.to_string();drop(error);drop(activity);
            let _=context.apply_user_completion(&lease,||complete.take().unwrap()(app,Err(anyhow!(message))));
            return;
        },
        None=>return,
    };
    let id=pending.asset_id().to_owned();
    let cancel=Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_cancel=cancel.clone();
    let captured=persistence.clone();
    let (sender,receiver)=mpsc::channel();
    let spawned=std::thread::Builder::new().name("delivery-store-ack".into()).spawn(move||{
        let result:std::result::Result<bool,DeliveryRetryError>=pending.wait().map_err(DeliveryRetryError::from).and_then(|receipt|{
            if worker_cancel.load(Ordering::SeqCst) || activity.is_quiescing() || !captured.is_current() {return Ok(false);}
            match acknowledge_namespace_delivery(receipt) {
                Ok(acknowledged)=>Ok(acknowledged),
                Err(DeliveryRetryError::Api(error)) if error.is_terminal_session_error()=>Err(error.into()),
                Err(_)=>Ok(false), // Local save remains durable; remote retry retains original row.
            }
        });
        let _=sender.send(result);
        drop(activity);
    });
    match spawned {
        Ok(handle)=>{
            DELIVERY_COMMIT_WORKERS.with(|workers|workers.borrow_mut().push(DeliveryCommitWorker {lease:lease.clone(),cancel:cancel.clone(),handle}));
            poll_image_delivery_commit(app.as_weak(),context,persistence,cancel,receiver,image,id,canvas_target,complete.take().unwrap());
        },
        Err(error)=>{
            // Dropping the receiver never releases the queued writer command's guards.
            let _=context.apply_user_completion(&lease,||complete.take().unwrap()(app,Err(anyhow!(error))));
        },
    }
}
pub(super) fn start_video_delivery_commit_with_binding(
    app: &AppWindow,
    context: AppContext,
    expected: Option<PrivatePersistence>,
    prepared: PreparedNamespaceVideoDelivery,
    time: String,
    complete: impl FnOnce(&AppWindow, Result<(Image, String, bool)>) + 'static,
) {
    let lease = prepared.lease().clone();
    let mut complete = Some(complete);
    let prepared_write = (|| -> Result<_> {
        reap_delivery_commit_workers();
        anyhow::ensure!(
            !DELIVERY_COMMIT_CLOSING.with(Cell::get),
            "delivery worker admission closed"
        );
        anyhow::ensure!(
            !DELIVERY_COMMIT_FAILED.with(Cell::get),
            "delivery worker previously failed"
        );
        let persistence = context
            .store
            .borrow()
            .private_persistence
            .clone()
            .ok_or_else(|| anyhow!("Store not activated"))?;
        anyhow::ensure!(
            persistence.lease() == &lease
                && persistence.is_current()
                && expected
                    .as_ref()
                    .is_none_or(|original| original.same_binding_metadata(&persistence)),
            "delivery Store changed"
        );
        let write = persistence.prepare_ordered_save()?;
        let activity = persistence.begin_activity()?;
        Ok((persistence, write, activity))
    })();
    let (persistence, write, activity) = match prepared_write {
        Ok(value) => value,
        Err(error) => {
            let _ = context.apply_user_completion(&lease, || {
                if expected.as_ref().is_none_or(|original| {
                    context
                        .store
                        .borrow()
                        .private_persistence
                        .as_ref()
                        .is_some_and(|current| current.same_binding_metadata(original))
                }) {
                    complete.take().unwrap()(app, Err(error));
                }
            });
            return;
        }
    };
    let canvas_target = (!prepared.record().canvas_source_node_id.is_empty()).then(|| {
        (
            prepared.record().canvas_source_node_id.clone(),
            prepared.record().local_task_id.clone(),
        )
    });
    let image = Image::default();
    let mut write = Some(write);
    let mut prepared = Some(prepared);
    let outcome = context.apply_user_completion(&lease, || {
        if !context
            .store
            .borrow()
            .private_persistence
            .as_ref()
            .is_some_and(|current| current.same_binding_metadata(&persistence))
        {
            return None;
        }
        let history = canvas_target.as_ref().and_then(|(source, _)| {
            let store = context.store.borrow();
            (store.canvas_notes.iter().any(|note| note.id == *source)
                && !store
                    .assets
                    .iter()
                    .any(|asset| asset.id == prepared.as_ref().unwrap().confirmation().file_id))
            .then(|| CanvasSnapshot {
                notes: store.canvas_notes.clone(),
                links: store.canvas_links.clone(),
            })
        });
        let asset_id = prepared.as_ref().unwrap().confirmation().file_id.clone();
        let outcome = write.take().unwrap().enqueue_video_delivery(
            app,
            &mut context.store.borrow_mut(),
            prepared.take().unwrap(),
            &time,
        );
        if let Some(history) = history {
            if context
                .store
                .borrow()
                .assets
                .iter()
                .any(|asset| asset.id == asset_id)
            {
                context.canvas_history.borrow_mut().record(history);
            }
        }
        Some(outcome)
    });
    drop(write);
    drop(prepared);
    let pending = match outcome.ok().flatten() {
        Some(Ok(pending)) => pending,
        Some(Err(error)) => {
            let message = error.to_string();
            drop(error);
            drop(activity);
            let _ = context.apply_user_completion(&lease, || {
                complete.take().unwrap()(app, Err(anyhow!(message)))
            });
            return;
        }
        None => return,
    };
    let id = pending.asset_id().to_owned();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let captured = persistence.clone();
    let (sender, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("delivery-store-ack".into())
        .spawn(move || {
            let result: std::result::Result<bool, DeliveryRetryError> = pending
                .wait()
                .map_err(DeliveryRetryError::from)
                .and_then(|receipt| {
                    if worker_cancel.load(Ordering::SeqCst)
                        || activity.is_quiescing()
                        || !captured.is_current()
                    {
                        return Ok(false);
                    }
                    match acknowledge_namespace_delivery(receipt) {
                        Ok(acknowledged) => Ok(acknowledged),
                        Err(DeliveryRetryError::Api(error))
                            if error.is_terminal_session_error() =>
                        {
                            Err(error.into())
                        }
                        Err(_) => Ok(false), // Local save remains durable; remote retry retains original row.
                    }
                });
            let _ = sender.send(result);
            drop(activity);
        });
    match spawned {
        Ok(handle) => {
            DELIVERY_COMMIT_WORKERS.with(|workers| {
                workers.borrow_mut().push(DeliveryCommitWorker {
                    lease: lease.clone(),
                    cancel: cancel.clone(),
                    handle,
                })
            });
            poll_image_delivery_commit(
                app.as_weak(),
                context,
                persistence,
                cancel,
                receiver,
                image,
                id,
                canvas_target,
                complete.take().unwrap(),
            );
        }
        Err(error) => {
            // Dropping the receiver never releases the queued writer command's guards.
            let _ = context.apply_user_completion(&lease, || {
                complete.take().unwrap()(app, Err(anyhow!(error)))
            });
        }
    }
}

fn poll_image_delivery_commit(
    weak:Weak<AppWindow>,context:AppContext,persistence:PrivatePersistence,cancel:Arc<std::sync::atomic::AtomicBool>,
    receiver:mpsc::Receiver<std::result::Result<bool,DeliveryRetryError>>,image:Image,id:String,canvas_target:Option<(String,String)>,
    complete:impl FnOnce(&AppWindow,Result<(Image,String,bool)>)+'static,
) {
    slint::Timer::single_shot(Duration::from_millis(50),move||{
        let finished=finish_delivery_preparation(&cancel);
        if matches!(finished,Ok(true)) {
            poll_image_delivery_commit(weak,context,persistence,cancel,receiver,image,id,canvas_target,complete);return;
        }
        let result=if let Err(error)=finished {Err(DeliveryRetryError::from(error))} else {match receiver.try_recv() {
            Ok(result)=>result,
            Err(TryRecvError::Empty)=>{poll_image_delivery_commit(weak,context,persistence,cancel,receiver,image,id,canvas_target,complete);return;},
            Err(TryRecvError::Disconnected)=>Err(anyhow!("delivery commit worker disconnected").into()),
        }};
        let Some(app)=weak.upgrade()else{return;};
        let bound=context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.same_binding_metadata(&persistence));
        if matches!(&result,Err(DeliveryRetryError::Api(error)) if error.is_terminal_session_error()) {
            let original=SessionScope{owner_user_id:persistence.lease().namespace.user_public_id().into(),auth_epoch:persistence.lease().auth_epoch};
            if bound && terminal_auth_scope_matches_context(&context,&original){drop(result);sign_out_locally(&app,&context,true,Some(original.auth_epoch));}
            return;
        }
        if !bound || !persistence.is_current(){return;}
        let mut effects=None;let mut canvas_effects=None;
        let canvas_visuals=canvas_target.as_ref().filter(|_|result.is_ok()).map(|_|prepare_canvas_projection(&app,&context.store.borrow()));
        let visuals=result.as_ref().ok().map(|_|prepare_delivery_visuals(&app,&context.store.borrow()));
        let _=context.apply_user_completion(persistence.lease(),||{
            if let Some(visuals)=visuals {
                effects=Some(visuals.publish_metadata(&app,persistence.clone()));
                push_notifications(&app,&context.store.borrow());
                push_prompt_history(&app,&context.store.borrow());
            }
            if let Some(visuals)=canvas_visuals {
                canvas_effects=Some(visuals.publish_metadata(&app));
                let state=app.global::<AppState>();
                if let Some((source,task_id))=&canvas_target {
                    let filled={
                        let store=context.store.borrow();
                        store.assets.iter().find(|asset|asset.id==id).is_some_and(|asset|
                            store.canvas_notes.iter().chain(store.canvas_workspaces.values().flat_map(|workspace|workspace.notes.iter()))
                                .any(|note|note.id==*source && note.kind=="image" && note.image_path==asset.source_path))
                    };
                    if filled && context.generations.active.borrow().values().any(|task|task.task_id==*task_id)
                        && state.get_canvas_generation_loading_node_id().as_str()==source {
                        state.set_canvas_generation_loading_node_id("".into());
                    }
                }
                state.set_canvas_can_undo(context.canvas_history.borrow().can_undo());
                state.set_canvas_can_redo(context.canvas_history.borrow().can_redo());
            }
            complete(&app,result.map(|ack|(image,id,ack)).map_err(|_|anyhow!("original image delivery was not confirmed")));
        });
        // Includes enhancement/cutout/toolbox deliveries, which do not use the main generation poller.
        refresh_backend_snapshot_captured(&app,context.clone(),persistence.clone());
        if let Some(effects)=canvas_effects {start_canvas_preview_effects(&app,persistence,effects);}
        if let Some(effects)=effects {start_activation_visual_effects(&app,context,effects);}
    });
}

pub(super) struct PendingNamespaceDeliveryCommit {
    receiver: mpsc::Receiver<client_state::WriteResult>,
    proof: NamespaceDeliveryProof,
    asset_id: String,
}
impl PendingNamespaceDeliveryCommit {
    pub(super) fn asset_id(&self) -> &str { &self.asset_id }
    /// Blocking wait belongs on a registered background worker, never in a
    /// completion or while borrowing Store. Only the real writer ack grants a receipt.
    pub(super) fn wait(self) -> Result<CommittedNamespaceDelivery> {
        self.receiver.recv().map_err(|_| anyhow!("delivery writer acknowledgment disconnected"))??;
        Ok(CommittedNamespaceDelivery { prepared: self.proof })
    }
}
pub(super) enum DeliveryEnqueueOwnership {
    Prepared(PreparedPrivateStoreWrite),
    Queue(client_state::PreparedStoreEnqueueError),
}
pub(super) struct GuardedDeliveryEnqueueError {
    error: anyhow::Error,
    _ownership: DeliveryEnqueueOwnership,
    _prepared: DeliveryEnqueueEvidence,
}
enum DeliveryEnqueueEvidence { Image(PreparedNamespaceDelivery),Video(PreparedNamespaceVideoDelivery) }
impl std::fmt::Debug for GuardedDeliveryEnqueueError {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result { self.error.fmt(f) }
}
impl std::fmt::Display for GuardedDeliveryEnqueueError {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result { std::fmt::Display::fmt(&self.error,f) }
}
impl std::error::Error for GuardedDeliveryEnqueueError {}
impl PreparedPrivateStoreWrite {
    /// Call only within the ORIGINAL lease completion. Prepare this write and
    /// all held-file/index validation/decoding outside it. Return the ENTIRE
    /// result out of the completion: even a failure owns admission and proof.
    pub(super) fn enqueue_delivery(self, app:&AppWindow, store:&mut Store,
        prepared:PreparedNamespaceDelivery, time:&str)
        -> std::result::Result<PendingNamespaceDeliveryCommit,GuardedDeliveryEnqueueError> {
        let staged = if self.lease() != prepared.lease()
            || !store.private_persistence.as_ref().is_some_and(|binding|binding.lease()==self.lease()) {
            Err(anyhow!("delivery writer lease mismatch"))
        } else if !prepared.record().canvas_source_node_id.is_empty() {
            let loading=app.global::<AppState>().get_canvas_generation_loading_node_id();
            stage_canvas_namespace_delivery(store,&prepared,time,loading.as_str())
        } else {
            stage_namespace_delivery(store,&prepared,time)
        };
        let asset_id = match staged {
            Ok(id)=>id,
            Err(error)=>return Err(GuardedDeliveryEnqueueError {error,_ownership:DeliveryEnqueueOwnership::Prepared(self),_prepared:DeliveryEnqueueEvidence::Image(prepared)}),
        };
        // Immediate ordered enqueue uses the current full Store projection.
        // Never undo staging after queue/ack failure: another snapshot may
        // already retain the same output. Its exact retry is idempotent.
        match self.enqueue(local_store_data(app,store)) {
            Ok(receiver)=>Ok(PendingNamespaceDeliveryCommit {receiver,proof:prepared.into_proof(),asset_id}),
            Err(error)=>Err(GuardedDeliveryEnqueueError {
                error:anyhow!("delivery Store enqueue refused"),
                _ownership:DeliveryEnqueueOwnership::Queue(error),_prepared:DeliveryEnqueueEvidence::Image(prepared),
            }),
        }
    }
}
/// Pure metadata projection only. The owned delivery proof was validated during
/// preparation, and will be checked again by the post-ack delivery consumer.
// Canvas output metadata stage. No filesystem/decoder/writer wait here.
fn stage_canvas_namespace_delivery(
    store: &mut Store, prepared: &PreparedNamespaceDelivery, time: &str, loading_node_id: &str,
) -> Result<String> {
    let record=prepared.record();let confirmation=prepared.confirmation();
    anyhow::ensure!(record.task_type=="image_generation" && !record.canvas_source_node_id.is_empty()
        && confirmation.failed_asset_id.is_none(),"invalid Canvas output identity");
    let source_id=&record.canvas_source_node_id;
    let active=normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
    let mut candidates=Vec::new();
    let active_sources=store.canvas_notes.iter().filter(|note|note.id==*source_id).count();
    anyhow::ensure!(active_sources<=1,"ambiguous Canvas source");
    if active_sources==1{candidates.push(active.clone());}
    for (workspace_id,workspace) in &store.canvas_workspaces {
        if workspace_id==&active{continue;}
        let count=workspace.notes.iter().filter(|note|note.id==*source_id).count();
        anyhow::ensure!(count<=1,"ambiguous Canvas source");
        if count==1{candidates.push(workspace_id.clone());}
    }
    let [target]=candidates.as_slice()else{anyhow::bail!("original Canvas workspace missing or ambiguous");};
    let (mut notes,mut links)=if target==&active {(store.canvas_notes.clone(),store.canvas_links.clone())}
        else {let workspace=&store.canvas_workspaces[target];(workspace.notes.clone(),workspace.links.clone())};
    let source=notes.iter().find(|note|note.id==*source_id).cloned().ok_or_else(||anyhow!("Canvas source missing"))?;
    let id=confirmation.file_id.clone();let path=prepared.source_path();
    let node_id=format!("delivery-{}-{}",record.client_request_id,confirmation.item_index);
    anyhow::ensure!(!store.generations.iter().any(|asset|asset.id==id || asset.source_path==path),"Canvas output cannot adopt generation history");
    anyhow::ensure!(!store.assets.iter().any(|asset|asset.source_path==path && asset.id!=id),"Canvas output path conflict");
    let assets=store.assets.iter().filter(|asset|asset.id==id).collect::<Vec<_>>();
    anyhow::ensure!(assets.len()<=1,"Canvas output asset ambiguous");
    if let [asset]=assets.as_slice(){
        anyhow::ensure!(asset.source_path==path && asset.category=="other" && asset.origin=="generation"
            && asset.conversation_id==record.conversation_id && asset.model==record.model_code
            && !asset.delivery_recoverable && !asset.delivery_downloading,"Canvas output retry metadata changed");
        let matching=notes.iter().filter(|note|note.kind=="image" && note.image_path==path
            && (note.id==node_id || note.id==*source_id)).collect::<Vec<_>>();
        anyhow::ensure!(matching.len()==1,"Canvas output retry node missing or ambiguous");
        if matching[0].id==node_id {
            anyhow::ensure!(links.iter().any(|link|link.source_id==*source_id && link.target_id==node_id),"Canvas output retry source link missing");
        }
        return Ok(id);
    }
    anyhow::ensure!(!notes.iter().any(|note|note.id==node_id || note.image_path==path),"Canvas output node collision");
    let (width,height)=prepared.preview().dimensions();
    let replaces=loading_node_id==source_id && source.kind=="image" && source.image_path.trim().is_empty();
    anyhow::ensure!(replaces || (notes.len()<200 && links.len()<400),"Canvas capacity reached");
    if !replace_canvas_generation_placeholder(&mut notes,source_id,loading_node_id,path,width as f32,height as f32,0) {
        let mut node=CanvasNoteData{id:node_id.clone(),kind:"image".into(),image_path:path.into(),
            width:340.0,height:250.0,z_index:notes.iter().map(|note|note.z_index).max().unwrap_or(0).saturating_add(1),
            ..Default::default()};
        fit_image_node_to_intrinsic_aspect(&mut node,width as f32,height as f32);
        let (x,y)=generated_canvas_result_position(Some(&source),node.width,node.height,confirmation.item_index as i32,record.count);
        (node.x,node.y)=nearest_free_canvas_position(&notes,x,y,node.width,node.height,None);
        notes.push(node);
        anyhow::ensure!(matches!(connect_nodes(&mut links,source_id,&node_id),CanvasConnectResult::Connected{..}),"Canvas output link could not be created");
    }
    let references=if !record.lineage_reference_paths.is_empty(){record.lineage_reference_paths.clone()}else{record.reference_paths.clone()};
    let asset=AssetData{id:id.clone(),conversation_id:record.conversation_id.clone(),title:short_text(&record.raw_prompt,18),
        category:"other".into(),kind:record.mode.clone(),time:time.into(),prompt:display_generation_prompt(&record.generation_prompt),
        ratio:ratio_from_actual_dimensions(width as i32,height as i32),
        quality:quality_from_actual_dimensions(width as i32,height as i32),model:record.model_code.clone(),
        origin:"generation".into(),width:width as i32,height:height as i32,source_path:path.into(),reference_paths:references,
        cutout_done:false,remove_black_done:false,upscale_done:false,is_new:true,delivery_recoverable:false,delivery_downloading:false};
    if target==&active {store.canvas_notes=notes;store.canvas_links=links;}
    else {let workspace=store.canvas_workspaces.get_mut(target).unwrap();workspace.notes=notes;workspace.links=links;}
    store.assets.insert(0,asset);
    Ok(id)
}

fn stage_namespace_delivery(store:&mut Store, prepared:&PreparedNamespaceDelivery,time:&str)->Result<String> {
    let record=prepared.record();
    let confirmation=prepared.confirmation();
    let toolbox=match record.task_type.as_str() {
        "image_generation"|"image_edit"|"image_upscale"=>None,
        "image_watermark_removal"=>Some(("watermark_removal","去水印")),
        "image_colorization"=>Some(("image_colorization","老照片上色")),
        "image_enhancement"=>Some(("image_enhancement","图片清晰")),
        "image_cutout"=>Some(("image_cutout","智能抠图")),
        _=>anyhow::bail!("unsupported image delivery metadata type"),
    };
    anyhow::ensure!(toolbox.is_none() || confirmation.failed_asset_id.is_none(),"toolbox output cannot replace an ordinary failed card");
    let id=confirmation.failed_asset_id.as_deref().unwrap_or(&confirmation.file_id).to_owned();
    let path=prepared.source_path();
    let notification_id=format!("delivery-{}",confirmation.file_id);
    let assets=store.assets.iter().filter(|asset|asset.id==id).collect::<Vec<_>>();
    let generations=store.generations.iter().filter(|asset|asset.id==id).collect::<Vec<_>>();
    let notifications=store.notifications.iter().filter(|item|item.id==notification_id).collect::<Vec<_>>();
    anyhow::ensure!(assets.len()<=1 && generations.len()<=1,"delivery metadata is ambiguous");
    anyhow::ensure!(!store.assets.iter().chain(&store.generations).any(|asset|asset.source_path==path && asset.id!=id),
        "delivery path belongs to conflicting metadata");
    if let [asset]=assets.as_slice() {
        anyhow::ensure!(asset.source_path==path && !asset.delivery_recoverable && !asset.delivery_downloading,
            "successful delivery asset conflicts");
        if let Some((origin,_))=toolbox {
            anyhow::ensure!(generations.is_empty() && asset.category=="other" && asset.origin==origin,
                "toolbox delivery must remain Other-only");
        } else {
            let [generation]=generations.as_slice() else {anyhow::bail!("delivery generation projection missing");};
            anyhow::ensure!(generation.source_path==path && !generation.delivery_recoverable && !generation.delivery_downloading,
                "successful delivery generation conflicts");
        }
        anyhow::ensure!(notifications.len()==1 && notifications[0].success,"delivery success notification missing or conflicting");
        return Ok(id);
    }
    anyhow::ensure!(notifications.is_empty(),"delivery notification conflicts");
    let (width,height)=prepared.preview().dimensions();
    let enhancement=record.task_type=="image_enhancement";
    let cutout=record.task_type=="image_cutout";
    let title=if enhancement {
        let source=record.lineage_reference_paths.first().or_else(||record.reference_paths.first());
        let stem=source.and_then(|source|Path::new(source).file_stem()).and_then(|stem|stem.to_str())
            .filter(|stem|!stem.trim().is_empty()).unwrap_or("图片");
        format!("{} 清晰增强",short_text(stem,18))
    }else if cutout {format!("{} 抠图",short_text(record.raw_prompt.trim(),18))}
    else{short_text(&record.raw_prompt,18)};
    let notification=NotificationData {
        id:notification_id,title:if enhancement{format!("图片清晰增强完成：{title}")}else if cutout{format!("智能抠图完成：{title}")}else{format!("{}：{}",toolbox.map(|(_,name)|name).unwrap_or("Generation succeeded"),short_text(&record.raw_prompt,24))},
        model:toolbox.map(|(_,name)|name.to_owned()).unwrap_or_else(||record.model_code.clone()),
        time:time.to_owned(),reason:String::new(),success:true,read:false,
    };
    if confirmation.failed_asset_id.is_some() {
        let [failed]=generations.as_slice() else {anyhow::bail!("failed delivery card missing or ambiguous");};
        anyhow::ensure!(failed.source_path=="failed" && failed.delivery_recoverable
            && failed.conversation_id==record.conversation_id && failed.category==record.category
            && failed.kind==record.mode && failed.model==record.model_code,"failed delivery card identity mismatch");
        let mut completed=(*failed).clone();
        completed.source_path=path.to_owned();completed.width=width as i32;completed.height=height as i32;
        completed.ratio=ratio_from_actual_dimensions(width as i32,height as i32);
        completed.quality=quality_from_actual_dimensions(width as i32,height as i32);
        completed.time=time.to_owned();completed.is_new=true;completed.delivery_recoverable=false;completed.delivery_downloading=false;
        let notification=NotificationData {title:format!("图片下载完成：{}",short_text(&completed.prompt,24)),..notification};
        local_store::replace_failed_delivery_asset_with(store,&id,completed,notification,|_|Ok(()))?;
    } else {
        anyhow::ensure!(generations.is_empty(),"delivery generation identity conflicts");
        let item=AssetData {
            id:id.clone(),conversation_id:if toolbox.is_some(){String::new()}else{record.conversation_id.clone()},
            title,category:if toolbox.is_some(){"other".into()}else{record.category.clone()},
            kind:if enhancement || cutout{"game".into()}else{record.mode.clone()},time:time.to_owned(),prompt:if enhancement{"图片清晰增强".into()}else if cutout{
                let label=match record.quality.as_str(){"portrait"=>"人像","avatar"=>"头像","skin"=>"皮肤","product"=>"商品","clothing"=>"服饰","sky"=>"天空",_=>"通用"};
                format!("智能抠图（{label}）")
            }else{display_generation_prompt(&record.generation_prompt)},
            ratio:ratio_from_actual_dimensions(width as i32,height as i32),
            quality:quality_from_actual_dimensions(width as i32,height as i32),
            model:toolbox.map(|(_,name)|name.to_owned()).unwrap_or_else(||record.model_code.clone()),
            origin:toolbox.map(|(origin,_)|origin).unwrap_or(if record.task_type=="image_edit"{"image_edit"}else{"generation"}).into(),
            width:width as i32,height:height as i32,source_path:path.to_owned(),
            reference_paths:if !record.lineage_reference_paths.is_empty(){record.lineage_reference_paths.clone()}
                else if matches!(record.task_type.as_str(),"image_edit"|"image_upscale"){Vec::new()}
                else{record.reference_paths.clone()},
            cutout_done:cutout,remove_black_done:false,upscale_done:record.task_type=="image_upscale" || enhancement,
            is_new:toolbox.is_none(),delivery_recoverable:false,delivery_downloading:false,
        };
        if toolbox.is_none() {
            reveal_prompt_history_entry(store,&item.prompt);
            store.generations.insert(0,item.clone());
        }
        store.assets.insert(0,item);
        store.notifications.insert(0,notification);
    }
    Ok(id)
}

pub(super) struct CommittedNamespaceDelivery {
    prepared: NamespaceDeliveryProof,
}

impl CommittedNamespaceDelivery {
    pub(super) fn into_prepared(self) -> NamespaceDeliveryProof {
        self.prepared
    }
}

#[cfg(test)]
pub(super) fn persist_namespace_delivery(
    app:&AppWindow,store:&mut Store,writer:&ClientStateWriter,prepared:PreparedNamespaceDelivery,time:&str,
)->Result<(Image,String,CommittedNamespaceDelivery)> {
    // Test-only synchronous adapter exercises the same pure staging and actual
    // held writer acknowledgment. Production callers use owned ordered enqueue.
    prepared.ensure_current()?;
    let id=stage_namespace_delivery(store,&prepared,time)?;
    save_local_store_checked_for_namespace(app,store,writer,prepared.lease())?;
    prepared.ensure_current()?;
    let image=materialize_delivery_preview(prepared.preview());
    Ok((image,id,CommittedNamespaceDelivery{prepared:prepared.into_proof()}))
}

impl PreparedPrivateStoreWrite {
    pub(super) fn enqueue_video_delivery(self,app:&AppWindow,store:&mut Store,
        prepared:PreparedNamespaceVideoDelivery,time:&str)
        -> std::result::Result<PendingNamespaceDeliveryCommit,GuardedDeliveryEnqueueError> {
        let staged=if self.lease()!=prepared.lease()
            || !store.private_persistence.as_ref().is_some_and(|binding|binding.lease()==self.lease()) {
            Err(anyhow!("video Store binding mismatch"))
        } else { stage_video_delivery(store,&prepared,time) };
        let asset_id=match staged {
            Ok(id)=>id,
            Err(error)=>return Err(GuardedDeliveryEnqueueError {error,_ownership:DeliveryEnqueueOwnership::Prepared(self),
                _prepared:DeliveryEnqueueEvidence::Video(prepared)}),
        };
        match self.enqueue(local_store_data(app,store)) {
            Ok(receiver)=>Ok(PendingNamespaceDeliveryCommit {receiver,proof:prepared.into_proof(),asset_id}),
            Err(error)=>Err(GuardedDeliveryEnqueueError {error:anyhow!("video Store enqueue refused"),
                _ownership:DeliveryEnqueueOwnership::Queue(error),_prepared:DeliveryEnqueueEvidence::Video(prepared)}),
        }
    }
}
fn stage_video_delivery(store:&mut Store,prepared:&PreparedNamespaceVideoDelivery,time:&str)->Result<String> {
    let record=prepared.record();let confirmation=prepared.confirmation();
    anyhow::ensure!(matches!(record.task_type.as_str(),"image_to_video"|"video_generation")
        && record.video_request.is_some() && confirmation.failed_asset_id.is_none(),"video output identity incomplete");
    let output=SavedVideoOutput {
        model: record.model_code.clone(), resolution: record.quality.clone(), duration_secs: record.video_request.as_ref().map(|request| request.duration_secs).unwrap_or(0),
        source_asset_id:record.source_asset_id.clone(),
        prompt:record.raw_prompt.clone(),
        client_request_id:record.client_request_id.clone(),server_task_id:confirmation.task_id.clone(),
        file_id:confirmation.file_id.clone(),billing_account_group_id:record.billing_account_group_id.clone(),
        sha256:confirmation.sha256.clone(),size_bytes:confirmation.size_bytes,source_path:prepared.source_path().into(),
        title:short_text(&record.raw_prompt,48),created_at:time.into(),
    };
    let key=output.key();
    anyhow::ensure!(!store.video_outputs.iter().any(|(other,value)|other!=&key && value.source_path==output.source_path),
        "video path belongs to another retained output");
    if let Some(existing)=store.video_outputs.get(&key) {
        anyhow::ensure!(existing.source_asset_id==output.source_asset_id && existing.client_request_id==output.client_request_id && existing.server_task_id==output.server_task_id
            && existing.file_id==output.file_id && existing.billing_account_group_id==output.billing_account_group_id
            && existing.sha256==output.sha256 && existing.size_bytes==output.size_bytes && existing.source_path==output.source_path,
            "retained video output identity conflicts");
    } else {
        store.video_outputs.insert(key.clone(),output);
    }
    Ok(key)
}

pub(super) fn start_generation(
    app: &AppWindow,
    context: AppContext,
    override_prompt: Option<String>,
    create_conversation: bool,
    retry_failed_id: Option<String>,
    forced_count: Option<i32>,
    existing_generation_policy: ExistingGenerationPolicy,
) {
    start_generation_for_destination(
        app,
        context,
        override_prompt,
        create_conversation,
        retry_failed_id,
        forced_count,
        existing_generation_policy,
        GenerationDestination::Gallery,
    );
}

pub(super) fn start_canvas_generation(
    app: &AppWindow,
    context: AppContext,
    source_node_id: String,
    prompt: String,
) {
    let state = app.global::<AppState>();
    if !state.get_canvas_workflow_id().is_empty() && context.store.borrow().canvas_references.is_empty() {
        state.set_generation_status(if state.get_language().as_str() == "en" {
            "Upload a reference image first"
        } else {
            "请先上传参考图"
        }.into());
        return;
    }
    start_generation_for_destination(
        app,
        context,
        Some(prompt),
        false,
        None,
        None,
        ExistingGenerationPolicy::StopExisting,
        GenerationDestination::Canvas { source_node_id },
    );
}

fn start_generation_for_destination(
    app: &AppWindow,
    context: AppContext,
    override_prompt: Option<String>,
    create_conversation: bool,
    retry_failed_id: Option<String>,
    forced_count: Option<i32>,
    existing_generation_policy: ExistingGenerationPolicy,
    destination: GenerationDestination,
) {
    let state = app.global::<AppState>();
    let visible_prompt = state.get_prompt().trim().to_string();
    let applied_chinese = state
        .get_deep_optimization_applied_chinese()
        .trim()
        .to_string();
    let applied_english = state
        .get_deep_optimization_applied_english()
        .trim()
        .to_string();
    let input_prompt =
        submitted_prompt_for_visible_prompt(&visible_prompt, &applied_chinese, &applied_english);
    let raw_prompt = if let Some(override_prompt) = override_prompt {
        override_prompt.trim().to_string()
    } else {
        let selected_prompts = {
            let store = context.store.borrow();
            selected_custom_prompt_replacements_for_category(
                &store,
                &current_workspace_category(app),
            )
        };
        compose_inline_custom_prompts(&input_prompt, &selected_prompts)
    };
    if raw_prompt.trim().is_empty() {
        state.set_generation_status("请输入生成需求".into());
        return;
    }
    if !require_online_operation(app, "生成图片") {
        return;
    }
    if context.backend.is_none() {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    }
    start_backend_generation(
        app,
        context,
        raw_prompt,
        create_conversation,
        retry_failed_id,
        forced_count,
        existing_generation_policy,
        destination,
    );
}

pub(super) fn start_asset_regeneration(
    app: &AppWindow,
    context: AppContext,
    item: AssetData,
) -> bool {
    let is_canvas_result = item.category == "other";
    if is_canvas_result && category_is_generating(&context, &current_workspace_category(app)) {
        app.global::<AppState>().set_viewer_message("当前已有生成任务，请等待完成后再试".into());
        return false;
    }
    if !restore_asset_regeneration_inputs(app, &context, &item) {
        return false;
    }
    let state = app.global::<AppState>();
    let destination = if is_canvas_result {
        let source_node_id = state.invoke_create_canvas_generation_source(item.prompt.clone().into(), 0.0, 0.0);
        if source_node_id.is_empty() {
            return false;
        }
        GenerationDestination::Canvas { source_node_id: source_node_id.to_string() }
    } else {
        GenerationDestination::Gallery
    };
    state.set_viewer_message("".into());
    state.set_viewer_open(false);
    start_generation_for_destination(
        app,
        context,
        Some(item.prompt),
        false,
        None,
        None,
        ExistingGenerationPolicy::KeepExisting,
        destination,
    );
    true
}

fn restore_asset_regeneration_inputs(
    app: &AppWindow,
    context: &AppContext,
    item: &AssetData,
) -> bool {
    let state = app.global::<AppState>();
    let category = resolve_category(&item.category, &item.prompt);
    let max_references = max_reference_images_for_category(&category);
    if item.reference_paths.len() > max_references {
        let message = reference_limit_message(max_references);
        state.set_viewer_message(message.into());
        state.set_generation_status(message.into());
        return false;
    }

    let mut references = Vec::with_capacity(item.reference_paths.len());
    for source_path in &item.reference_paths {
        let path = PathBuf::from(source_path);
        if !path.is_file() {
            let message = format!("原参考图已不存在，无法再次生成：{}", path.display());
            state.set_viewer_message(message.clone().into());
            state.set_generation_status(message.into());
            return false;
        }
        if load_preview_image(&path, PreviewPurpose::Reference).is_err() {
            let message = format!("原参考图无法读取，无法再次生成：{}", path.display());
            state.set_viewer_message(message.clone().into());
            state.set_generation_status(message.into());
            return false;
        }
        references.push(ReferenceData {
            id: Uuid::new_v4().to_string(),
            source_path: path.display().to_string(),
        });
    }

    // Unclassified AI creations must never fall through to the default character gallery.
    let is_canvas_result = item.category == "other";
    if is_canvas_result {
        state.invoke_open_canvas_workspace(DEFAULT_CANVAS_WORKSPACE_ID.into());
        state.set_canvas_workflow_id("".into());
        state.set_canvas_workflow_title("".into());
        state.set_canvas_workflow_template("".into());
        state.set_canvas_workflow_hint("".into());
        state.set_canvas_workflow_artwork(Image::default());
        state.set_canvas_workflow_prompt(item.prompt.clone().into());
        state.set_canvas_tool("select".into());
    } else {
        state.set_asset_type(category.clone().into());
        state.set_current_conversation_id(item.conversation_id.clone().into());
    }
    if !item.ratio.trim().is_empty() {
        state.set_ratio(item.ratio.clone().into());
    }
    if !item.quality.trim().is_empty() {
        state.set_quality(item.quality.clone().into());
    }
    if !item.kind.trim().is_empty() {
        state.set_mode(item.kind.clone().into());
    }
    {
        let mut store = context.store.borrow_mut();
        if is_canvas_result {
            store.canvas_references = references;
            navigate_to_with_store(app, &store, "canvas");
        } else {
            *references_for_category_mut(&mut store.references, &category) = references;
            push_references(app, &store);
            push_generations(app, &store);
        }
    }
    sync_generation_state_for_current_category(context, app);
    true
}

fn submitted_prompt_for_visible_prompt(
    visible_prompt: &str,
    applied_chinese: &str,
    applied_english: &str,
) -> String {
    if !applied_english.trim().is_empty() && visible_prompt.trim() == applied_chinese.trim() {
        applied_english.trim().to_string()
    } else {
        visible_prompt.trim().to_string()
    }
}

#[cfg(test)]
mod deep_prompt_tests {
    use super::{
        insert_canvas_generated_asset, replace_canvas_generation_placeholder,
        submitted_prompt_for_visible_prompt, CanvasNoteData, Store,
    };

    #[test]
    fn applied_chinese_prompt_submits_its_matching_english_version() {
        assert_eq!(
            submitted_prompt_for_visible_prompt(
                "月下的锻造工坊",
                "月下的锻造工坊",
                "a moonlit forge workshop",
            ),
            "a moonlit forge workshop",
        );
    }

    #[test]
    fn editing_the_readable_prompt_invalidates_the_english_binding() {
        assert_eq!(
            submitted_prompt_for_visible_prompt(
                "月下的古老锻造工坊",
                "月下的锻造工坊",
                "a moonlit forge workshop",
            ),
            "月下的古老锻造工坊",
        );
    }

    #[test]
    fn composer_generation_replaces_its_loading_rectangle_in_place() {
        let mut notes = vec![CanvasNoteData {
            id: "loading-result".to_string(),
            kind: "image".to_string(),
            content: "tomato growth".to_string(),
            x: 100.0,
            y: 200.0,
            width: 340.0,
            height: 250.0,
            selected: true,
            ..CanvasNoteData::default()
        }];

        assert!(replace_canvas_generation_placeholder(
            &mut notes,
            "loading-result",
            "loading-result",
            "generated.png",
            1024.0,
            512.0,
            0,
        ));
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].image_path, "generated.png");
        assert_eq!(notes[0].content, "");
        assert!((notes[0].width / notes[0].height - 2.0).abs() < 0.001);
        assert!((notes[0].width - 680.0).abs() < 0.001);
        assert!((notes[0].height - 340.0).abs() < 0.001);
        assert!((notes[0].x + notes[0].width / 2.0 - 270.0).abs() < 0.001);
        assert!((notes[0].y + notes[0].height / 2.0 - 325.0).abs() < 0.001);
        assert!(!notes[0].selected);
    }

    #[test]
    fn composer_generation_moves_result_away_from_an_existing_image() {
        let mut notes = vec![
            CanvasNoteData {
                id: "existing-image".to_string(),
                kind: "image".to_string(),
                image_path: "existing.png".to_string(),
                x: 100.0,
                y: 200.0,
                width: 340.0,
                height: 250.0,
                ..CanvasNoteData::default()
            },
            CanvasNoteData {
                id: "loading-result".to_string(),
                kind: "image".to_string(),
                content: "tomato growth".to_string(),
                x: 100.0,
                y: 200.0,
                width: 340.0,
                height: 250.0,
                selected: true,
                ..CanvasNoteData::default()
            },
        ];

        assert!(replace_canvas_generation_placeholder(
            &mut notes,
            "loading-result",
            "loading-result",
            "generated.png",
            680.0,
            500.0,
            0,
        ));

        let generated = notes
            .iter()
            .find(|note| note.id == "loading-result")
            .expect("generated image");
        let existing = notes
            .iter()
            .find(|note| note.id == "existing-image")
            .expect("existing image");
        assert!(
            generated.x >= existing.x + existing.width + 48.0
                || generated.x + generated.width + 48.0 <= existing.x
                || generated.y >= existing.y + existing.height + 48.0
                || generated.y + generated.height + 48.0 <= existing.y
        );
    }

    #[test]
    fn canvas_generation_is_added_to_other_assets_without_copying_its_file() {
        let mut store = Store::default();
        let references = vec!["reference.png".to_string()];

        insert_canvas_generated_asset(
            &mut store,
            "生成角色体型变化",
            "生成角色体型变化",
            "game",
            "2K",
            "openai_image",
            "generation",
            "canvas-conversation",
            "2026-09-02 12:30",
            "generated.png",
            &references,
            2048,
            1152,
            false,
        );

        assert_eq!(store.assets.len(), 1);
        assert!(store.generations.is_empty());
        let asset = &store.assets[0];
        assert_eq!(asset.category, "other");
        assert_eq!(asset.source_path, "generated.png");
        assert_eq!(asset.ratio, "16:9");
        assert_eq!(asset.reference_paths, references);
    }

    #[test]
    fn regenerating_an_ai_creation_keeps_workbench_inputs_and_uses_canvas() {
        use super::*;
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let context = AppContext::default();
        wire_infinite_canvas_callbacks(&app, context.clone());
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_page("assets".into());
        state.set_asset_type("scene".into());
        state.set_prompt("workbench draft".into());
        context.store.borrow_mut().references.character.push(ReferenceData {
            id: "keep-character-reference".into(), source_path: String::new(),
        });
        insert_canvas_generated_asset(&mut context.store.borrow_mut(), "AI result", "AI prompt",
            "game", "2K", "test-model", "generation", "ai-conversation", "", "result.png",
            &[], 2560, 1440, false);
        let item = context.store.borrow().assets[0].clone();
        assert!(restore_asset_regeneration_inputs(&app, &context, &item));
        assert_eq!(state.get_page(), "canvas");
        assert_eq!(state.get_prompt(), "workbench draft");
        assert_eq!(state.get_canvas_workflow_prompt(), "AI prompt");
        assert_eq!(context.store.borrow().references.character.len(), 1);
        assert!(context.store.borrow().generations.is_empty());
    }
}

pub(super) fn compose_inline_custom_prompts(
    input_prompt: &str,
    replacements: &[(String, String)],
) -> String {
    let mut composed = input_prompt.to_string();
    let mut missing = Vec::new();
    for (name, content) in replacements {
        let content = content.trim();
        if content.is_empty() {
            continue;
        }
        let display = inline_custom_prompt_display_text(name);
        if composed.contains(&display) {
            composed = composed.replacen(&display, content, 1);
        } else {
            missing.push(content.to_string());
        }
    }
    let composed = composed.trim();
    if composed.is_empty() || composed == "//" {
        return missing.join("\n\n");
    }
    if missing.is_empty() {
        composed.to_string()
    } else {
        missing.push(composed.to_string());
        missing.join("\n\n")
    }
}

pub(super) fn retry_failed_generation(app: &AppWindow, context: AppContext, id: String) {
    let store = context.store.clone();
    let item = {
        let store_ref = store.borrow();
        store_ref
            .generations
            .iter()
            .find(|item| item.id == id && item.source_path == "failed")
            .cloned()
    };
    let Some(item) = item else {
        app.global::<AppState>()
            .set_generation_status("未找到可重试的失败图片".into());
        return;
    };
    if item.prompt.trim().is_empty() {
        app.global::<AppState>()
            .set_generation_status("失败图片没有可重试的提示词".into());
        return;
    }
    if !restore_asset_regeneration_inputs(app, &context, &item) {
        return;
    }
    let state = app.global::<AppState>();
    state.set_count(1);
    state.set_prompt(item.prompt.clone().into());
    start_generation(
        app,
        context,
        Some(item.prompt),
        false,
        Some(item.id),
        Some(1),
        ExistingGenerationPolicy::KeepExisting,
    );
}

fn start_registered_paid_stop(app:&AppWindow,context:&AppContext,task:ActiveGeneration,persistence:PrivatePersistence) {
    if !persistence.is_current() || !paid_viewer_binding_matches(context,&persistence) { return; }
    let Some(key)=task.client_request_id.clone() else { return; };
    let worker_task=task.clone();let cancellations=context.cancelled_generation_requests.clone();
    let launched=spawn_delivery_preparation(&persistence,move|captured,activity,cancel|{
        if activity.is_quiescing() || cancel.load(Ordering::Acquire) { return Err(DeliveryRetryError::AuthenticationRequired); }
        let authority=captured.storage_authority()?;
        let row=load_pending_generations_for_namespace(&authority)?.into_iter()
            .find(|row|row.client_request_id==key).ok_or_else(||anyhow!("original cancellation record not yet durable"))?;
        if row.local_task_id!=worker_task.task_id
            || row.owner_user_id!=worker_task.session_scope.owner_user_id
            || row.auth_epoch!=worker_task.session_scope.auth_epoch {
            return Err(anyhow!("original cancellation task changed").into());
        }
        if !row.cancel_requested {
            if !apply_generation_patch_for_namespace(&authority,&row.identity(),GenerationRecoveryPatch::RequestCancellation)? {
                return Err(anyhow!("original cancellation identity changed").into());
            }
        }
        // The existing running checked worker is the sole remote-cancel owner.
        // Shutdown may stop it locally; this acknowledged marker remains recoverable.
        cancellations.lock().map_err(|_|anyhow!("cancellation ownership unavailable"))?.insert(key);
        Ok(())
    });
    match launched {
        Ok((cancel,receiver))=>poll_registered_paid_stop(app.as_weak(),context.clone(),persistence,task,cancel,receiver),
        Err(_)=>{let _=context.apply_user_completion(persistence.lease(),||{
            if paid_viewer_binding_matches(context,&persistence)
                && active_generation_matches_scope(context,&task.category,&task.task_id,&task.session_scope) {
                set_generation_status_for_category(context,app,&task.category,"无法保存停止请求；生成状态已保留，请重试");
            }
        });}
    }
}

fn poll_registered_paid_stop(weak:Weak<AppWindow>,context:AppContext,persistence:PrivatePersistence,task:ActiveGeneration,
    cancel:Arc<std::sync::atomic::AtomicBool>,receiver:mpsc::Receiver<std::result::Result<(),DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        let Some(app)=weak.upgrade()else{cancel.store(true,Ordering::Release);return;};
        if !persistence.is_current() || !paid_viewer_binding_matches(&context,&persistence) {
            cancel.store(true,Ordering::Release);return;
        }
        let joined=finish_delivery_preparation(&cancel);
        if matches!(joined,Ok(true)) {poll_registered_paid_stop(weak,context,persistence,task,cancel,receiver);return;}
        let saved=matches!(joined,Ok(false)) && matches!(receiver.try_recv(),Ok(Ok(())));
        let _=context.apply_user_completion(persistence.lease(),||{
            if !paid_viewer_binding_matches(&context,&persistence)
                || !active_generation_matches_scope(&context,&task.category,&task.task_id,&task.session_scope) {return;}
            if !saved {
                set_generation_status_for_category(&context,&app,&task.category,"无法保存停止请求；生成状态已保留，请重试");return;
            }
            remove_active_generation(&context,&task.category,&task.task_id);
            set_generation_status_for_category(&context,&app,&task.category,"停止请求已保存；已提交的任务正在确认");
            sync_generation_state_for_current_category(&context,&app);
            let state=app.global::<AppState>();
            if current_workspace_category(&app)==task.category {
                if !task.prompt.trim().is_empty(){state.set_prompt(task.prompt.clone().into());}
                finish_conversation_placeholder(&state,&task.conversation_id,None);
            }
        });
    });
}

pub(super) fn stop_generation(app: &AppWindow, context: &AppContext) {
    let paid_task=context.generations.active.borrow().get(&current_workspace_category(app)).cloned();
    if let Some(task)=paid_task {
        if let Some(persistence)=task.registered_cancel_owner.clone() {
            start_registered_paid_stop(app,context,task,persistence);return;
        }
    }
    let store = &context.store;
    let state = app.global::<AppState>();
    let category = current_workspace_category(app);
    let task_id = context
        .generations
        .active
        .borrow()
        .get(&category)
        .map(|task| task.task_id.clone());
    let Some(task_id) = task_id else {
        sync_generation_state_for_current_category(context, app);
        return;
    };
    let cancellation = (|| -> Result<()> {
        let active = context.generations.active.borrow();
        let task = active.get(&category).ok_or_else(|| anyhow!("active generation changed"))?;
        if let Some(key) = task.client_request_id.as_ref() {
            let authority = context.storage_authority_for(&context.namespace_for(&task.session_scope)?)?;
            let row = load_pending_generations_for_namespace(&authority)?.into_iter()
                .find(|row| row.client_request_id == *key).ok_or_else(|| anyhow!("retained cancellation record missing"))?;
            anyhow::ensure!(apply_generation_patch_for_namespace(&authority, &row.identity(), GenerationRecoveryPatch::RequestCancellation)?, "retained cancellation identity changed");
        }
        Ok(())
    })();
    if cancellation.is_err() {
        state.set_generation_status("无法保存停止请求；生成状态已保留，请重试".into());
        return;
    }
    let Some(task) = remove_active_generation(context, &category, &task_id) else {
        sync_generation_state_for_current_category(context, app);
        return;
    };
    discard_canvas_generation_placeholder(&state, &task.destination);
    refresh_delivery_download_flags(app, context);
    set_generation_status_for_category(context, app, &category, "停止请求已保存；已提交的任务正在确认");
    sync_generation_state_for_current_category(context, app);
    if task.destination == GenerationDestination::Gallery {
        if !task.prompt.trim().is_empty() {
            state.set_prompt(task.prompt.clone().into());
        }
        finish_conversation_placeholder(&state, &task.conversation_id, None);
    }
    match task.destination {
        GenerationDestination::Canvas { .. } => push_canvas_references(app, &store.borrow()),
        GenerationDestination::Gallery => push_references(app, &store.borrow()),
    }
    if let Some(client_request_id) = task.client_request_id.as_ref() {
        if let Ok(mut cancellations) = context.cancelled_generation_requests.lock() {
            cancellations.insert(client_request_id.clone());
        }
    }
    if generation_scope_allows_polling(&app.as_weak(), context, &task.session_scope) {
        if let (Some(backend), Some(server_task_id)) =
            (context.backend.clone(), task.server_task_id)
        {
            let Some(key) = task.client_request_id else { return; };
            let Ok(lease) = context.namespace_for(&task.session_scope) else { return; };
            let Ok(authority) = context.storage_authority_for(&lease) else { return; };
            let Ok(activity) = backend.api.begin_user_work(&task.session_scope) else { return; };
            let cancellations = context.cancelled_generation_requests.clone();
            let session_scope = task.session_scope;
            let worker_scope = session_scope.clone();
            let (sender, receiver) = mpsc::channel::<()>();
            std::thread::spawn(move || {
                if !activity.is_quiescing() {
                    let api = GenerationApi::new(backend.api.clone());
                    cleanup_cancelled_generation(&backend, &authority, &api, &worker_scope, &key, &[], Some(&server_task_id), &cancellations);
                }
                drop(activity);
                let _ = sender.send(());
            });
            observe_detached_generation_scope(
                app.as_weak(),
                context.clone(),
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
            );
        }
    }
}

pub(super) fn add_stream_success_item(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    raw_prompt: &str,
    category: &str,
    mode: &str,
    quality: &str,
    image_model: &str,
    origin: &str,
    conversation_id: &str,
    display_prompt: &str,
    time: &str,
    staged_path: &Path,
    reference_paths: &[String],
    upscale_done: bool,
) -> Result<(Image, String, String)> {
    let source_path = save_generated_file(app, staged_path, raw_prompt)?;
    let (width, height) = inspect_image_dimensions(Path::new(&source_path))?;
    let (width, height) = (width as i32, height as i32);
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.to_string(),
        title: short_text(raw_prompt, 18),
        category: category.to_string(),
        kind: mode.to_string(),
        time: time.to_string(),
        prompt: display_generation_prompt(display_prompt),
        ratio: ratio_from_actual_dimensions(width, height),
        quality: quality.to_string(),
        model: image_model.to_string(),
        origin: origin.to_string(),
        width,
        height,
        source_path: source_path.clone(),
        reference_paths: reference_paths.to_vec(),
        cutout_done: false,
        remove_black_done: false,
        upscale_done,
        is_new: true,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    let conversation_image =
        load_preview_image(Path::new(&source_path), PreviewPurpose::Reference)?;
    let generated_id = item.id.clone();
    let history_prompt = item.prompt.clone();
    let notification = NotificationData {
        id: Uuid::new_v4().to_string(),
        title: format!("Generation succeeded: {}", short_text(raw_prompt, 24)),
        model: image_model.to_string(),
        time: time.to_string(),
        reason: String::new(),
        success: true,
        read: false,
    };
    let mut store_mut = store.borrow_mut();
    persist_generated_asset_checked(
        app,
        &mut store_mut,
        item,
        notification,
        true,
        Some(&history_prompt),
    )?;
    push_all(app, &store_mut);
    Ok((conversation_image, source_path, generated_id))
}

pub(super) fn replace_failed_delivery_asset_checked(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    failed_asset_id: &str,
    staged_path: &Path,
    time: &str,
) -> Result<(String, String)> {
    let failed_asset = {
        let store = store.borrow();
        let mut matches = store.generations.iter().filter(|item| {
            item.id == failed_asset_id
                && item.source_path == "failed"
                && item.delivery_recoverable
        });
        let failed_asset = matches.next().cloned();
        if matches.next().is_some() {
            anyhow::bail!("failed delivery card is ambiguous");
        }
        failed_asset.ok_or_else(|| anyhow!("failed delivery card is missing"))?
    };
    let (width, height) = inspect_image_dimensions(staged_path)?;
    let source_path = save_generated_file(app, staged_path, &failed_asset.prompt)?;
    let completed_asset = AssetData {
        id: failed_asset.id.clone(),
        conversation_id: failed_asset.conversation_id,
        title: failed_asset.title,
        category: failed_asset.category,
        kind: failed_asset.kind,
        time: time.to_string(),
        prompt: failed_asset.prompt.clone(),
        ratio: ratio_from_actual_dimensions(width as i32, height as i32),
        quality: quality_from_actual_dimensions(width as i32, height as i32),
        model: failed_asset.model.clone(),
        origin: failed_asset.origin,
        width: width as i32,
        height: height as i32,
        source_path: source_path.clone(),
        reference_paths: failed_asset.reference_paths,
        cutout_done: failed_asset.cutout_done,
        remove_black_done: failed_asset.remove_black_done,
        upscale_done: failed_asset.upscale_done,
        is_new: true,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    let notification = NotificationData {
        id: Uuid::new_v4().to_string(),
        title: format!(
            "图片下载完成：{}",
            short_text(&failed_asset.prompt, 24)
        ),
        model: failed_asset.model,
        time: time.to_string(),
        reason: String::new(),
        success: true,
        read: false,
    };
    let persisted = {
        let mut store = store.borrow_mut();
        replace_failed_delivery_asset_with(
            &mut store,
            failed_asset_id,
            completed_asset,
            notification,
            |pending| save_local_store_checked(app, pending),
        )
    };
    if let Err(error) = persisted {
        let _ = fs::remove_file(&source_path);
        return Err(error);
    }
    push_all(app, &store.borrow());
    Ok((source_path, failed_asset_id.to_string()))
}

pub(super) fn add_canvas_stream_success_item(
    app: &AppWindow,
    context: &AppContext,
    source_node_id: &str,
    raw_prompt: &str,
    bytes: &[u8],
    result_index: i32,
    total_count: i32,
    mode: &str,
    quality: &str,
    image_model: &str,
    origin: &str,
    conversation_id: &str,
    display_prompt: &str,
    time: &str,
    reference_paths: &[String],
    upscale_done: bool,
) -> Result<String> {
    let (bytes, _, width, height) = generated_image_from_bytes(bytes)?;
    let loading_node_id = app
        .global::<AppState>()
        .get_canvas_generation_loading_node_id()
        .to_string();
    let replaces_loading_placeholder = result_index == 0 && loading_node_id == source_node_id;
    let target_workspace_id = {
        let store = context.store.borrow();
        let active_workspace_id = normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
        let target_workspace_id = canvas_workspace_id_for_source(&store, source_node_id)
            .ok_or_else(|| anyhow!("生成来源所在画板已不存在"))?;
        let (node_count, link_count) = if target_workspace_id == active_workspace_id {
            (store.canvas_notes.len(), store.canvas_links.len())
        } else {
            let workspace = store
                .canvas_workspaces
                .get(&target_workspace_id)
                .ok_or_else(|| anyhow!("生成来源所在画板已不存在"))?;
            (workspace.notes.len(), workspace.links.len())
        };
        if !replaces_loading_placeholder && (node_count >= 200 || link_count >= 400) {
            return Err(anyhow!("画布已达到容量上限"));
        }
        target_workspace_id
    };
    let source_path = save_generated_bytes(app, &bytes, raw_prompt)?;
    let mut store = context.store.borrow_mut();
    let active_workspace_id = normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
    let target_is_active = target_workspace_id == active_workspace_id;
    let (target_notes, target_links) = if target_is_active {
        let store = &mut *store;
        (&mut store.canvas_notes, &mut store.canvas_links)
    } else {
        let workspace = store
            .canvas_workspaces
            .get_mut(&target_workspace_id)
            .ok_or_else(|| anyhow!("生成来源所在画板已不存在"))?;
        (&mut workspace.notes, &mut workspace.links)
    };

    let source = target_notes
        .iter()
        .find(|note| note.id == source_node_id)
        .cloned()
        .ok_or_else(|| anyhow!("生成来源所在画板已不存在"))?;

    if target_is_active {
        context
            .canvas_history
            .borrow_mut()
            .record(CanvasSnapshot {
                notes: target_notes.clone(),
                links: target_links.clone(),
            });
    }

    if replace_canvas_generation_placeholder(
        target_notes,
        source_node_id,
        &loading_node_id,
        &source_path,
        width as f32,
        height as f32,
        result_index,
    ) {
        insert_canvas_generated_asset(
            &mut store,
            raw_prompt,
            display_prompt,
            mode,
            quality,
            image_model,
            origin,
            conversation_id,
            time,
            &source_path,
            reference_paths,
            width as i32,
            height as i32,
            upscale_done,
        );
        save_local_store(app, &store);
        let state = app.global::<AppState>();
        state.set_canvas_generation_loading_node_id("".into());
        if target_is_active {
            state.set_canvas_selected_id("".into());
            state.set_canvas_selected_count(0);
            push_canvas_notes(app, &store);
            state.set_canvas_can_undo(context.canvas_history.borrow().can_undo());
            state.set_canvas_can_redo(context.canvas_history.borrow().can_redo());
        }
        push_generations(app, &store);
        return Ok(source_path);
    }

    let mut result = CanvasNoteData {
        id: Uuid::new_v4().to_string(),
        kind: "image".to_string(),
        content: String::new(),
        image_path: source_path.clone(),
        width: 340.0,
        height: 250.0,
        parent_group_id: String::new(),
        z_index: target_notes
            .iter()
            .map(|note| note.z_index)
            .max()
            .unwrap_or(0)
            + 1,
        selected: false,
        ..CanvasNoteData::default()
    };
    fit_image_node_to_intrinsic_aspect(&mut result, width as f32, height as f32);
    let (x, y) = generated_canvas_result_position(
        Some(&source),
        result.width,
        result.height,
        result_index,
        total_count,
    );
    (result.x, result.y) = nearest_free_canvas_position(
        target_notes,
        x,
        y,
        result.width,
        result.height,
        None,
    );
    let result_id = result.id.clone();

    target_notes.push(result);
    let _ = connect_nodes(target_links, source_node_id, &result_id);
    insert_canvas_generated_asset(
        &mut store,
        raw_prompt,
        display_prompt,
        mode,
        quality,
        image_model,
        origin,
        conversation_id,
        time,
        &source_path,
        reference_paths,
        width as i32,
        height as i32,
        upscale_done,
    );
    save_local_store(app, &store);
    if target_is_active {
        push_canvas_notes(app, &store);
        let state = app.global::<AppState>();
        state.set_canvas_can_undo(context.canvas_history.borrow().can_undo());
        state.set_canvas_can_redo(context.canvas_history.borrow().can_redo());
    }
    push_generations(app, &store);
    Ok(source_path)
}

#[allow(clippy::too_many_arguments)]
fn insert_canvas_generated_asset(
    store: &mut Store,
    raw_prompt: &str,
    display_prompt: &str,
    mode: &str,
    quality: &str,
    image_model: &str,
    origin: &str,
    conversation_id: &str,
    time: &str,
    source_path: &str,
    reference_paths: &[String],
    width: i32,
    height: i32,
    upscale_done: bool,
) {
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.to_string(),
        title: short_text(raw_prompt, 18),
        category: "other".to_string(),
        kind: mode.to_string(),
        time: time.to_string(),
        prompt: display_generation_prompt(display_prompt),
        ratio: ratio_from_actual_dimensions(width, height),
        quality: quality.to_string(),
        model: image_model.to_string(),
        origin: origin.to_string(),
        width,
        height,
        source_path: source_path.to_string(),
        reference_paths: reference_paths.to_vec(),
        cutout_done: false,
        remove_black_done: false,
        upscale_done,
        is_new: true,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    store.assets.insert(0, item);
}

fn replace_canvas_generation_placeholder(
    notes: &mut [CanvasNoteData],
    source_node_id: &str,
    loading_node_id: &str,
    source_path: &str,
    image_width: f32,
    image_height: f32,
    result_index: i32,
) -> bool {
    if result_index != 0 || loading_node_id != source_node_id {
        return false;
    }
    let Some(source_index) = notes.iter().position(|note| {
        note.id == source_node_id && note.kind == "image" && note.image_path.trim().is_empty()
    }) else {
        return false;
    };

    let (desired_x, desired_y, width, height, source_id) = {
        let source = &mut notes[source_index];
        let center_x = source.x + source.width / 2.0;
        let center_y = source.y + source.height / 2.0;
        source.content.clear();
        source.image_path = source_path.to_string();
        source.width = 340.0;
        source.height = 250.0;
        fit_image_node_to_intrinsic_aspect(source, image_width, image_height);
        source.selected = false;
        (
            center_x - source.width / 2.0,
            center_y - source.height / 2.0,
            source.width,
            source.height,
            source.id.clone(),
        )
    };
    let (x, y) = nearest_free_canvas_position(
        notes,
        desired_x,
        desired_y,
        width,
        height,
        Some(&source_id),
    );
    notes[source_index].x = x;
    notes[source_index].y = y;
    true
}

pub(super) fn discard_canvas_generation_placeholder(
    state: &AppState,
    destination: &GenerationDestination,
) {
    let GenerationDestination::Canvas { source_node_id } = destination else {
        return;
    };
    if state.get_canvas_generation_loading_node_id().as_str() != source_node_id {
        return;
    }
    state.set_canvas_generation_loading_node_id("".into());
    state.invoke_remove_canvas_node(source_node_id.clone().into());
}

pub(super) fn canvas_workspace_id_for_source(
    store: &Store,
    source_node_id: &str,
) -> Option<String> {
    let active_workspace_id = normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
    if store
        .canvas_notes
        .iter()
        .any(|note| note.id == source_node_id)
    {
        return Some(active_workspace_id.clone());
    }
    store
        .canvas_workspaces
        .iter()
        .find(|(workspace_id, workspace)| {
            *workspace_id != &active_workspace_id
                && workspace
                    .notes
                    .iter()
                    .any(|note| note.id == source_node_id)
        })
        .map(|(workspace_id, _)| workspace_id.clone())
}

pub(super) fn upsert_stream_failure_card(generations: &mut Vec<AssetData>, card: AssetData) {
    if card.delivery_recoverable {
        generations.retain(|existing| existing.id != card.id);
    }
    generations.insert(0, card);
}

pub(super) fn add_stream_failure_item(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    raw_prompt: &str,
    category: &str,
    mode: &str,
    ratio: &str,
    quality: &str,
    image_model: &str,
    origin: &str,
    conversation_id: &str,
    reason: &str,
    time: &str,
    reference_paths: &[String],
    failed_asset_id: Option<&str>,
) -> String {
    let asset_id = failed_asset_id
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let delivery_recoverable = failed_asset_id.is_some();
    let mut store_mut = store.borrow_mut();
    reveal_prompt_history_entry(&mut store_mut, raw_prompt);
    upsert_stream_failure_card(
        &mut store_mut.generations,
        AssetData {
            id: asset_id.clone(),
            conversation_id: conversation_id.to_string(),
            title: short_text(raw_prompt, 18),
            category: category.to_string(),
            kind: mode.to_string(),
            time: time.to_string(),
            prompt: raw_prompt.to_string(),
            ratio: ratio.to_string(),
            quality: quality.to_string(),
            model: image_model.to_string(),
            origin: origin.to_string(),
            width: 0,
            height: 0,
            source_path: "failed".to_string(),
            reference_paths: reference_paths.to_vec(),
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
            delivery_recoverable,
            delivery_downloading: false,
        },
    );
    store_mut.notifications.insert(
        0,
        NotificationData {
            id: Uuid::new_v4().to_string(),
            title: format!("Generation failed: {}", short_text(raw_prompt, 24)),
            model: image_model.to_string(),
            time: time.to_string(),
            reason: reason.to_string(),
            success: false,
            read: false,
        },
    );
    save_local_store(app, &store_mut);
    push_all(app, &store_mut);
    asset_id
}

pub(super) fn restore_stream_inputs(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    category: &str,
    original_references: Vec<ReferenceData>,
    original_quote: QuoteContext,
) {
    let state = app.global::<AppState>();
    let mut store_mut = store.borrow_mut();
    if current_workspace_category(app) == category {
        state.set_quote_title(original_quote.title.into());
        state.set_quote_prompt(original_quote.prompt.into());
        state.set_quote_ratio(original_quote.ratio.into());
        state.set_quote_quality(original_quote.quality.into());
        state.set_quote_width(original_quote.width);
        state.set_quote_height(original_quote.height);
    }
    *references_for_category_mut(&mut store_mut.references, category) = original_references;
    save_local_store(app, &store_mut);
    push_all(app, &store_mut);
}

pub(super) fn set_stream_final_status(
    context: &AppContext,
    app: &AppWindow,
    category: &str,
    success_count: i32,
    failed_count: i32,
    failure_reason: Option<&str>,
) {
    if failed_count <= 0 {
        set_generation_status_for_category(context, app, category, "生成成功");
    } else if success_count > 0 {
        let status = failure_reason
            .filter(|reason| !reason.trim().is_empty())
            .map(|reason| format!("部分生成失败：{reason}"))
            .unwrap_or_else(|| "部分生成失败".to_string());
        set_generation_status_for_category(context, app, category, &status);
    } else {
        set_generation_status_for_category(
            context,
            app,
            category,
            failure_reason
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or("生成失败"),
        );
    }
}
