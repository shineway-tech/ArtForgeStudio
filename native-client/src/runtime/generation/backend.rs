use super::*;

pub(super) struct CapturedImageEditBrushPoint {
    pub x:f32,pub y:f32,pub size:f32,pub shape:String,
}
pub(super) struct CapturedImageEditRequest {
    pub prompt:String,pub model_code:String,pub quality:String,
    pub estimated_credit_cost:i32,pub category:String,pub mode:String,pub conversation_id:String,
}
pub(super) struct PreparedPaidImageFile {
    authority:Arc<NamespaceStorageAuthority>,
    file:NamespaceManagedFile,path:PathBuf,indexed:ManagedFileRecord,pub(super) sha256:String,pub(super) size:u64,
}
impl PreparedPaidImageFile {
    // Worker-only. The held physical identity, index row and full immutable
    // content are checked again before the retained request may be created.
    fn ensure_current(&mut self,authority:&NamespaceStorageAuthority)->Result<()> {
        anyhow::ensure!(authority.lease()==self.authority.lease(),"paid image namespace changed");
        // A held file belongs to the exact NamespaceFs that opened it, not a
        // freshly opened authority for the same lease. Retain that owner.
        let authority=&self.authority;
        let metadata=authority.inspect_regular(&self.file)?;
        anyhow::ensure!(metadata.identity==self.indexed.physical_identity && metadata.link_count==1
            && metadata.byte_size==self.size,"paid image file changed");
        let indexed=authority.delivery_index()?.find_file_by_path_for_namespace(authority,self.file.key().area(),
            self.file.key().relative_name().as_str())?.ok_or_else(||anyhow!("paid image index missing"))?;
        anyhow::ensure!(indexed.id==self.indexed.id && indexed.physical_identity==self.indexed.physical_identity
            && !indexed.pending_delete && indexed.byte_size==self.size,"paid image index changed");
        // The upload producer separately checks these exact fingerprints on its
        // same held read; this check never authorizes a later unchecked reopen.
        let bytes=authority.with_regular_reader(&mut self.file,|reader|{
            use std::io::Read;
            let mut bytes=Vec::new();reader.take(100*1024*1024+1).read_to_end(&mut bytes)?;Ok(bytes)
        })?;
        anyhow::ensure!(bytes.len() as u64==self.size && paid_image_sha(&bytes)==self.sha256,"paid image content changed");
        Ok(())
    }
}
pub(super) struct PreparedImageEditInputs {
    persistence:PrivatePersistence,original:PreparedPaidImageFile,
    inputs:Vec<PreparedPaidImageFile>,width:u32,height:u32,
}
pub(super) struct PreparedAssetRegeneration {
    persistence:PrivatePersistence,item:AssetData,
    references:Vec<PreparedPaidImageFile>,
}
fn paid_image_sha(bytes:&[u8])->String {use sha2::Digest;format!("{:x}",sha2::Sha256::digest(bytes))}
fn paid_image_key(lease:&NamespaceLease,path:&Path)->Result<ManagedFileKey> {
    anyhow::ensure!(path.is_absolute(),"paid image requires owned absolute path");
    let mut areas=vec![ManagedUserArea::Input,ManagedUserArea::Output,ManagedUserArea::Prompt,
        ManagedUserArea::Canvas,ManagedUserArea::CanvasUploads,ManagedUserArea::CanvasExports,
        ManagedUserArea::References,ManagedUserArea::ReferencesLibrary,ManagedUserArea::ReferencesImports,
        ManagedUserArea::ToolboxCompressionInputs,ManagedUserArea::ToolboxCompressionResults,
        ManagedUserArea::ToolboxConversionInputs,ManagedUserArea::ToolboxConversionResults,ManagedUserArea::ToolboxCropInputs];
    areas.sort_by_key(|area|std::cmp::Reverse(lease.namespace.path(*area).components().count()));
    areas.into_iter().find_map(|area|path.strip_prefix(lease.namespace.path(area)).ok()
        .and_then(|name|name.to_str()).and_then(|name|ManagedFileKey::new(area,name).ok()))
        .ok_or_else(||anyhow!("paid image source is outside owned image areas"))
}
pub(super) fn capture_paid_image_file(authority:&Arc<NamespaceStorageAuthority>,path:&Path)->Result<(PreparedPaidImageFile,Vec<u8>)> {
    use std::io::Read;
    let key=paid_image_key(authority.lease(),path)?;
    let mut file=authority.open_existing_regular(&key)?;
    let metadata=authority.inspect_regular(&file)?;
    anyhow::ensure!(metadata.link_count==1 && metadata.byte_size>0 && metadata.byte_size<=100*1024*1024,"paid image size or links invalid");
    let indexed=authority.delivery_index()?.find_file_by_path_for_namespace(authority,key.area(),key.relative_name().as_str())?
        .ok_or_else(||anyhow!("paid original image index missing"))?;
    anyhow::ensure!(!indexed.pending_delete && indexed.physical_identity==metadata.identity && indexed.byte_size==metadata.byte_size,
        "paid original image index mismatch");
    let bytes=authority.with_regular_reader(&mut file,|reader|{
        let mut bytes=Vec::new();reader.take(100*1024*1024+1).read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() as u64==metadata.byte_size,"paid image changed during read");Ok(bytes)
    })?;
    let sha256=paid_image_sha(&bytes);
    Ok((PreparedPaidImageFile{authority:authority.clone(),file,path:path.to_owned(),indexed,sha256,size:metadata.byte_size},bytes))
}
fn decode_paid_image_bytes(path:&Path,bytes:&[u8])->Result<image::DynamicImage>{
    let mut reader=image::ImageReader::new(std::io::Cursor::new(bytes));
    if let Ok(format)=image::ImageFormat::from_path(path){reader.set_format(format);}
    match reader.with_guessed_format()?.into_dimensions(){
        Ok((w,h))=>anyhow::ensure!(w>0 && h>0 && u64::from(w)*u64::from(h)<=100_000_000,"paid image dimensions exceed policy"),
        Err(error)=>{
            #[cfg(target_os="macos")]
            if path.extension().and_then(|extension|extension.to_str()).is_some_and(|extension|
                matches!(extension.to_ascii_lowercase().as_str(),"heic"|"heif")) {
                // Existing NSData-native fallback cannot inspect dimensions first.
                let image=decode_image_bytes(path,bytes)?.0;
                anyhow::ensure!(u64::from(image.width())*u64::from(image.height())<=100_000_000,"native paid image too large");
                return Ok(image);
            }
            return Err(error.into());
        }
    }
    Ok(decode_image_bytes(path,bytes)?.0)
}
fn publish_paid_edit_input(authority:&Arc<NamespaceStorageAuthority>,prefix:&str,bytes:&[u8])->Result<PreparedPaidImageFile>{
    let _mutation=authority.begin_ordinary_mutation()?;
    let key=ManagedFileKey::new(ManagedUserArea::Input,&format!("image-edit-{prefix}-{}.png",Uuid::new_v4()))?;
    let mut file=authority.create_temporary_regular_for(&key)?;
    authority.write_new_regular_from(&mut file,&mut std::io::Cursor::new(bytes))?;
    authority.sync_regular(&mut file)?;authority.publish_regular(&mut file,NamespaceManagedPublication::Absent(&key))?;
    let registration=NamespacedManagedFileRegistration::new(authority,file,"reference","user")?;
    authority.delivery_index()?.register_file_for_namespace(authority,&registration)?;
    let path=authority.lease().namespace.path(key.area()).join(key.relative_name().as_str());
    capture_paid_image_file(authority,&path).map(|(proof,_)|proof)
}
pub(super) fn prepare_image_edit_inputs_for_namespace(
    persistence:&PrivatePersistence,source_path:&Path,points:Vec<CapturedImageEditBrushPoint>
)->Result<PreparedImageEditInputs>{
    let _effect=persistence.begin_effect()?;let authority=persistence.storage_authority()?;
    anyhow::ensure!(points.len()<=25_000 && points.iter().all(|point|point.x.is_finite() && point.y.is_finite()
        && point.size.is_finite() && matches!(point.shape.as_str(),"square"|"circle")),"invalid captured brush points");
    let(mut original,bytes)=capture_paid_image_file(&authority,source_path)?;
    let mut source=decode_paid_image_bytes(source_path,&bytes)?.to_rgba8();
    if source.width().max(source.height())>4096 {
        source=image::DynamicImage::ImageRgba8(source).resize(4096,4096,image::imageops::FilterType::Lanczos3).to_rgba8();
    }
    let mut source_bytes=encode_png_rgba(&source,source.width(),source.height())?;
    while source_bytes.len()>7_500_000 && source.width().max(source.height())>1024 {
        source=image::imageops::resize(&source,((source.width() as f32*0.82).round() as u32).max(1),
            ((source.height() as f32*0.82).round() as u32).max(1),image::imageops::FilterType::Lanczos3);
        source_bytes=encode_png_rgba(&source,source.width(),source.height())?;
    }
    anyhow::ensure!(source_bytes.len()<=7_500_000,"image-edit source exceeds paired upload limit");
    let brush=points.into_iter().map(|point|BrushPoint{x:point.x,y:point.y,size:point.size,shape:point.shape.into(),..Default::default()}).collect::<Vec<_>>();
    let mask=viewer_callbacks::rasterize_image_edit_mask(&brush,source.width(),source.height())?;
    let mask_bytes=encode_png_rgba(&mask,mask.width(),mask.height())?;
    let inputs=vec![publish_paid_edit_input(&authority,"source",&source_bytes)?,publish_paid_edit_input(&authority,"mask",&mask_bytes)?];
    original.ensure_current(&authority)?;
    Ok(PreparedImageEditInputs{persistence:persistence.clone(),original,inputs,width:source.width(),height:source.height()})
}
pub(super) fn prepare_asset_regeneration_for_namespace(
    persistence:&PrivatePersistence,item:AssetData
)->Result<PreparedAssetRegeneration>{
    let _effect=persistence.begin_effect()?;let authority=persistence.storage_authority()?;
    let category=resolve_category(&item.category,&item.prompt);
    anyhow::ensure!(!item.model.trim().is_empty() && !item.prompt.trim().is_empty()
        && item.reference_paths.len()<=max_reference_images_for_category(&category),"original generation metadata incomplete");

    let mut references=Vec::with_capacity(item.reference_paths.len());
    for path in &item.reference_paths {
        let(proof,bytes)=capture_paid_image_file(&authority,Path::new(path))?;
        decode_paid_image_bytes(Path::new(path),&bytes)?;references.push(proof);
    }
    Ok(PreparedAssetRegeneration{persistence:persistence.clone(),item,references})
}

// Proof-consuming paid entry points. No UI/file reads in request construction.
fn paid_viewer_record(scope:&BillingScope,prompt:String,model:String,quality:String,category:String,mode:String,
    ratio:String,conversation:String,task_type:&str,width:u32,height:u32,references:&[PreparedPaidImageFile],lineage:Vec<String>,source_id:String)
    ->PendingGenerationRecord {
    PendingGenerationRecord {
        source_asset_id:source_id,video_request:None,schema_version:2,cancel_requested:false,
        created_at_epoch_ms:Local::now().timestamp_millis(),client_request_id:Uuid::new_v4().to_string(),
        owner_user_id:scope.request.session.owner_user_id.clone(),billing_account_group_id:scope.request.account_group_id.clone(),
        auth_epoch:scope.request.session.auth_epoch,local_task_id:Uuid::new_v4().to_string(),server_task_id:String::new(),
        raw_prompt:prompt.clone(),generation_prompt:prompt,task_type:task_type.into(),category,mode,ratio,quality,model_code:model,
        conversation_id:conversation,count:1,target_width:width,target_height:height,create_conversation:false,
        reference_paths:references.iter().map(|file|file.path.to_string_lossy().into_owned()).collect(),
        reference_sha256:references.iter().map(|file|file.sha256.clone()).collect(),reference_size_bytes:references.iter().map(|file|file.size).collect(),
        lineage_reference_paths:lineage,uploaded_file_ids:Vec::new(),deliveries:Vec::new(),terminal:false,expected_success_count:0,
        canvas_source_node_id:String::new(),canvas_ui_extraction:false,
    }
}
pub(super) fn start_backend_image_edit_with_prepared_inputs(
    app:&AppWindow,context:AppContext,authority:Arc<NamespaceStorageAuthority>,billing_scope:&BillingScope,
    original:Option<AssetData>,mut prepared:PreparedImageEditInputs,request:CapturedImageEditRequest,
) {
    let persistence=prepared.persistence.clone();
    let record=paid_viewer_record(billing_scope,request.prompt,request.model_code,request.quality,
        request.category,request.mode,ratio_from_actual_dimensions(prepared.width as i32,prepared.height as i32),
        if request.conversation_id.trim().is_empty(){Uuid::new_v4().to_string()}else{request.conversation_id},
        "image_edit",prepared.width,prepared.height,&prepared.inputs,vec![prepared.original.path.to_string_lossy().into_owned()],
        original.as_ref().map(|item|item.id.clone()).unwrap_or_default());
    let validate=move|authority:&NamespaceStorageAuthority|{
        anyhow::ensure!(original.as_ref().is_none_or(|item|Path::new(&item.source_path)==prepared.original.path),
            "image-edit original metadata changed");
        prepared.original.ensure_current(authority)?;
        anyhow::ensure!(prepared.inputs.len()==2,"image-edit pair is incomplete");
        for input in &mut prepared.inputs{input.ensure_current(authority)?;}
        Ok(())
    };
    start_paid_viewer_record(app,context,persistence,authority,billing_scope,record,request.estimated_credit_cost,validate);
}
pub(super) fn start_asset_regeneration_with_prepared_inputs(
    app:&AppWindow,context:AppContext,authority:Arc<NamespaceStorageAuthority>,billing_scope:&BillingScope,
    mut prepared:PreparedAssetRegeneration,
)->bool {
    let persistence=prepared.persistence.clone();let item=&prepared.item;
    let record=paid_viewer_record(billing_scope,item.prompt.clone(),item.model.clone(),item.quality.clone(),
        resolve_category(&item.category,&item.prompt),item.kind.clone(),item.ratio.clone(),
        if item.conversation_id.trim().is_empty(){Uuid::new_v4().to_string()}else{item.conversation_id.clone()},
        "image_generation",0,0,&prepared.references,item.reference_paths.clone(),item.id.clone());
    let validate=move|authority:&NamespaceStorageAuthority|{
        for reference in &mut prepared.references{reference.ensure_current(authority)?;}
        Ok(())
    };
    start_paid_viewer_record(app,context,persistence,authority,billing_scope,record,0,validate)
}
pub(super) fn start_backend_video(
    app: &AppWindow,
    context: AppContext,
    persistence: PrivatePersistence,
    authority: Arc<NamespaceStorageAuthority>,
    scope: &BillingScope,
    request: CreateVideoGenerationTask,
    source_id: String,
) -> bool {
    let mut record = paid_viewer_record(
        scope,
        request.prompt.clone(),
        request.model_code.clone(),
        request.resolution.clone(),
        "other".into(),
        "game".into(),
        request.aspect_ratio.clone(),
        Uuid::new_v4().to_string(),
        "image_to_video",
        0,
        0,
        &[],
        vec![],
        source_id,
    );
    record.client_request_id = request.client_request_id.clone();
    record.uploaded_file_ids = request.reference_file_ids.clone();
    record.video_request = Some(request);
    start_paid_viewer_record(
        app,
        context,
        persistence,
        authority,
        scope,
        record,
        0,
        |_| Ok(()),
    )
}

fn start_paid_viewer_record(
    app:&AppWindow,context:AppContext,persistence:PrivatePersistence,authority:Arc<NamespaceStorageAuthority>,
    scope:&BillingScope,record:PendingGenerationRecord,credit_cost:i32,
    validate:impl FnOnce(&NamespaceStorageAuthority)->Result<()>+Send+'static,
)->bool {
    let Some(backend)=context.backend.clone()else{return false;};
    if authority.lease()!=persistence.lease() || !persistence.is_current()
        || !context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.same_binding_metadata(&persistence)){return false;}
    let Ok(scope)=capture_billing_scope_for_submission(Some(&backend),&authority,scope)else{return false;};
    let _activity=match persistence.begin_activity(){Ok(activity)=>activity,Err(_)=>return false};
    if record.model_code.trim().is_empty() || record.generation_prompt.trim().is_empty(){return false;}
    let Ok(write)=persistence.prepare_ordered_save()else{return false;};let mut write=Some(write);
    let queued=context.apply_user_completion(persistence.lease(),||{
        if !paid_viewer_binding_matches(&context,&persistence) || !context.billing_context.is_current(&scope) || category_is_generating(&context,&record.category){return None;}
        insert_active_generation(&context,ActiveGeneration {
            task_id:record.local_task_id.clone(),client_request_id:Some(record.client_request_id.clone()),server_task_id:None,
            category:record.category.clone(),conversation_id:record.conversation_id.clone(),prompt:record.raw_prompt.clone(),
            credit_cost,total_count:1,loading_count:1,completed_count:0,success_count:0,failed_count:0,last_failure_reason:None,
            progress:1,eta:0,latest_success_id:None,session_scope:scope.request.session.clone(),
            destination:GenerationDestination::Gallery,delivery_download_reservations:Vec::new(),
            registered_cancel_owner:Some(persistence.clone()),
        });
        let state=app.global::<AppState>();state.set_viewer_open(false);state.set_viewer_message("".into());
        state.set_image_editor_generating(false);
        state.set_page(if record.video_request.is_some() { "video-generation" } else { "generation" }.into());
        if record.video_request.is_some() { state.set_video_generating(true); state.set_video_status("正在提交视频任务…".into()); }
        state.set_asset_type(record.category.clone().into());
        state.set_current_conversation_id(record.conversation_id.clone().into());
        state.set_ratio(record.ratio.clone().into());state.set_quality(record.quality.clone().into());state.set_mode(record.mode.clone().into());
        state.set_image_model(record.model_code.clone().into());
        if record.task_type=="image_generation" {
            *references_for_category_mut(&mut context.store.borrow_mut().references,&record.category)=record.reference_paths.iter()
                .map(|path|ReferenceData{id:Uuid::new_v4().to_string(),source_path:path.clone()}).collect();
        }
        state.set_generation_status("正在保存原请求并准备提交...".into());
        sync_generation_state_for_current_category(&context,app);
        Some(write.take().unwrap().enqueue(local_store_data(app,&context.store.borrow())))
    });
    drop(write);
    let store_ack=match queued {
        Ok(Some(Ok(receiver)))=>receiver,
        Ok(Some(Err(error)))=>{
            drop(error);
            let _=context.apply_user_completion(persistence.lease(),||{
                remove_active_generation(&context,&record.category,&record.local_task_id);
                if record.video_request.is_some() { app.global::<AppState>().set_video_generating(false); }
                sync_generation_state_for_current_category(&context,app);
                set_generation_status_for_category(&context,app,&record.category,"原输入保存未确认，尚未提交收费请求");
            });return false;
        },
        _=>return false,
    };
    let visuals=prepare_delivery_visuals(app,&context.store.borrow());
    let references=prepare_category_reference_projection(app,&context.store.borrow(),&record.category);
    let effects=context.apply_user_completion(persistence.lease(),||{
        if !paid_viewer_binding_matches(&context,&persistence){return None;}
        Some((visuals.publish_metadata(app,persistence.clone()),references.publish_metadata(app)))
    });
    let Ok(Some((visuals,references)))=effects else{return false;};
    start_activation_visual_effects(app,context.clone(),visuals);
    start_canvas_reference_preview_effects(app,persistence.clone(),references);
    let cancellations=context.cancelled_generation_requests.clone();
    let worker_record=record.clone();let worker_scope=scope.clone();
    let (sender,outcomes)=mpsc::channel();
    let spawned=spawn_delivery_preparation(&persistence,move|persistence,activity,cancel|{
        if cancel.load(Ordering::SeqCst) || activity.is_quiescing(){return Err(DeliveryRetryError::AuthenticationRequired);}
        if persistence.lease()!=authority.lease(){return Err(anyhow!("paid original namespace changed").into());}
        store_ack.recv().map_err(|_|anyhow!("paid input Store acknowledgement disconnected"))?
            .map_err(anyhow::Error::from)?;
        validate(&authority)?;
        // No source or payer may be recaptured after this point.
        capture_billing_scope_for_submission(Some(&backend),&authority,&worker_scope)?;
        upsert_pending_generation_for_namespace(&authority,&worker_scope,worker_record.clone())?;
        run_generation_record_checked(backend,authority,Some(worker_scope.clone()),worker_scope.request.session.clone(),
            worker_record,sender,cancellations,Some(cancel.clone()))?;
        Ok(())
    });
    match spawned {
        Ok((cancel,finished))=>poll_paid_viewer_record(app.as_weak(),context,persistence,record,cancel,finished,outcomes,Vec::new(),Instant::now()),
        Err(_)=>{let _=context.apply_user_completion(persistence.lease(),||{
            remove_active_generation(&context,&record.category,&record.local_task_id);
                if record.video_request.is_some() { app.global::<AppState>().set_video_generating(false); }
            sync_generation_state_for_current_category(&context,app);
            set_generation_status_for_category(&context,app,&record.category,"原请求工作未能启动，请重试");
        });return false;}
    }
    true
}
fn poll_paid_viewer_record(
    weak:Weak<AppWindow>,context:AppContext,persistence:PrivatePersistence,record:PendingGenerationRecord,
    cancel:Arc<std::sync::atomic::AtomicBool>,finished:mpsc::Receiver<std::result::Result<(),DeliveryRetryError>>,
    outcomes:mpsc::Receiver<GenerationOutcome>,mut retained:Vec<GenerationOutcome>,started:Instant,
) {
    slint::Timer::single_shot(Duration::from_millis(80),move||{
        let app=weak.upgrade();let bound=persistence.is_current()
            && context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.same_binding_metadata(&persistence));
        if app.is_none() || !bound {
            cancel.store(true,Ordering::SeqCst);
        }
        let joined=finish_delivery_preparation(&cancel);
        if joined.is_err(){
            cancel.store(true,Ordering::SeqCst);
        }
        if app.is_some() && bound && joined.is_ok(){
            let app=app.as_ref().unwrap();
            let mut latest=None;
            for outcome in outcomes.try_iter(){
                match outcome {
                    GenerationOutcome::Progress{percent}=>latest=Some(percent),
                    GenerationOutcome::Accepted{task_id}=>{
                        if record.video_request.is_some() { app.global::<AppState>().set_video_task_id(task_id.clone().into()); app.global::<AppState>().set_video_status("视频任务已提交，正在生成…".into()); }
                        let _=context.apply_user_completion(persistence.lease(),||{
                            if let Some(active)=context.generations.active.borrow_mut().get_mut(&record.category){
                                if active.task_id==record.local_task_id {active.server_task_id=Some(task_id);}
                            }
                        });
                    },
                    other=>retained.push(other),
                }
            }
            if let Some(percent)=latest { if record.video_request.is_some() { app.global::<AppState>().set_video_progress(percent.clamp(1,99)); } let _=context.apply_user_completion(persistence.lease(),||
                update_active_generation_progress(&context,app,&record.category,&record.local_task_id,percent.clamp(1,99),0));}
        }
        if matches!(joined,Ok(true)){
            poll_paid_viewer_record(weak,context,persistence,record,cancel,finished,outcomes,retained,started);return;
        }
        let result=if joined.is_ok(){finished.try_recv().ok()}else{None};
        let terminal=result.as_ref().is_some_and(|result|matches!(result,Err(DeliveryRetryError::Api(error)) if error.is_terminal_session_error()));
        if terminal {
            let original=SessionScope{owner_user_id:record.owner_user_id.clone(),auth_epoch:record.auth_epoch};
            if let Some(app)=app.as_ref(){
                if context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.same_binding_metadata(&persistence))
                    && terminal_auth_scope_matches_context(&context,&original){
                    drop(result);drop(retained);drop(outcomes);
                    sign_out_locally(app,&context,true,Some(original.auth_epoch));
                }
            }
            return;
        }
        let Some(app)=app.filter(|_|bound)else{return;};
        let success=matches!(result,Some(Ok(())));
        if !success || retained.is_empty(){
            retained.clear();retained.push(GenerationOutcome::Failure{reason:"原请求尚未完成，恢复记录已保留".into(),time:Local::now().format("%Y-%m-%d %H:%M").to_string()});
        }
        finish_paid_viewer_results(app.as_weak(),context,persistence,record,retained);
    });
}

pub(super) fn paid_viewer_binding_matches(context:&AppContext,persistence:&PrivatePersistence)->bool {
    context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.same_binding_metadata(persistence))
}
fn paid_viewer_task_matches(context:&AppContext,record:&PendingGenerationRecord)->bool {
    active_generation_matches_scope(context,&record.category,&record.local_task_id,
        &SessionScope{owner_user_id:record.owner_user_id.clone(),auth_epoch:record.auth_epoch})
}
fn finish_paid_viewer_results(
    weak:Weak<AppWindow>,context:AppContext,persistence:PrivatePersistence,record:PendingGenerationRecord,
    mut outcomes:Vec<GenerationOutcome>,
){
    // Both initial and recursive consumption occur outside a caller's completion.
    slint::Timer::single_shot(Duration::ZERO,move||{
        let Some(app)=weak.upgrade()else{return;};
        if !persistence.is_current() || !paid_viewer_binding_matches(&context,&persistence){return;}
        let scope=SessionScope{owner_user_id:record.owner_user_id.clone(),auth_epoch:record.auth_epoch};
        if !active_generation_matches_scope(&context,&record.category,&record.local_task_id,&scope){return;}
        let outcome=if outcomes.is_empty(){GenerationOutcome::Finished}else{outcomes.remove(0)};
        match outcome {
            GenerationOutcome::NamespaceVideoSuccess { prepared, time } => {
                let original = persistence.clone(); let original_record = record.clone();
                start_video_delivery_commit_with_binding(&app, context.clone(), Some(persistence), *prepared, time, move |app, result| {
                    if !paid_viewer_binding_matches(&context, &original) { return; }
                    let state = app.global::<AppState>(); state.set_video_generating(false);
                    match result {
                        Ok((_, id, acknowledged)) => {
                            if let Some(output) = context.store.borrow().video_outputs.get(&id) { state.set_video_result_path(output.source_path.clone().into()); }
                            state.set_video_progress(100);
                            state.set_video_status(if acknowledged { "视频已生成并保存" } else { "视频已保存，交付确认待重试" }.into());
                            mark_active_generation_image_completed(&context, app, &record.category, &record.local_task_id, true, Some(id), None);
                            push_video_assets(app, &context.store.borrow());
                        }
                        Err(_) => { state.set_video_status("视频保存尚未完成，恢复记录已保留".into()); }
                    }
                    finish_paid_viewer_results(app.as_weak(), context, original, original_record, outcomes);
                });
            },
            GenerationOutcome::NamespaceImageSuccess{prepared,time}=>{
                let original=persistence.clone();let original_record=record.clone();
                start_image_delivery_commit_captured(&app,context.clone(),persistence,*prepared,time,move|app,result|{
                    // The existing commit invokes this only inside its original
                    // short completion. Do not perform reads or start effects here.
                    if !paid_viewer_binding_matches(&context,&original) || !paid_viewer_task_matches(&context,&record){return;}
                    match result {
                        Ok((_image,id,acknowledged))=>{
                            mark_active_generation_image_completed(&context,app,&record.category,&record.local_task_id,true,Some(id),None);
                            if !acknowledged {set_generation_status_for_category(&context,app,&record.category,"图片已保存，远端交付确认待重试");}
                        },
                        Err(_)=>{
                            mark_active_generation_image_completed(&context,app,&record.category,&record.local_task_id,false,None,Some("图片交付尚未确认，原任务已保留"));
                        }
                    }
                    finish_paid_viewer_results(app.as_weak(),context,original,original_record,outcomes);
                });
            },
            GenerationOutcome::ImageFailure{reason,time,delivery}=>{
                stage_paid_viewer_failure(&app,context,persistence,record,reason,time,
                    delivery.and_then(|delivery|delivery.failed_asset_id),outcomes);
            },
            GenerationOutcome::Failure{reason,time}=>{
                stage_paid_viewer_failure(&app,context,persistence,record,reason,time,None,Vec::new());
            },
            GenerationOutcome::ImageSuccess{..}=>{
                // D/E only accepts the owned proof producer. No raw-path fallback.
                stage_paid_viewer_failure(&app,context,persistence,record,
                    "图片原始交付凭据缺失，任务已保留".into(),Local::now().format("%Y-%m-%d %H:%M").to_string(),None,Vec::new());
            },
            GenerationOutcome::CreditInsufficient{message}=>{
                let _=context.apply_user_completion(persistence.lease(),||{
                    if !paid_viewer_binding_matches(&context,&persistence) || !paid_viewer_task_matches(&context,&record){return;}
                    remove_active_generation(&context,&record.category,&record.local_task_id);
                    sync_generation_state_for_current_category(&context,&app);
                    let state=app.global::<AppState>();
                    if record.video_request.is_some() { state.set_video_generating(false); state.set_video_status(message.user_message().into()); }
                    show_credit_rejection(&state,&message);
                });
                if persistence.is_current() && paid_viewer_binding_matches(&context,&persistence){refresh_backend_snapshot_captured(&app,context,persistence.clone());}
            },
            GenerationOutcome::Finished=>{
                if record.video_request.is_some() { let state=app.global::<AppState>(); state.set_video_generating(false); if state.get_video_result_path().is_empty() { state.set_video_status("服务端任务已结束；视频交付尚未完成时将保留恢复记录".into()); } }
                let latest=context.generations.active.borrow().get(&record.category).and_then(|active|
                    (active.task_id==record.local_task_id).then(||active.latest_success_id.clone())).flatten();
                let viewer=latest.as_ref().and_then(|id|prepare_viewer_projection(&app,&context.store.borrow(),id,"generation"));
                let mut effects=None;
                let _=context.apply_user_completion(persistence.lease(),||{
                    if !paid_viewer_binding_matches(&context,&persistence) || !paid_viewer_task_matches(&context,&record){return;}
                    let Some(task)=remove_active_generation(&context,&record.category,&record.local_task_id)else{return;};
                    set_stream_final_status(&context,&app,&record.category,task.success_count,task.failed_count,task.last_failure_reason.as_deref());
                    sync_generation_state_for_current_category(&context,&app);
                    if let Some(viewer)=viewer{effects=Some(viewer.publish_metadata(&app));}
                });
                if let Some(effects)=effects {start_viewer_preview_effects(&app,context.clone(),persistence.clone(),effects);}
                if persistence.is_current() && paid_viewer_binding_matches(&context,&persistence){refresh_backend_snapshot_captured(&app,context,persistence.clone());}
            },
            GenerationOutcome::Accepted{..}|GenerationOutcome::Progress{..}=>
                finish_paid_viewer_results(app.as_weak(),context,persistence,record,outcomes),
        }
    });
}
fn stage_paid_viewer_failure(
    app:&AppWindow,context:AppContext,persistence:PrivatePersistence,record:PendingGenerationRecord,
    reason:String,time:String,failed_id:Option<String>,remaining:Vec<GenerationOutcome>,
){
    if record.video_request.is_some() {
        let _ = context.apply_user_completion(persistence.lease(), || {
            if !paid_viewer_binding_matches(&context, &persistence) { return; }
            let state = app.global::<AppState>(); state.set_video_generating(false); state.set_video_status(reason.clone().into());
            remove_active_generation(&context, &record.category, &record.local_task_id);
            sync_generation_state_for_current_category(&context, app);
        });
        return;
    }

    let write=match persistence.prepare_ordered_save(){Ok(write)=>write,Err(_)=>{
        finish_paid_viewer_save_failure(app,&context,&persistence,&record);return;
    }};
    let asset=AssetData {
        id:failed_id.clone().unwrap_or_else(||Uuid::new_v4().to_string()),conversation_id:record.conversation_id.clone(),
        title:short_text(&record.raw_prompt,18),category:record.category.clone(),kind:record.mode.clone(),time:time.clone(),
        prompt:record.raw_prompt.clone(),ratio:record.ratio.clone(),quality:record.quality.clone(),model:record.model_code.clone(),
        origin:record.task_type.clone(),width:0,height:0,source_path:"failed".into(),reference_paths:record.lineage_reference_paths.clone(),
        cutout_done:false,remove_black_done:false,upscale_done:false,is_new:false,delivery_recoverable:failed_id.is_some(),delivery_downloading:false,
    };
    let notification=NotificationData{id:Uuid::new_v4().to_string(),title:format!("Generation failed: {}",short_text(&record.raw_prompt,24)),
        model:record.model_code.clone(),time,reason:reason.clone(),success:false,read:false};
    let mut write=Some(write);
    let queued=context.apply_user_completion(persistence.lease(),||{
        if !paid_viewer_binding_matches(&context,&persistence) || !paid_viewer_task_matches(&context,&record){return None;}
        let mut store=context.store.borrow_mut();
        reveal_prompt_history_entry(&mut store,&record.raw_prompt);
        upsert_stream_failure_card(&mut store.generations,asset);store.notifications.insert(0,notification);
        Some(write.take().unwrap().enqueue(local_store_data(app,&store)))
    });
    drop(write);
    let receiver=match queued.ok().flatten(){Some(Ok(receiver))=>receiver,Some(Err(error))=>{
        drop(error);finish_paid_viewer_save_failure(app,&context,&persistence,&record);return;
    },None=>{
        finish_paid_viewer_save_failure(app,&context,&persistence,&record);return;
    }};
    match spawn_delivery_preparation(&persistence,move|_,activity,cancel|{
        receiver.recv().map_err(|_|anyhow!("paid failure save disconnected"))?.map_err(anyhow::Error::from)?;
        if cancel.load(Ordering::Acquire)||activity.is_quiescing(){return Err(DeliveryRetryError::AuthenticationRequired);}
        Ok(())
    }){
        Ok((cancel,result))=>poll_paid_viewer_failure_save(app.as_weak(),context,persistence,record,reason,remaining,cancel,result),
        Err(_)=>finish_paid_viewer_save_failure(app,&context,&persistence,&record),
    }
}
fn finish_paid_viewer_save_failure(app:&AppWindow,context:&AppContext,persistence:&PrivatePersistence,record:&PendingGenerationRecord){
    let _=context.apply_user_completion(persistence.lease(),||{
        if !paid_viewer_binding_matches(context,persistence) || !paid_viewer_task_matches(context,record){return;}
        remove_active_generation(context,&record.category,&record.local_task_id);sync_generation_state_for_current_category(context,app);
        set_generation_status_for_category(context,app,&record.category,"本地保存尚未确认，原任务与暂存数据已保留");
    });
}
fn poll_paid_viewer_failure_save(
    weak:Weak<AppWindow>,context:AppContext,persistence:PrivatePersistence,record:PendingGenerationRecord,
    reason:String,remaining:Vec<GenerationOutcome>,cancel:Arc<std::sync::atomic::AtomicBool>,
    result:mpsc::Receiver<std::result::Result<(),DeliveryRetryError>>,
){
    slint::Timer::single_shot(Duration::from_millis(50),move||{
        if weak.upgrade().is_none() || !persistence.is_current() || !paid_viewer_binding_matches(&context,&persistence){cancel.store(true,Ordering::Release);}
        let finished=finish_delivery_preparation(&cancel);
        if matches!(finished,Ok(true)){poll_paid_viewer_failure_save(weak,context,persistence,record,reason,remaining,cancel,result);return;}
        let Some(app)=weak.upgrade()else{return;};
        if !persistence.is_current() || !paid_viewer_binding_matches(&context,&persistence){return;}
        if finished.is_err() || !matches!(result.try_recv(),Ok(Ok(()))){finish_paid_viewer_save_failure(&app,&context,&persistence,&record);return;}
        let visuals=prepare_delivery_visuals(&app,&context.store.borrow());let mut effects=None;
        let _=context.apply_user_completion(persistence.lease(),||{
            if !paid_viewer_binding_matches(&context,&persistence) || !paid_viewer_task_matches(&context,&record){return;}
            effects=Some(visuals.publish_metadata(&app,persistence.clone()));
            push_notifications(&app,&context.store.borrow());push_prompt_history(&app,&context.store.borrow());
            mark_active_generation_image_completed(&context,&app,&record.category,&record.local_task_id,false,None,Some(&reason));
        });
        if let Some(effects)=effects{start_activation_visual_effects(&app,context.clone(),effects);}
        finish_paid_viewer_results(app.as_weak(),context,persistence,record,remaining);
    });
}

fn cleanup_cancelled_generation_checked(
    backend: &BackendRuntime,
    authority: &NamespaceStorageAuthority,
    api: &GenerationApi,
    session_scope: &SessionScope,
    client_request_id: &str,
    uploaded_file_ids: &[String],
    server_task_id: Option<&str>,
    cancellations: &Arc<Mutex<BTreeSet<String>>>,
) -> std::result::Result<bool,DeliveryRetryError> {
    if !backend_generation_scope_active(backend, session_scope) {
        return Ok(false);
    }
    let result = (|| -> std::result::Result<bool,DeliveryRetryError> {
        let row = load_pending_generations_for_namespace(authority)?.into_iter()
            .find(|row| row.client_request_id == client_request_id).ok_or_else(|| anyhow!("cancellation intent missing"))?;
        if !apply_generation_patch_for_namespace(authority,&row.identity(),GenerationRecoveryPatch::RequestCancellation)? {return Err(anyhow!("cancellation identity changed").into());}
        // No ID is not proof that POST never arrived. Preserve the exact
        // tombstone and inputs; startup may not turn this into new billed work.
        let Some(task_id) = server_task_id.filter(|id| !id.is_empty()) else { return Ok(false); };
        let before = api.task_scoped(task_id, session_scope)?;
        require_saved_group(&row.billing_account_group_id, &before.billing_account_group_id)?;
        if before.id != task_id {return Err(anyhow!("cancel resource identity mismatch").into());}
        api.cancel_scoped(task_id, session_scope)?;
        let after = api.task_scoped(task_id, session_scope)?;
        require_saved_group(&row.billing_account_group_id, &after.billing_account_group_id)?;
        if after.id != task_id {return Err(anyhow!("cancel resource identity mismatch").into());}
        if after.status != "cancelled" || after.success_count != 0 { return Ok(false); }
        for file_id in uploaded_file_ids { api.delete_reference_scoped(file_id, session_scope)?; }
        Ok(remove_pending_generation_for_namespace(authority, &row.identity())?)
    })();
    if !result? { return Ok(false); }
    if let Ok(mut items) = cancellations.lock() {
        items.remove(client_request_id);
    }
    Ok(true)
}

// Companion prerequisite tests; no independent behavioral RED claimed.
#[cfg(test)]
mod core_paid_viewer_input_tests {
    use super::*;
    struct Fixture(video_image_callbacks::tests::scoped_inputs::Fixture);
    impl std::ops::Deref for Fixture {type Target=video_image_callbacks::tests::scoped_inputs::Fixture;fn deref(&self)->&Self::Target{&self.0}}
    impl Drop for Fixture {fn drop(&mut self){
        let workers=drain_delivery_commit_workers_for_lease_for_test(self.persistence.lease());
        let previews=drain_activation_preview_workers_for_lease_for_test(self.persistence.lease());
        let retired=self.context.user_activity.begin_quiesce(self.persistence.lease()).map(|guard|guard.retire());
        if !std::thread::panicking(){workers.unwrap();previews.unwrap();retired.unwrap();}
    }}
    fn fixture()->(Fixture,PathBuf) {
        let f=Fixture(video_image_callbacks::tests::scoped_inputs::Fixture::new());
        let authority=f.authority.clone();
        let path=std::thread::spawn(move||persist_reference_image_for_namespace(&authority,
            &image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(20,10,image::Rgba([17,41,89,255])))).unwrap()).join().unwrap();
        (f,path)
    }
    #[test]
    fn core_paid_image_edit_preparation_registers_exact_source_and_mask_without_global_files(){
        i_slint_backend_testing::init_no_event_loop();let(f,path)=fixture();let p=f.persistence.clone();
        let mut prepared=std::thread::spawn(move||prepare_image_edit_inputs_for_namespace(&p,&path,vec![
            CapturedImageEditBrushPoint{x:0.5,y:0.5,size:0.5,shape:"square".into()}
        ]).unwrap()).join().unwrap();
        assert_eq!((prepared.width,prepared.height),(20,10));
        assert_eq!(prepared.inputs.len(),2);
        for input in &mut prepared.inputs {
            assert_eq!(input.file.key().area(),ManagedUserArea::Input);
            assert!(f.persistence.owns_path(&input.path));
            assert!(input.ensure_current(&f.authority).is_ok());
        }
        let source=decode_reference_bytes(&f.authority.read_image_source(&prepared.inputs[0].path,100_000).unwrap()).unwrap().to_rgba8();
        let mask=decode_reference_bytes(&f.authority.read_image_source(&prepared.inputs[1].path,100_000).unwrap()).unwrap().to_rgba8();
        assert_eq!(*source.get_pixel(10,5),image::Rgba([17,41,89,255]));
        assert_eq!(*mask.get_pixel(10,5),image::Rgba([255,255,255,0]));
        assert_eq!(*mask.get_pixel(0,0),image::Rgba([255,255,255,255]));
        drop(prepared);
    }
    #[test]
    fn core_paid_image_edit_rejects_external_source_without_publishing_inputs(){
        i_slint_backend_testing::init_no_event_loop();let(f,_)=fixture();let external=tempfile::tempdir().unwrap();
        let path=external.path().join("source.png");image::RgbaImage::new(20,10).save(&path).unwrap();
        let original=fs::read(&path).unwrap();let p=f.persistence.clone();let worker_path=path.clone();
        assert!(std::thread::spawn(move||prepare_image_edit_inputs_for_namespace(&p,&worker_path,Vec::new())).join().unwrap().is_err());
        assert_eq!(fs::read(path).unwrap(),original);
        assert!(f.authority.enumerate_regular_names(ManagedUserArea::Input).unwrap().is_empty());
    }
    #[test]
    fn core_paid_image_edit_held_original_replacement_is_rejected_before_submission(){
        i_slint_backend_testing::init_no_event_loop();let(f,path)=fixture();let p=f.persistence.clone();let original=path.clone();
        let mut prepared=std::thread::spawn(move||prepare_image_edit_inputs_for_namespace(&p,&original,Vec::new()).unwrap()).join().unwrap();
        let old=path.with_extension("retained-old");fs::rename(&path,&old).unwrap();
        image::RgbaImage::from_pixel(20,10,image::Rgba([1,2,3,255])).save(&path).unwrap();
        assert!(prepared.original.ensure_current(&f.authority).is_err());
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
        assert!(old.is_file() && path.is_file());
        drop(prepared);
    }
}

// Additional actual E prerequisite tests: metadata is not file-read authority.
#[cfg(test)]
mod core_regeneration_input_tests {
    use super::*;
    fn item(references:Vec<String>)->AssetData {AssetData{
        id:"retained-failed-image".into(),conversation_id:"conversation".into(),title:"original title".into(),
        category:"character".into(),kind:"game".into(),time:String::new(),prompt:"original prompt".into(),
        ratio:"1:1".into(),quality:"general".into(),model:"original-model".into(),origin:"generation".into(),
        width:80,height:80,source_path:"failed".into(),reference_paths:references,
        cutout_done:false,remove_black_done:false,upscale_done:false,is_new:false,
        delivery_recoverable:false,delivery_downloading:false,
    }}
    #[test]
    fn core_regeneration_failed_output_does_not_block_original_reference_inputs(){
        i_slint_backend_testing::init_no_event_loop();
        let f=video_image_callbacks::tests::scoped_inputs::Fixture::new();let authority=f.authority.clone();
        let source=std::thread::spawn(move||persist_reference_image_for_namespace(&authority,
            &image::DynamicImage::ImageRgba8(image::RgbaImage::new(20,10))).unwrap()).join().unwrap();
        let p=f.persistence.clone();let original=item(vec![source.to_string_lossy().into_owned()]);
        let prepared=std::thread::spawn(move||prepare_asset_regeneration_for_namespace(&p,original).unwrap()).join().unwrap();
        assert_eq!(prepared.item.source_path,"failed");assert_eq!(prepared.item.model,"original-model");
        assert_eq!(prepared.references.len(),1);assert_eq!(prepared.references[0].path,source);
        drop(prepared);f.drain();
    }
    #[test]
    fn core_regeneration_text_only_failed_result_keeps_original_metadata_without_output_read(){
        i_slint_backend_testing::init_no_event_loop();
        let f=video_image_callbacks::tests::scoped_inputs::Fixture::new();let p=f.persistence.clone();
        let prepared=std::thread::spawn(move||prepare_asset_regeneration_for_namespace(&p,item(Vec::new())).unwrap()).join().unwrap();
        assert!(prepared.references.is_empty());assert_eq!(prepared.item.prompt,"original prompt");
        assert_eq!(prepared.item.quality,"general");assert_eq!(prepared.item.source_path,"failed");
        drop(prepared);f.drain();
    }
}

#[cfg(test)]
mod core_paid_viewer_caller_tests {
    use super::*;
    use std::io::{Read,Write};use std::net::TcpListener;
    const OWNER:&str="11111111-1111-4111-8111-111111111111";
    const PAYER:&str="22222222-2222-4222-8222-222222222222";
    const OTHER:&str="99999999-9999-4999-8999-999999999999";
    const SOURCE_FILE:&str="55555555-5555-4555-8555-555555555555";
    const MASK_FILE:&str="66666666-6666-4666-8666-666666666666";
    fn envelope(value:serde_json::Value)->Vec<u8>{serde_json::to_vec(&serde_json::json!({"request_id":"paid-fixture","data":value,"error":null,"meta":null})).unwrap()}
    struct Server {
        url:String,listener:Option<TcpListener>,stop:Arc<std::sync::atomic::AtomicBool>,
        requests:Arc<Mutex<Vec<(String,Vec<u8>)>>>,worker:Option<std::thread::JoinHandle<()>>,
    }
    impl Server {
        fn new()->Self{
            let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
            Self{url:format!("http://{}/",listener.local_addr().unwrap()),listener:Some(listener),
                stop:Arc::new(std::sync::atomic::AtomicBool::new(false)),requests:Arc::new(Mutex::new(Vec::new())),worker:None}
        }
        fn start(&mut self,mut reply:impl FnMut(&str,&[u8])->(u16,Vec<u8>)+Send+'static){
            let listener=self.listener.take().unwrap();let stop=self.stop.clone();let requests=self.requests.clone();
            self.worker=Some(std::thread::spawn(move||{
                let until=Instant::now()+Duration::from_secs(20);
                while !stop.load(Ordering::Acquire) && Instant::now()<until {
                    let mut stream=match listener.accept(){
                        Ok((stream,_))=>stream,
                        Err(error)if error.kind()==std::io::ErrorKind::WouldBlock=>{std::thread::sleep(Duration::from_millis(2));continue;},
                        Err(_)=>panic!("paid fixture accept failed"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut request=Vec::new();let mut block=[0u8;2048];
                    let boundary=loop{
                        let read=match stream.read(&mut block){
                            Ok(0)if request.is_empty()=>break None,Ok(n)=>n,
                            Err(_)if request.is_empty()=>break None,Err(_)=>panic!("paid fixture partial request"),
                        };
                        assert!(read>0);request.extend_from_slice(&block[..read]);assert!(request.len()<1024*1024);
                        if let Some(end)=request.windows(4).position(|window|window==b"\r\n\r\n"){
                            let headers=String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                            let size=headers.lines().find_map(|line|line.strip_prefix("content-length:"))
                                .map(|size|size.trim().parse::<usize>().unwrap()).unwrap_or(0);
                            if request.len()>=end+4+size {break Some((end,size));}
                        }
                    };
                    let Some((end,size))=boundary else{continue;};
                    let head=String::from_utf8(request[..end].to_vec()).unwrap();let body=request[end+4..end+4+size].to_vec();
                    requests.lock().unwrap().push((head.clone(),body.clone()));
                    let(status,body)=reply(&head,&body);
                    let _=write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                    let _=stream.write_all(&body);
                }
            }));
        }
        fn finish(&mut self){self.stop.store(true,Ordering::Release);if let Some(worker)=self.worker.take(){worker.join().unwrap();}}
    }
    impl Drop for Server {fn drop(&mut self){
        self.stop.store(true,Ordering::Release);
        if let Some(worker)=self.worker.take(){let result=worker.join();if !std::thread::panicking(){result.unwrap();}}
    }}
    struct Fixture(video_image_callbacks::tests::scoped_inputs::Fixture);
    impl std::ops::Deref for Fixture{type Target=video_image_callbacks::tests::scoped_inputs::Fixture;fn deref(&self)->&Self::Target{&self.0}}
    impl Drop for Fixture{fn drop(&mut self){
        *self.context.active_namespace.lock().unwrap_or_else(|error|error.into_inner())=None;
        let delivery=drain_delivery_commit_workers_for_lease_for_test(self.persistence.lease());
        let preview=drain_activation_preview_workers_for_lease_for_test(self.persistence.lease());
        let player=drain_video_player_workers_for_lease_for_test(self.persistence.lease());
        let retired=self.context.user_activity.begin_quiesce(self.persistence.lease()).map(|guard|guard.retire());
        if !std::thread::panicking(){delivery.unwrap();preview.unwrap();player.unwrap();retired.unwrap();}
    }}
    fn setup(url:&str)->(Fixture,AppWindow){
        i_slint_backend_testing::init_no_event_loop();
        let mut inner=video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let backend=Arc::new(BackendRuntime{api:ApiClient::new(ApiClientConfig{
            base_url:reqwest::Url::parse(url).unwrap(),app_version:"999.0.0".into(),timeout:Duration::from_secs(2)
        },DeviceIdentity{id:Uuid::new_v4().to_string(),name:"paid-caller-fixture".into(),platform:"macos".into()},
            inner.context.backend.as_ref().unwrap().api.session().clone()).unwrap()});
        backend.api.bind_user_work(UserWorkAdmission::new(inner.context.active_namespace.clone(),inner.context.user_activity.clone())).unwrap();
        let p=PrivatePersistence::for_test_with_storage((*inner.writer).clone(),inner.persistence.lease().clone(),
            inner.context.user_activity.clone(),backend.api.upgrade_latch().clone(),inner.context.data_root_capability.clone().unwrap(),
            backend.api.clone(),inner.context.file_index.clone().unwrap());
        inner.context.backend=Some(backend);inner.context.store.borrow_mut().private_persistence=Some(p.clone());
        inner.authority=p.storage_authority().unwrap();inner.persistence=p;
        let transition=inner.context.namespace_operations.try_begin_transition().unwrap();
        let recovery=transition.begin_prepublication_recovery(inner.persistence.lease()).unwrap();
        recovery.verify_no_unsupported_imports(&inner.authority).unwrap();
        let recovered=recovery.finish().unwrap();
        transition.prepare_publication(inner.persistence.lease(),recovered).unwrap().publish();
        let f=Fixture(inner);let app=AppWindow::new().unwrap();
        let state=app.global::<AppState>();state.set_page("assets".into());state.set_logged_in(true);state.set_session_state("online".into());
        state.set_asset_type("character".into());state.set_mode("game".into());state.set_image_model("paid-test-model".into());
        state.set_ratio("1:1".into());state.set_quality("1K".into());state.set_prompt("later unrelated UI prompt".into());
        f.context.store.borrow_mut().custom_prompts.push("real transaction trigger".into());
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();publish_group(&f,PAYER);
        (f,app)
    }
    fn publish_group(f:&Fixture,group:&str){
        let manager=&f.context.billing_context;let session=f.context.current_account_session_scope().unwrap();
        if manager.confirmed_scope().is_none(){manager.bind_authenticated_session(session.clone()).unwrap();}
        let ticket=manager.begin_switch(&session,"paid-fixture",group,PreviousBillingAuthority::StillValid).unwrap();
        let snapshot:AccountSnapshot=serde_json::from_value(serde_json::json!({
            "user":{"id":OWNER,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
            "read_only":false,"capabilities":["bill"],"membership":null,"entitlement":{},"credits":null,"quota":null,
            "billing_group":{"group_id":group,"name":"fixture","group_status":"active","role":"owner","member_id":null,"relationship_status":null,
                "readable_context":true,"selectable":true,"group_version":"1","membership_version":null,"capabilities":["bill"],"quota":null}
        })).unwrap();
        let staged=manager.stage_confirmation(&ticket,snapshot.billing_group.clone(),snapshot).unwrap();
        f.writer.save_selected_group(OWNER,"paid-fixture",group).unwrap();manager.publish_persisted(ticket,staged);
    }
    fn source(f:&Fixture)->PathBuf {
        let authority=f.authority.clone();std::thread::spawn(move||persist_reference_image_for_namespace(&authority,
            &image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(40,40,image::Rgba([17,41,89,255])))).unwrap()).join().unwrap()
    }
    fn original(path:&Path)->AssetData {AssetData {
        id:"original-asset".into(),conversation_id:OTHER.into(),title:"Original".into(),category:"character".into(),kind:"game".into(),
        time:"2026-09-08".into(),prompt:"Original paid prompt".into(),ratio:"1:1".into(),quality:"1K".into(),model:"paid-test-model".into(),
        origin:"backend".into(),width:40,height:40,source_path:path.to_string_lossy().into_owned(),reference_paths:Vec::new(),
        cutout_done:false,remove_black_done:false,upscale_done:false,is_new:false,delivery_recoverable:false,delivery_downloading:false,
    }}
    fn serve(server:&mut Server,f:&Fixture,paired:bool) {
        let authority=f.authority.clone();let writer=(*f.writer).clone();let lease=f.persistence.lease().clone();let url=server.url.clone();let mut uploads=0;
        server.start(move|headers,body|{
            let first=headers.lines().next().unwrap();let lower=headers.to_ascii_lowercase();
            if first.starts_with("POST /v1/uploads/references ") {
                assert!(!lower.contains("x-account-group-id:"));
                let rows=load_pending_generations_for_namespace(&authority).unwrap();assert_eq!(rows.len(),1,"input upload must follow original durable intent");
                assert_eq!(rows[0].billing_account_group_id,PAYER);assert_eq!(rows[0].model_code,"paid-test-model");
                let saved=writer.load_client_state_for_namespace(&lease).unwrap().unwrap();assert_eq!(saved.image_model,"paid-test-model");
                let request:serde_json::Value=serde_json::from_slice(body).unwrap();assert!(request["size_bytes"].as_u64().unwrap()>0);
                assert_eq!(request["sha256"].as_str().unwrap().len(),64);
                uploads+=1;let id=if uploads==1 {SOURCE_FILE}else{MASK_FILE};assert!(uploads<=if paired{2}else{1});
                return(200,envelope(serde_json::json!({"file":{"id":id},"upload":{"method":"POST","url":format!("{url}upload"),"fields":{},"file_field":"file"}})));
            }
            if first.starts_with("POST /upload ") {return(200,Vec::new());}
            if first.starts_with(&format!("POST /v1/uploads/references/{SOURCE_FILE}/complete ")) || first.starts_with(&format!("POST /v1/uploads/references/{MASK_FILE}/complete ")) {return(200,envelope(serde_json::json!({})));}
            if first.starts_with("GET /v1/account "){return(503,serde_json::to_vec(&serde_json::json!({"request_id":"refresh-pending","data":null,"error":{"code":"service_unavailable","message":"controlled refresh failure","details":null},"meta":null})).unwrap());}
            assert!(first.starts_with("POST /v1/generation/tasks "),"unexpected paid fixture endpoint");
            assert!(lower.contains(&format!("x-account-group-id: {PAYER}")));
            let rows=load_pending_generations_for_namespace(&authority).unwrap();assert_eq!(rows.len(),1);
            assert_eq!(rows[0].uploaded_file_ids.len(),if paired{2}else{1});
            let request:serde_json::Value=serde_json::from_slice(body).unwrap();
            assert_eq!(request["client_request_id"],rows[0].client_request_id);assert_eq!(request["model_code"],"paid-test-model");
            assert_eq!(request["prompt"],"Original paid prompt");assert_eq!(request["quality"],"1K");
            assert!(lower.contains(&format!("idempotency-key: {}",rows[0].client_request_id)));
            if paired {assert_eq!(request["task_type"],"image_edit");assert_eq!(request["source_file_id"],SOURCE_FILE);assert_eq!(request["mask_file_id"],MASK_FILE);}
            else {assert_eq!(request["task_type"],"image_generation");assert_eq!(request["reference_file_ids"],serde_json::json!([SOURCE_FILE]));}
            (503,serde_json::to_vec(&serde_json::json!({"request_id":"ambiguous-original","data":null,"error":{"code":"service_unavailable","message":"private failure","details":null},"meta":null})).unwrap())
        });
    }
    fn finished(f:&Fixture)->bool{f.context.generations.active.borrow().is_empty()}
    #[test]
    fn core_paid_image_edit_actual_entry_persists_original_pair_and_payer_before_transport(){
        let server=Server::new();let(f,app)=setup(&server.url);let path=source(&f);let item=original(&path);
        let p=f.persistence.clone();let original_path=path.clone();let prepared=std::thread::spawn(move||prepare_image_edit_inputs_for_namespace(&p,&original_path,
            vec![CapturedImageEditBrushPoint{x:0.5,y:0.5,size:0.5,shape:"square".into()}]).unwrap()).join().unwrap();
        let scope=f.context.billing_context.confirmed_scope().unwrap();let mut server=server;serve(&mut server,&f,true);
        start_backend_image_edit_with_prepared_inputs(&app,f.context.clone(),f.authority.clone(),&scope,Some(item),prepared,CapturedImageEditRequest{
            prompt:"Original paid prompt".into(),model_code:"paid-test-model".into(),quality:"1K".into(),estimated_credit_cost:1,category:"character".into(),mode:"game".into(),conversation_id:OTHER.into(),
        });
        assert!(!finished(&f),"actual registered paid work was not admitted");
        video_image_callbacks::tests::scoped_inputs::pump(||finished(&f));server.finish();
        assert_eq!(server.requests.lock().unwrap().iter().filter(|(head,_)|head.starts_with("POST /v1/generation/tasks ")).count(),1);
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].task_type,"image_edit");
        assert!(path.is_file());
    }
    #[test]
    fn core_paid_regeneration_actual_entry_preserves_failed_output_metadata_and_real_references(){
        let server=Server::new();let(f,app)=setup(&server.url);let path=source(&f);let mut item=original(Path::new("failed"));
        item.reference_paths=vec![path.to_string_lossy().into_owned()];let p=f.persistence.clone();
        let prepared=std::thread::spawn(move||prepare_asset_regeneration_for_namespace(&p,item).unwrap()).join().unwrap();
        let scope=f.context.billing_context.confirmed_scope().unwrap();let mut server=server;serve(&mut server,&f,false);
        assert!(start_asset_regeneration_with_prepared_inputs(&app,f.context.clone(),f.authority.clone(),&scope,prepared));
        video_image_callbacks::tests::scoped_inputs::pump(||finished(&f));server.finish();
        assert_eq!(server.requests.lock().unwrap().iter().filter(|(head,_)|head.starts_with("POST /v1/generation/tasks ")).count(),1);
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].task_type,"image_generation");
        assert_eq!(rows[0].generation_prompt,"Original paid prompt");assert!(path.is_file());
    }
    #[test]
    fn core_paid_input_store_rejection_prevents_intent_upload_and_billable_post(){
        let server=Server::new();let(f,app)=setup(&server.url);let path=source(&f);let mut item=original(Path::new("failed"));item.reference_paths=vec![path.to_string_lossy().into_owned()];
        let p=f.persistence.clone();let prepared=std::thread::spawn(move||prepare_asset_regeneration_for_namespace(&p,item).unwrap()).join().unwrap();
        let scope=f.context.billing_context.confirmed_scope().unwrap();f.writer.reject_custom_prompt_inserts_for_test(true);
        assert!(start_asset_regeneration_with_prepared_inputs(&app,f.context.clone(),f.authority.clone(),&scope,prepared));
        video_image_callbacks::tests::scoped_inputs::pump(||finished(&f));
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
        backend_generation::billing_capture_test_support::assert_no_request(server.listener.as_ref().unwrap());
        f.writer.reject_custom_prompt_inserts_for_test(false);assert!(path.is_file());
    }
    struct ReleasePaidCompletion(Option<mpsc::Sender<()>>);
    impl Drop for ReleasePaidCompletion {fn drop(&mut self){if let Some(release)=self.0.take(){let _=release.send(());}}}
    #[test]
    fn core_paid_sent_result_rejects_same_lease_replacement_store_before_final_consumption(){
        let mut server=Server::new();let(f,app)=setup(&server.url);let path=source(&f);
        let mut item=original(Path::new("failed"));item.reference_paths=vec![path.to_string_lossy().into_owned()];
        let p=f.persistence.clone();let prepared=std::thread::spawn(move||prepare_asset_regeneration_for_namespace(&p,item).unwrap()).join().unwrap();
        let scope=f.context.billing_context.confirmed_scope().unwrap();serve(&mut server,&f,false);
        let(sent,received)=mpsc::channel();let(release,released)=mpsc::channel();let mut guard=ReleasePaidCompletion(Some(release));
        set_delivery_preparation_after_send_for_test(move||{sent.send(()).unwrap();let _=released.recv();});
        assert!(start_asset_regeneration_with_prepared_inputs(&app,f.context.clone(),f.authority.clone(),&scope,prepared));
        let reached=Cell::new(false);
        video_image_callbacks::tests::scoped_inputs::pump(||{if received.try_recv().is_ok(){reached.set(true);}reached.get()});
        let replacement=PrivatePersistence::for_test_with_storage((*f.writer).clone(),f.persistence.lease().clone(),
            f.context.user_activity.clone(),f.context.backend.as_ref().unwrap().api.upgrade_latch().clone(),
            f.context.data_root_capability.clone().unwrap(),f.context.backend.as_ref().unwrap().api.clone(),f.context.file_index.clone().unwrap());
        f.context.store.borrow_mut().private_persistence=Some(replacement);
        app.global::<AppState>().set_generation_status("replacement Store owns status".into());
        let before=serde_json::to_value(local_store_data(&app,&f.context.store.borrow())).unwrap();
        guard.0.take().unwrap().send(()).unwrap();
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(500));
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(500));server.finish();
        assert_eq!(serde_json::to_value(local_store_data(&app,&f.context.store.borrow())).unwrap(),before);
        assert_eq!(app.global::<AppState>().get_generation_status(),"replacement Store owns status");
        assert!(f.context.store.borrow().generations.is_empty());
        assert_eq!(load_pending_generations_for_namespace(&f.authority).unwrap().len(),1);
    }
    #[test]
    fn core_paid_regeneration_actual_owned_output_waits_for_store_before_delivery_ack(){
        let mut server=Server::new();let(f,app)=setup(&server.url);let p=f.persistence.clone();
        let prepared=std::thread::spawn(move||prepare_asset_regeneration_for_namespace(&p,original(Path::new("failed"))).unwrap()).join().unwrap();
        let scope=f.context.billing_context.confirmed_scope().unwrap();
        let mut encoded=std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8,12,image::Rgba([17,41,89,255])))
            .write_to(&mut encoded,image::ImageFormat::Png).unwrap();
        let bytes=encoded.into_inner();let hash=paid_image_sha(&bytes);let download=format!("{}owned-output",server.url);
        let detail=envelope(serde_json::json!({"id":OTHER,"billing_account_group_id":PAYER,"status":"completed",
            "progress_percent":100,"success_count":1,"failure_count":0,"failure":null,"prompt":"Original paid prompt","result_prompt":null,
            "type":"image_generation","quality":"1K","requested_count":1,"items":[{"index":0,"status":"succeeded","credit_cost":"1","failure":null,
                "file":{"id":SOURCE_FILE,"status":"available","mime_type":"image/png","size_bytes":bytes.len().to_string(),"sha256":hash,
                    "width":8,"height":12,"download_url":download}}]}));
        let writer=(*f.writer).clone();let lease=f.persistence.lease().clone();let authority=f.authority.clone();
        let acked=Arc::new(std::sync::atomic::AtomicBool::new(false));let observed=acked.clone();
        server.start(move|headers,body|{
            let first=headers.lines().next().unwrap();
            if first.starts_with("POST /v1/generation/tasks ") {
                let rows=load_pending_generations_for_namespace(&authority).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].billing_account_group_id,PAYER);
                assert!(headers.to_ascii_lowercase().contains(&format!("x-account-group-id: {PAYER}")));
                return(200,detail.clone());
            }
            if first.starts_with(&format!("GET /v1/generation/tasks/{OTHER} ")){return(200,detail.clone());}
            if first.starts_with("GET /owned-output "){return(200,bytes.clone());}
            if first.starts_with(&format!("POST /v1/generation/tasks/{OTHER}/deliveries/{SOURCE_FILE}/ack ")) {
                let data=writer.load_client_state_for_namespace(&lease).unwrap().unwrap();assert_eq!(data.assets.len(),1);assert_eq!(data.generations.len(),1);
                assert_eq!(data.assets[0].id,SOURCE_FILE);assert_eq!(std::fs::read(&data.assets[0].source_path).unwrap(),bytes);
                let body:serde_json::Value=serde_json::from_slice(body).unwrap();assert_eq!(body["sha256"],hash);assert_eq!(body["size_bytes"],bytes.len());
                observed.store(true,Ordering::Release);return(200,envelope(serde_json::json!({})));
            }
            assert!(first.starts_with("GET /v1/account "));
            (503,serde_json::to_vec(&serde_json::json!({"request_id":"refresh-pending","data":null,
                "error":{"code":"service_unavailable","message":"controlled refresh failure","details":null},"meta":null})).unwrap())
        });
        assert!(start_asset_regeneration_with_prepared_inputs(&app,f.context.clone(),f.authority.clone(),&scope,prepared));
        video_image_callbacks::tests::scoped_inputs::pump(||finished(&f));
        assert!(acked.load(Ordering::Acquire));
        let saved=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(saved.assets.len(),1);assert_eq!(saved.generations.len(),1);assert_eq!(saved.assets[0].id,SOURCE_FILE);
        assert!(Path::new(&saved.assets[0].source_path).starts_with(f.persistence.lease().namespace.path(ManagedUserArea::Output)));
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());server.finish();
    }
    fn running_detail()->Vec<u8>{envelope(serde_json::json!({
        "id":OTHER,"billing_account_group_id":PAYER,"status":"running","progress_percent":5,
        "success_count":0,"failure_count":0,"failure":null,"prompt":"Original paid prompt","result_prompt":null,
        "type":"image_generation","quality":"1K","requested_count":1,"items":[]
    }))}
    #[test]
    fn core_paid_original_binding_replacement_rejects_entry_before_intent_or_transport(){
        let server=Server::new();let(f,app)=setup(&server.url);let p=f.persistence.clone();
        let prepared=std::thread::spawn(move||prepare_asset_regeneration_for_namespace(&p,original(Path::new("failed"))).unwrap()).join().unwrap();
        let scope=f.context.billing_context.confirmed_scope().unwrap();
        let replacement=PrivatePersistence::for_test_with_storage((*f.writer).clone(),f.persistence.lease().clone(),
            f.context.user_activity.clone(),f.context.backend.as_ref().unwrap().api.upgrade_latch().clone(),
            f.context.data_root_capability.clone().unwrap(),f.context.backend.as_ref().unwrap().api.clone(),f.context.file_index.clone().unwrap());
        f.context.store.borrow_mut().private_persistence=Some(replacement);
        let before=serde_json::to_value(local_store_data(&app,&f.context.store.borrow())).unwrap();
        assert!(!start_asset_regeneration_with_prepared_inputs(&app,f.context.clone(),f.authority.clone(),&scope,prepared));
        assert_eq!(serde_json::to_value(local_store_data(&app,&f.context.store.borrow())).unwrap(),before);
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());assert!(finished(&f));
        backend_generation::billing_capture_test_support::assert_no_request(server.listener.as_ref().unwrap());
    }
    #[test]
    fn core_paid_actual_stop_has_one_registered_remote_owner_and_terminal_scope_dispatch(){
        let mut server=Server::new();let(f,app)=setup(&server.url);let p=f.persistence.clone();
        let prepared=std::thread::spawn(move||prepare_asset_regeneration_for_namespace(&p,original(Path::new("failed"))).unwrap()).join().unwrap();
        let scope=f.context.billing_context.confirmed_scope().unwrap();
        let authority=f.authority.clone();let marked=Arc::new(std::sync::atomic::AtomicBool::new(false));let observed=marked.clone();
        server.start(move|headers,_|{
            let first=headers.lines().next().unwrap();
            if first.starts_with(&format!("POST /v1/generation/tasks/{OTHER}/cancel ")) {
                let rows=load_pending_generations_for_namespace(&authority).unwrap();
                assert_eq!(rows.len(),1);assert!(rows[0].cancel_requested);assert_eq!(rows[0].billing_account_group_id,PAYER);
                observed.store(true,Ordering::Release);
                return(401,serde_json::to_vec(&serde_json::json!({"request_id":"actual-stop-terminal","data":null,
                    "error":{"code":"session_invalid","message":"private fixture detail","details":null},"meta":null})).unwrap());
            }
            assert!(first.starts_with("POST /v1/generation/tasks ")||first.starts_with(&format!("GET /v1/generation/tasks/{OTHER} ")));
            (200,running_detail())
        });
        wire_generation_callbacks(&app,f.context.clone());
        assert!(start_asset_regeneration_with_prepared_inputs(&app,f.context.clone(),f.authority.clone(),&scope,prepared));
        video_image_callbacks::tests::scoped_inputs::pump(||f.context.generations.active.borrow().values()
            .any(|task|task.server_task_id.as_deref()==Some(OTHER)));
        app.global::<AppState>().invoke_stop_generation();app.global::<AppState>().invoke_stop_generation();
        video_image_callbacks::tests::scoped_inputs::pump(||app.global::<AppState>().get_session_state()=="signed_out");
        let delivery=drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease());
        let previews=drain_activation_preview_workers_for_lease_for_test(f.persistence.lease());
        delivery.unwrap();previews.unwrap();server.finish();
        assert!(marked.load(Ordering::Acquire));assert!(f.context.current_user_id.lock().unwrap().is_none());
        let requests=server.requests.lock().unwrap();
        assert_eq!(requests.iter().filter(|(head,_)|head.starts_with(&format!("POST /v1/generation/tasks/{OTHER}/cancel "))).count(),1);
        assert_eq!(requests.iter().filter(|(head,_)|head.starts_with("POST /v1/generation/tasks ")).count(),1);
        let bytes=std::fs::read(f.persistence.lease().namespace.path(ManagedUserArea::Recovery).join("pending-generations.json")).unwrap();
        let document:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
        let rows:Vec<PendingGenerationRecord>=serde_json::from_value(document["generations"].clone()).unwrap();
        assert_eq!(rows.len(),1);assert!(rows[0].cancel_requested);assert_eq!(rows[0].server_task_id,OTHER);
    }
    #[test]
    fn core_paid_shutdown_after_actual_stop_preserves_durable_intent_without_remote_cancel(){
        let mut server=Server::new();let(f,app)=setup(&server.url);let p=f.persistence.clone();
        let prepared=std::thread::spawn(move||prepare_asset_regeneration_for_namespace(&p,original(Path::new("failed"))).unwrap()).join().unwrap();
        let scope=f.context.billing_context.confirmed_scope().unwrap();
        let entered=Arc::new(std::sync::atomic::AtomicBool::new(false));let seen=entered.clone();let(release,wait)=mpsc::channel();
        struct Release(Option<mpsc::Sender<()>>);impl Drop for Release{fn drop(&mut self){if let Some(release)=self.0.take(){let _=release.send(());}}}
        let mut release=Release(Some(release));let mut held=false;
        server.start(move|headers,_|{
            let first=headers.lines().next().unwrap();
            if first.starts_with(&format!("GET /v1/generation/tasks/{OTHER} ")) && !held {
                held=true;seen.store(true,Ordering::Release);wait.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            assert!(!first.contains("/cancel"));(200,running_detail())
        });
        wire_generation_callbacks(&app,f.context.clone());
        assert!(start_asset_regeneration_with_prepared_inputs(&app,f.context.clone(),f.authority.clone(),&scope,prepared));
        video_image_callbacks::tests::scoped_inputs::pump(||entered.load(Ordering::Acquire));
        app.global::<AppState>().invoke_stop_generation();
        video_image_callbacks::tests::scoped_inputs::pump(||load_pending_generations_for_namespace(&f.authority).unwrap()
            .iter().any(|row|row.cancel_requested));
        cancel_delivery_commit_workers(f.persistence.lease());
        release.0.take().unwrap().send(()).unwrap();
        // No later UI timer is needed to stop and join the original HTTP worker.
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();server.finish();
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();
        assert_eq!(rows.len(),1);assert!(rows[0].cancel_requested);assert_eq!(rows[0].server_task_id,OTHER);
        assert!(!server.requests.lock().unwrap().iter().any(|(head,_)|head.contains("/cancel")));
    }

    #[test]
    fn core_paid_shared_cancel_joins_running_task_without_another_ui_timer(){
        let mut server=Server::new();let(f,app)=setup(&server.url);let p=f.persistence.clone();
        let prepared=std::thread::spawn(move||prepare_asset_regeneration_for_namespace(&p,original(Path::new("failed"))).unwrap()).join().unwrap();
        let scope=f.context.billing_context.confirmed_scope().unwrap();
        server.start(|headers,_|{
            let first=headers.lines().next().unwrap();
            assert!(first.starts_with("POST /v1/generation/tasks ")||first.starts_with(&format!("GET /v1/generation/tasks/{OTHER} ")));
            (200,running_detail())
        });
        assert!(start_asset_regeneration_with_prepared_inputs(&app,f.context.clone(),f.authority.clone(),&scope,prepared));
        video_image_callbacks::tests::scoped_inputs::pump(||server.requests.lock().unwrap().iter().any(|(head,_)|head.starts_with("POST /v1/generation/tasks ")));
        let before=Instant::now();
        // This performs actual cancel+alljoin without pumping any UI timer.
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();
        assert!(before.elapsed()<Duration::from_secs(5));server.finish();
        let rows=load_pending_generations_for_namespace(&f.authority).unwrap();assert_eq!(rows.len(),1);
        assert_eq!(rows[0].billing_account_group_id,PAYER);assert_eq!(rows[0].server_task_id,OTHER);
        assert!(!rows[0].cancel_requested,"local shutdown must not invent explicit Stop");
        assert!(f.context.cancelled_generation_requests.lock().unwrap().is_empty());
        assert_eq!(server.requests.lock().unwrap().iter().filter(|(head,_)|head.starts_with("POST /v1/generation/tasks ")).count(),1);
        assert!(!server.requests.lock().unwrap().iter().any(|(head,_)|head.contains("/cancel")));
    }
    #[test]
    fn core_paid_cancel_terminal_http_preserves_typed_original_scope_and_tombstone(){
        let mut server=Server::new();let(f,_app)=setup(&server.url);let scope=f.context.billing_context.confirmed_scope().unwrap();
        let mut record=paid_viewer_record(&scope,"Original paid prompt".into(),"paid-test-model".into(),"1K".into(),
            "character".into(),"game".into(),"1:1".into(),OTHER.into(),"image_generation",0,0,&[],Vec::new(),String::new());
        record.server_task_id=OTHER.into();upsert_pending_generation_for_namespace(&f.authority,&scope,record.clone()).unwrap();
        server.start(|headers,_|{
            let first=headers.lines().next().unwrap();
            if first.starts_with(&format!("GET /v1/generation/tasks/{OTHER} ")){return(200,running_detail());}
            assert!(first.starts_with(&format!("POST /v1/generation/tasks/{OTHER}/cancel ")));
            (401,serde_json::to_vec(&serde_json::json!({"request_id":"cancel-session-ended","data":null,
                "error":{"code":"session_invalid","message":"private session failure","details":null},"meta":null})).unwrap())
        });
        let backend=f.context.backend.as_ref().unwrap().clone();let authority=f.authority.clone();let session=scope.request.session.clone();
        let request_id=record.client_request_id.clone();let cancellations=f.context.cancelled_generation_requests.clone();
        let result=std::thread::spawn(move||{
            let api=GenerationApi::new(backend.api.clone()).with_saved_group(PAYER);
            cleanup_cancelled_generation_checked(&backend,&authority,&api,&session,&request_id,&[],Some(OTHER),&cancellations)
        }).join().unwrap();server.finish();
        assert!(matches!(result,Err(DeliveryRetryError::Api(ref error))if error.is_terminal_session_error()));
        assert!(terminal_auth_scope_matches_context(&f.context,&scope.request.session));
        *f.context.current_user_id.lock().unwrap()=Some(OTHER.into());
        assert!(!terminal_auth_scope_matches_context(&f.context,&scope.request.session));
        // No fake success or removal of uncertain cancellation state.
        // Test-only read of this fixture's exact retained file after API retirement;
        // this does not give production code a way to adopt a retired namespace.
        let bytes=std::fs::read(f.persistence.lease().namespace.path(ManagedUserArea::Recovery).join("pending-generations.json")).unwrap();
        let document:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
        let rows:Vec<PendingGenerationRecord>=serde_json::from_value(document["generations"].clone()).unwrap();
        assert_eq!(rows.len(),1);assert_eq!(rows[0].identity(),record.identity());assert!(rows[0].cancel_requested);
        assert_eq!(server.requests.lock().unwrap().len(),2);
    }

}



pub(super) fn generation_download_staging_path(
    client_request_id: &str,
    item_index: usize,
    file: &TaskOutputFile,
) -> PathBuf {
    let extension = match file.mime_type.as_str() {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    };
    app_data_dir().join("delivery-staging").join(format!(
        "{}-{}-{}.{}",
        sanitize_filename(client_request_id),
        item_index,
        sanitize_filename(&file.id),
        extension
    ))
}

fn delivery_confirmation_for_item(
    client_request_id: &str,
    detail: &GenerationTaskDetail,
    item_index: usize,
) -> Option<DeliveryConfirmation> {
    let item = detail.items.iter().find(|item| item.index == item_index)?;
    if item.status != "succeeded" {
        return None;
    }
    let file = item.file.as_ref()?;
    Some(DeliveryConfirmation {
        client_request_id: client_request_id.to_string(),
        item_index,
        task_id: detail.id.clone(),
        file_id: file.id.clone(),
        sha256: file.sha256.clone(),
        size_bytes: file.size_bytes.parse().unwrap_or(0),
        failed_asset_id: None,
    })
}

fn failed_delivery_confirmation_for_item(
    session_scope: &SessionScope,
    client_request_id: &str,
    detail: &GenerationTaskDetail,
    item_index: usize,
    existing_failed_asset_id: Option<&str>,
) -> Result<DeliveryConfirmation> {
    let mut delivery = delivery_confirmation_for_item(client_request_id, detail, item_index)
        .ok_or_else(|| anyhow!("succeeded generation item is missing delivery metadata"))?;
    let failed_asset_id = existing_failed_asset_id
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if !matches!(
        pending_delivery_failed(
            &session_scope.owner_user_id,
            session_scope.auth_epoch,
            client_request_id,
            &delivery,
            &failed_asset_id,
        ),
        Ok(true)
    ) {
        return Err(anyhow!("pending generation delivery cannot be marked recoverable"));
    }
    delivery.failed_asset_id = Some(failed_asset_id);
    Ok(delivery)
}

fn failed_asset_id_for_delivery(record: &PendingGenerationRecord, file_id: &str) -> Option<String> {
    record
        .deliveries
        .iter()
        .find(|delivery| delivery.file_id == file_id)
        .map(|delivery| delivery.failed_asset_id.clone())
        .filter(|value| !value.trim().is_empty())
}

use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

fn report_unhandled_terminal_failures(
    sender: &mpsc::Sender<GenerationOutcome>,
    detail: &GenerationTaskDetail,
    expected_count: usize,
    handled_success: &BTreeSet<usize>,
    handled_failure: &mut BTreeSet<usize>,
    fallback: &str,
) {
    if !detail.terminal()
        || (detail.failure.is_none() && !detail.status.eq_ignore_ascii_case("failed"))
    {
        return;
    }
    let reason = detail
        .failure
        .as_ref()
        .map(TaskFailure::generation_message)
        .unwrap_or_else(|| fallback.to_string());
    let reported = handled_success.len() + handled_failure.len();
    let missing = expected_count.saturating_sub(reported);
    let time = Local::now().format("%Y-%m-%d %H:%M").to_string();
    for synthetic_index in 0..missing {
        handled_failure.insert(usize::MAX.saturating_sub(synthetic_index));
        let _ = sender.send(GenerationOutcome::ImageFailure {
            reason: reason.clone(),
            time: time.clone(),
            delivery: None,
        });
    }
}

pub(super) fn reference_fingerprints_for_namespace(authority: &NamespaceStorageAuthority, paths: &[PathBuf]) -> Result<(Vec<String>, Vec<u64>)> {
    let mut hashes = Vec::with_capacity(paths.len()); let mut sizes = Vec::with_capacity(paths.len());
    for path in paths {
        anyhow::ensure!(authority.lease().namespace.owns_path(path), "reference is outside captured namespace");
        let bytes = authority.read_image_source(path, 100 * 1024 * 1024)?;
        hashes.push(format!("{:x}", Sha256::digest(&bytes))); sizes.push(bytes.len() as u64);
    }
    Ok((hashes, sizes))
}
pub(super) fn generation_references_match_for_namespace(authority: &NamespaceStorageAuthority, record: &PendingGenerationRecord) -> bool {
    let paths = record.reference_paths.iter().map(PathBuf::from).collect::<Vec<_>>();
    reference_fingerprints_for_namespace(authority, &paths)
        .is_ok_and(|(hashes, sizes)| hashes == record.reference_sha256 && sizes == record.reference_size_bytes)
}
pub(super) fn reference_fingerprints(paths: &[PathBuf]) -> Result<(Vec<String>, Vec<u64>)> {
    let mut sha256 = Vec::with_capacity(paths.len());
    let mut sizes = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = fs::read(path).with_context(|| format!("无法读取参考图 {}", path.display()))?;
        sha256.push(format!("{:x}", Sha256::digest(&bytes)));
        sizes.push(bytes.len() as u64);
    }
    Ok((sha256, sizes))
}

pub(super) fn generation_references_match(record: &PendingGenerationRecord) -> bool {
    if record.reference_paths.is_empty() {
        return true;
    }
    if record.reference_paths.len() != record.reference_sha256.len()
        || record.reference_paths.len() != record.reference_size_bytes.len()
    {
        return false;
    }
    record
        .reference_paths
        .iter()
        .zip(&record.reference_sha256)
        .zip(&record.reference_size_bytes)
        .all(|((path, expected_sha256), expected_size)| {
            let Ok(bytes) = fs::read(path) else {
                return false;
            };
            bytes.len() as u64 == *expected_size
                && format!("{:x}", Sha256::digest(&bytes)).eq_ignore_ascii_case(expected_sha256)
        })
}

pub(super) fn recovered_delivery_path_matches(
    path: &str,
    expected_sha256: &str,
    expected_size_bytes: u64,
) -> bool {
    if path.trim().is_empty() || expected_sha256.trim().is_empty() {
        return false;
    }
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    bytes.len() as u64 == expected_size_bytes
        && format!("{:x}", Sha256::digest(&bytes)).eq_ignore_ascii_case(expected_sha256)
        && image::load_from_memory(&bytes).is_ok()
}

pub(super) fn recovered_delivery_file_matches(delivery: &PendingDeliveryRecord) -> bool {
    recovered_delivery_path_matches(&delivery.local_path, &delivery.sha256, delivery.size_bytes)
}

fn recovered_delivery_ready_for_ack(
    delivery: &PendingDeliveryRecord,
    verified_file_ids: &BTreeSet<String>,
) -> bool {
    !delivery.acknowledged && verified_file_ids.contains(&delivery.file_id)
}

fn sanitize_recovered_delivery_paths_with<F>(
    record: &mut PendingGenerationRecord,
    persist_invalid_file_ids: F,
) -> Result<BTreeSet<String>>
where
    F: FnOnce(&BTreeSet<String>) -> Result<bool>,
{
    let mut verified_file_ids = BTreeSet::new();
    let mut invalid_file_ids = BTreeSet::new();
    for delivery in &record.deliveries {
        if delivery.local_path.trim().is_empty() {
            continue;
        }
        if recovered_delivery_file_matches(delivery) {
            verified_file_ids.insert(delivery.file_id.clone());
        } else if !delivery.acknowledged {
            invalid_file_ids.insert(delivery.file_id.clone());
        }
    }
    if invalid_file_ids.is_empty() {
        return Ok(verified_file_ids);
    }
    if !persist_invalid_file_ids(&invalid_file_ids)? {
        return Err(anyhow!(
            "pending generation delivery is missing or belongs to another session scope"
        ));
    }
    for delivery in &mut record.deliveries {
        if invalid_file_ids.contains(&delivery.file_id) {
            delivery.local_path.clear();
        }
    }
    Ok(verified_file_ids)
}

pub(super) fn sanitize_recovered_delivery_paths(
    record: &mut PendingGenerationRecord,
) -> Result<BTreeSet<String>> {
    let owner_user_id = record.owner_user_id.clone();
    let auth_epoch = record.auth_epoch;
    let client_request_id = record.client_request_id.clone();
    sanitize_recovered_delivery_paths_with(record, |invalid_file_ids| {
        let mut cleared_file_ids = BTreeSet::new();
        let record_matched = update_pending_generation_scoped(
            &owner_user_id,
            auth_epoch,
            &client_request_id,
            |stored| {
                for delivery in &mut stored.deliveries {
                    if invalid_file_ids.contains(&delivery.file_id) {
                        delivery.local_path.clear();
                        cleared_file_ids.insert(delivery.file_id.clone());
                    }
                }
            },
        )?;
        Ok(record_matched && &cleared_file_ids == invalid_file_ids)
    })
}

pub(super) fn clear_recovered_delivery_local_path(
    session_scope: &SessionScope,
    client_request_id: &str,
    file_id: &str,
) -> Result<bool> {
    let mut cleared = false;
    let record_matched = update_pending_generation_scoped(
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
        client_request_id,
        |record| {
            if let Some(delivery) = record
                .deliveries
                .iter_mut()
                .find(|delivery| delivery.file_id == file_id)
            {
                delivery.local_path.clear();
                cleared = true;
            }
        },
    )?;
    Ok(record_matched && cleared)
}

pub(super) fn backend_generation_scope_active(
    backend: &BackendRuntime,
    session_scope: &SessionScope,
) -> bool {
    backend.api.user_work_is_current(session_scope)
}

#[derive(Clone)]
struct UpscaleSource {
    title: String,
    category: String,
    kind: String,
    prompt: String,
    conversation_id: String,
    source_path: String,
    reference_paths: Vec<String>,
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetryGenerationRecoveryCommitError {
    NewRecovery,
    OldDelivery,
    NewRecoveryRollback,
}

fn commit_retry_generation_recovery_with(
    retry_failed_id: Option<&str>,
    recoverable_delivery_id: Option<&str>,
    persist_new_recovery: impl FnOnce() -> Result<()>,
    abandon_old_delivery: impl FnOnce(&str) -> Result<bool>,
    rollback_new_recovery: impl FnOnce() -> Result<bool>,
    remove_retry_card: impl FnOnce(&str),
) -> std::result::Result<(), RetryGenerationRecoveryCommitError> {
    if persist_new_recovery().is_err() {
        return Err(RetryGenerationRecoveryCommitError::NewRecovery);
    }
    if let Some(failed_asset_id) = recoverable_delivery_id {
        if !matches!(abandon_old_delivery(failed_asset_id), Ok(true)) {
            return match rollback_new_recovery() {
                Ok(true) => Err(RetryGenerationRecoveryCommitError::OldDelivery),
                Ok(false) | Err(_) => Err(RetryGenerationRecoveryCommitError::NewRecoveryRollback),
            };
        }
    }
    if let Some(retry_failed_id) = retry_failed_id {
        remove_retry_card(retry_failed_id);
    }
    Ok(())
}

pub(super) fn start_backend_generation(
    app: &AppWindow,
    context: AppContext,
    raw_prompt: String,
    create_conversation: bool,
    retry_failed_id: Option<String>,
    forced_count: Option<i32>,
    existing_generation_policy: ExistingGenerationPolicy,
    destination: GenerationDestination,
) {
    let (scope, authority, _activity) = match context.capture_billing_action(KnownCapability::Bill) {
        Ok(captured) => captured,
        Err(error) => { app.global::<AppState>().set_generation_status(error.user_message().into()); return; }
    };
    start_backend_generation_with_billing_scope(app, context, authority, &scope, raw_prompt, create_conversation, retry_failed_id, forced_count, existing_generation_policy, destination);
}

pub(super) fn start_backend_generation_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: &BillingScope,
    raw_prompt: String,
    create_conversation: bool,
    retry_failed_id: Option<String>,
    forced_count: Option<i32>,
    existing_generation_policy: ExistingGenerationPolicy,
    destination: GenerationDestination,
) {
    let Some(original_persistence)=context.store.borrow().private_persistence.clone()else{return;};
    if original_persistence.lease()!=authority.lease() || !original_persistence.is_current(){return;}
    let billing_scope = match capture_billing_scope_for_submission(
        context.backend.as_deref(),
        &authority,
        billing_scope,
    ) {
        Ok(scope) => scope,
        Err(error) => {
            app.global::<AppState>()
                .set_generation_status(error.user_message().into());
            return;
        }
    };
    let session_scope = billing_scope.request.session.clone();

    let Some(backend) = context.backend.clone() else {
        return;
    };
    let store = context.store.clone();
    let state = app.global::<AppState>();
    let side_scroll_map_canvas = matches!(&destination, GenerationDestination::Canvas { .. })
        && state.get_canvas_workflow_id().as_str() == "side-scroll-map";
    let character_multi_direction_canvas =
        matches!(&destination, GenerationDestination::Canvas { .. })
            && state.get_canvas_workflow_id().as_str() == "character-multi-direction";
    let scene_composition_canvas = matches!(&destination, GenerationDestination::Canvas { .. })
        && state.get_canvas_workflow_id().as_str() == "scene-composition";
    let skill_icon_canvas = matches!(&destination, GenerationDestination::Canvas { .. })
        && state.get_canvas_workflow_id().as_str() == "skill-icon-generator";
    let single_canvas_workflow = side_scroll_map_canvas
        || character_multi_direction_canvas
        || scene_composition_canvas
        || skill_icon_canvas;
    let model_code = state.get_image_model().to_string();
    if model_code.trim().is_empty() {
        state.set_generation_status("服务端没有可用的图像模型".into());
        return;
    }
    let category = resolve_category(&state.get_asset_type().to_string(), &raw_prompt);
    if category_is_generating(&context, &category) {
        match existing_generation_policy {
            ExistingGenerationPolicy::StopExisting => stop_generation(app, &context),
            ExistingGenerationPolicy::KeepExisting => {
                set_generation_status_for_category(
                    &context,
                    app,
                    &category,
                    "当前分类已有生成任务，已保留正在进行中的任务",
                );
                sync_generation_state_for_current_category(&context, app);
                push_generations(app, &store.borrow());
                if destination == GenerationDestination::Gallery {
                    navigate_to_with_store(app, &store.borrow(), "generation");
                }
            }
        }
        return;
    }
    let ratio = resolve_ratio_for_category(
        &category,
        &state.get_ratio().to_string(),
        &raw_prompt,
        &state.get_quote_ratio().to_string(),
    );
    let ratio = if side_scroll_map_canvas {
        normalize_side_scroll_map_ratio(&ratio)
    } else {
        ratio
    };
    let quality = state.get_quality().to_string();
    let count = if single_canvas_workflow {
        1
    } else {
        forced_count.unwrap_or_else(|| state.get_count().clamp(1, 4))
    };
    let mode = if single_canvas_workflow {
        "game".to_string()
    } else {
        state.get_mode().to_string()
    };
    let original_references = {
        let store = store.borrow();
        let references = match &destination {
            GenerationDestination::Canvas { .. } => &store.canvas_references,
            GenerationDestination::Gallery => references_for_category(&store.references, &category),
        };
        references
            .iter()
            .take(max_reference_images_for_category(&category))
            .cloned()
            .collect::<Vec<_>>()
    };
    let reference_paths = original_references
        .iter()
        .map(|item| PathBuf::from(&item.source_path))
        .collect::<Vec<_>>();
    let generation_reference_paths = reference_paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let (reference_sha256, reference_size_bytes) = match reference_fingerprints_for_namespace(&authority, &reference_paths) {
        Ok(fingerprints) => fingerprints,
        Err(error) => {
            state.set_generation_status(format!("参考图校验失败：{error}").into());
            return;
        }
    };
    let quote = QuoteContext {
        title: state.get_quote_title().to_string(),
        prompt: state.get_quote_prompt().to_string(),
        ratio: state.get_quote_ratio().to_string(),
        quality: state.get_quote_quality().to_string(),
        width: state.get_quote_width(),
        height: state.get_quote_height(),
    };
    let controls = PromptControls {
        category: category.clone(),
        creation: normalize_creation_mode_for_category(
            &category,
            &state.get_creation_mode().to_string(),
        ),
        style: state.get_style_mode().to_string(),
        view: state.get_view_mode().to_string(),
        weather: state.get_weather_mode().to_string(),
        time: state.get_time_mode().to_string(),
        light: state.get_light_mode().to_string(),
    };
    let deep_english = state
        .get_deep_optimization_applied_english()
        .trim()
        .to_string();
    let deep_chinese = state
        .get_deep_optimization_applied_chinese()
        .trim()
        .to_string();
    let uses_deep_english = !deep_english.is_empty()
        && (raw_prompt.trim() == deep_english || raw_prompt.trim().ends_with(&deep_english));
    let display_prompt = if uses_deep_english && !deep_chinese.is_empty() {
        let prefix = raw_prompt
            .trim()
            .strip_suffix(&deep_english)
            .unwrap_or_default();
        format!("{prefix}{deep_chinese}")
    } else {
        raw_prompt.clone()
    };
    let language = if single_canvas_workflow {
        if state.get_language().as_str() == "en" {
            PromptLanguage::English
        } else {
            PromptLanguage::Chinese
        }
    } else if uses_deep_english || state.get_translate_prompt() || state.get_language().as_str() == "en" {
        PromptLanguage::English
    } else {
        PromptLanguage::Chinese
    };
    let generation_prompt = if side_scroll_map_canvas {
        build_side_scroll_map_generation_prompt(&raw_prompt, &ratio, &quality, language)
    } else {
        build_generation_prompt_for_destination(
            &raw_prompt,
            &state.get_negative_prompt().to_string(),
            &controls,
            &quote,
            &category,
            &ratio,
            &quality,
            language,
            &destination,
        )
    };
    let recoverable_delivery_id = retry_failed_id.as_deref().filter(|failed_asset_id| {
        store.borrow().generations.iter().any(|item| {
            item.id == *failed_asset_id
                && item.source_path == "failed"
                && item.delivery_recoverable
        })
    });

    let conversation_id = if create_conversation || matches!(&destination, GenerationDestination::Canvas { .. }) {
        Uuid::new_v4().to_string()
    } else {
        let current = state.get_current_conversation_id().to_string();
        if current.trim().is_empty() {
            Uuid::new_v4().to_string()
        } else {
            current
        }
    };
    let local_task_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().simple().to_string();
    let recovery_record = PendingGenerationRecord {
        source_asset_id: String::new(),        video_request: None,
        schema_version: 2,
            cancel_requested: false,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        owner_user_id: session_scope.owner_user_id.clone(),
        billing_account_group_id: billing_scope.request.account_group_id.clone(),
        auth_epoch: session_scope.auth_epoch,
        local_task_id: local_task_id.clone(),
        server_task_id: String::new(),
        raw_prompt: display_prompt.clone(),
        generation_prompt: generation_prompt.clone(),
        task_type: "image_generation".to_string(),
        category: category.clone(),
        mode: mode.clone(),
        ratio: ratio.clone(),
        quality: quality.clone(),
        model_code: model_code.clone(),
        conversation_id: conversation_id.clone(),
        count,
        target_width: 0,
        target_height: 0,
        create_conversation,
        reference_paths: generation_reference_paths.clone(),
        reference_sha256,
        reference_size_bytes,
        lineage_reference_paths: generation_reference_paths.clone(),
        uploaded_file_ids: vec![],
        deliveries: vec![],
        terminal: false,
        expected_success_count: 0,
        canvas_source_node_id: match &destination {
            GenerationDestination::Canvas { source_node_id } => source_node_id.clone(),
            GenerationDestination::Gallery => String::new(),
        },
        canvas_ui_extraction: false,
    };
    let recovery_identity = recovery_record.identity();
    let recovery_commit = commit_retry_generation_recovery_with(
        retry_failed_id.as_deref(),
        recoverable_delivery_id,
        || {
            upsert_pending_generation_for_namespace(
                &authority,
                &billing_scope,
                recovery_record.clone(),
            )
        },
        |failed_asset_id| {
            recoverable_delivery_for_failed_asset_for_namespace(&authority, failed_asset_id)
                .and_then(|candidate| match candidate {
                    Some((old, _)) => abandon_pending_delivery_for_namespace(
                        &authority,
                        &old.identity(),
                        failed_asset_id,
                    ),
                    None => Ok(false),
                })
        },
        || remove_pending_generation_for_namespace(&authority, &recovery_identity),
        |retry_failed_id| {
            let mut store = store.borrow_mut();
            store.generations.retain(|item| item.id != retry_failed_id);
            save_local_store(app, &store);
            push_all(app, &store);
        },
    );
    if let Err(error) = recovery_commit {
        state.set_generation_status(
            match error {
                RetryGenerationRecoveryCommitError::NewRecovery => "任务准备失败，请重试",
                RetryGenerationRecoveryCommitError::OldDelivery => {
                    "本地生成恢复记录无法更新，请重启后重试"
                }
                RetryGenerationRecoveryCommitError::NewRecoveryRollback => {
                    "本地生成恢复记录无法回滚，请重启后检查任务状态"
                }
            }
            .into(),
        );
        return;
    }
    insert_active_generation(
        &context,
        ActiveGeneration {
            task_id: local_task_id.clone(),
            client_request_id: Some(request_id.clone()),
            server_task_id: None,
            category: category.clone(),
            conversation_id: conversation_id.clone(),
            prompt: display_prompt.clone(),
            credit_cost: 0,
            total_count: count,
            loading_count: count,
            completed_count: 0,
            success_count: 0,
            failed_count: 0,
            last_failure_reason: None,
            progress: 1,
            eta: 0,
            latest_success_id: None,
            session_scope: session_scope.clone(),
            destination: destination.clone(),
            delivery_download_reservations: Vec::new(),
            registered_cancel_owner:None,
        },
    );
    set_generation_status_for_category(&context, app, &category, "正在优化并上传参考图...");
    sync_generation_state_for_current_category(&context, app);
    if destination == GenerationDestination::Gallery {
        navigate_to_with_store(app, &context.store.borrow(), "generation");
    }

    if destination == GenerationDestination::Gallery {
        state.set_quote_title("".into());
        state.set_quote_prompt("".into());
        state.set_quote_ratio("".into());
        state.set_quote_quality("".into());
    }
    if create_conversation {
        let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
        conversations.insert(
            0,
            ConversationItem {
                id: conversation_id.clone().into(),
                title: short_text(&display_prompt, 10).into(),
                image: Image::default(),
                loading: true,
            },
        );
        state.set_conversations(ModelRc::new(VecModel::from(conversations)));
        state.set_current_conversation_id(conversation_id.clone().into());
    }

    let quality_for_worker = quality.clone();
    let aspect_ratio = api_aspect_ratio(&ratio);
    let display_prompt_for_worker = display_prompt.clone();
    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let cancellations = context.cancelled_generation_requests.clone();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        let Ok(_activity) = backend.api.begin_user_work(&worker_scope) else { return; };
        let api = GenerationApi::new(backend.api.clone());
        if !backend_generation_scope_active(&backend, &worker_scope) {
            return;
        }
        if !generation_references_match_for_namespace(&authority, &recovery_record) {
            let _ = sender.send(GenerationOutcome::Failure {
                reason: "参考图内容已变化，任务已暂停，请重新发起".to_string(),
                time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
            });
            return;
        }
        let mut uploaded = Vec::new();
        for path in reference_paths {
            match api.upload_reference_for_namespace(&path, &authority, &worker_scope, false) {
                Ok(file_id) => uploaded.push(file_id),
                Err(error) => {
                    if !backend_generation_scope_active(&backend, &worker_scope) {
                        return;
                    }
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &recovery_identity);
                    let _ = sender.send(GenerationOutcome::Failure {
                        reason: error.generation_message(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    });
                    return;
                }
            }
            let uploaded_snapshot = uploaded.clone();
            if !matches!(
                apply_generation_patch_for_namespace(
                    &authority,
                    &recovery_identity,
                    GenerationRecoveryPatch::UploadedFileIds(uploaded_snapshot)
                ),
                Ok(true)
            ) {
                if let Some(file_id) = uploaded.last() {
                    let _ = api.delete_reference_scoped(file_id, &worker_scope);
                }
                return;
            }
            if generation_cancel_requested(&cancellations, &request_id) {
                cleanup_cancelled_generation(
                    &backend,
                    &authority,
                    &api,
                    &worker_scope,
                    &request_id,
                    &uploaded,
                    None,
                    &cancellations,
                );
                return;
            }
        }
        if generation_cancel_requested(&cancellations, &request_id) {
            cleanup_cancelled_generation(
                &backend,
                &authority,
                &api,
                &worker_scope,
                &request_id,
                &uploaded,
                None,
                &cancellations,
            );
            return;
        }
        let request = CreateGenerationTask {
            client_request_id: request_id,
            task_type: "image_generation".to_string(),
            model_code,
            prompt: generation_prompt.clone(),
            quality: Some(quality_for_worker.clone()),
            count: Some(count),
            aspect_ratio: Some(aspect_ratio),
            reference_file_ids: Some(uploaded.clone()),
            target_language: None,
        };
        if begin_generation_submission(&authority, &request.client_request_id).is_err() { return; }
        let mut detail = match api.create_task_billing(&request, &billing_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &worker_scope) {
                    return;
                }
                if error.is_billing_rejection() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &recovery_identity);
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: error,
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &recovery_identity);
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: error.generation_message(),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        };
        let task_id = detail.id.clone();
        if generation_cancel_requested(&cancellations, &request.client_request_id) {
            cleanup_cancelled_generation(
                &backend,
                &authority,
                &api,
                &worker_scope,
                &request.client_request_id,
                &uploaded,
                Some(&task_id),
                &cancellations,
            );
            return;
        }
        let task_id_for_record = task_id.clone();
        if !matches!(
            apply_generation_patch_for_namespace(
                &authority,
                &recovery_identity,
                GenerationRecoveryPatch::Accepted {
                    server_task_id: task_id_for_record,
                    uploaded_file_ids: uploaded.clone(),
                    clear_reference_inputs: false
                }
            ),
            Ok(true)
        ) {
            return;
        }
        if sender
            .send(GenerationOutcome::Accepted {
                task_id: task_id.clone(),
            })
            .is_err()
        {
            let _ = api.cancel_scoped(&task_id, &worker_scope);
            return;
        }
        let mut handled_success = BTreeSet::new();
        let mut handled_failure = BTreeSet::new();
        loop {
            if !backend_generation_scope_active(&backend, &worker_scope) {
                return;
            }
            if generation_cancel_requested(&cancellations, &request.client_request_id) {
                cleanup_cancelled_generation(
                    &backend,
                    &authority,
                    &api,
                    &worker_scope,
                    &request.client_request_id,
                    &[],
                    Some(&task_id),
                    &cancellations,
                );
                return;
            }
            let _ = sender.send(GenerationOutcome::Progress {
                percent: detail.progress_percent,
            });
            for item in &detail.items {
                if item.status == "succeeded" && !handled_success.contains(&item.index) {
                    if let Some(file) = item.file.as_ref() {
                        match prepare_runtime_image_delivery(&api, authority.clone(), &request.client_request_id, item.index) {
                            Ok(Some(prepared)) => {
                                if sender.send(GenerationOutcome::NamespaceImageSuccess { prepared: Box::new(prepared), time: Local::now().format("%Y-%m-%d %H:%M").to_string() }).is_err() { return; }
                                handled_success.insert(item.index); continue;
                            }
                            Err(error) => {
                                if detail.terminal() && handled_failure.insert(item.index) { let _ = sender.send(GenerationOutcome::ImageFailure { reason: format!("交付暂未完成，原任务已保留：{error}"), time: Local::now().format("%Y-%m-%d %H:%M").to_string(), delivery: None }); }
                                continue;
                            }
                            Ok(None) => {}
                        }
                        let local_path = generation_download_staging_path(
                            &request.client_request_id,
                            item.index,
                            file,
                        );
                        match api.download_verified_to_path_scoped(file, &worker_scope, &local_path)
                        {
                            Ok(()) => {
                                handled_success.insert(item.index);
                                if sender
                                    .send(GenerationOutcome::ImageSuccess {
                                        local_path: local_path.display().to_string(),
                                        display_prompt: display_prompt_for_worker.clone(),
                                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                        upscale_done: false,
                                        delivery: delivery_confirmation_for_item(
                                            &request.client_request_id,
                                            &detail,
                                            item.index,
                                        ),
                                    })
                                    .is_err()
                                {
                                    let _ = fs::remove_file(local_path);
                                    return;
                                }
                            }
                            Err(error) if detail.terminal() => {
                                handled_failure.insert(item.index);
                                let (reason, delivery) = match failed_delivery_confirmation_for_item(
                                    &worker_scope,
                                    &request.client_request_id,
                                    &detail,
                                    item.index,
                                    None,
                                ) {
                                    Ok(delivery) => (error.generation_message(), Some(delivery)),
                                    Err(_) => (
                                        "本地生成恢复记录无法安全更新，已暂停交付，请重启后重试"
                                            .to_string(),
                                        None,
                                    ),
                                };
                                let _ = sender.send(GenerationOutcome::ImageFailure {
                                    reason,
                                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                    delivery,
                                });
                            }
                            Err(_) => {}
                        }
                    }
                } else if matches!(item.status.as_str(), "failed" | "cancelled")
                    && handled_failure.insert(item.index)
                {
                    let reason = item
                        .failure
                        .as_ref()
                        .map(TaskFailure::generation_message)
                        .unwrap_or_else(|| "服务端未能生成该图片".to_string());
                    let _ = sender.send(GenerationOutcome::ImageFailure {
                        reason,
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                        delivery: None,
                    });
                }
            }
            if detail.terminal() {
                report_unhandled_terminal_failures(
                    &sender,
                    &detail,
                    count.max(1) as usize,
                    &handled_success,
                    &mut handled_failure,
                    "服务端未能生成该图片",
                );
                let expected_success_count = detail.success_count.max(0) as usize;
                if !matches!(
                    apply_generation_patch_for_namespace(
                        &authority,
                        &recovery_identity,
                        GenerationRecoveryPatch::Terminal {
                            expected_success_count
                        }
                    ),
                    Ok(true)
                ) {
                    return;
                }
                let _ = sender.send(GenerationOutcome::Finished);
                return;
            }
            std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
            detail = match api.task_scoped(&task_id, &worker_scope) {
                Ok(detail) => detail,
                Err(error) => {
                    if !backend_generation_scope_active(&backend, &worker_scope) {
                        return;
                    }
                    let _ = sender.send(GenerationOutcome::Failure {
                        reason: error.generation_message(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    });
                    return;
                }
            };
        }
    });
    poll_generation_stream(
        app.as_weak(),
        context,
        original_persistence,
        session_scope,
        Vec::new(),
        Rc::new(RefCell::new(Some(receiver))),
        display_prompt,
        category,
        mode,
        ratio,
        quality,
        state.get_image_model().to_string(),
        "generation".to_string(),
        conversation_id,
        create_conversation,
        generation_reference_paths,
        original_references,
        quote,
        destination == GenerationDestination::Gallery,
        local_task_id,
        Instant::now(),
    );
}

pub(super) fn start_backend_image_edit(
    app: &AppWindow,
    context: AppContext,
    source_path: PathBuf,
    mask_path: PathBuf,
    prompt: String,
    model_code: String,
    quality: String,
) {
    let (scope, authority, _activity) = match context.capture_billing_action(KnownCapability::Bill) {
        Ok(captured) => captured,
        Err(error) => { app.global::<AppState>().set_image_editor_status(error.user_message().into()); return; }
    };
    start_backend_image_edit_with_billing_scope(app, context, authority, &scope, source_path, mask_path, prompt, model_code, quality);
}

pub(super) fn start_backend_image_edit_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: &BillingScope,
    source_path: PathBuf,
    mask_path: PathBuf,
    prompt: String,
    model_code: String,
    quality: String,
) {
    let Some(original_persistence)=context.store.borrow().private_persistence.clone()else{return;};
    if original_persistence.lease()!=authority.lease() || !original_persistence.is_current(){return;}
    let billing_scope = match capture_billing_scope_for_submission(
        context.backend.as_deref(),
        &authority,
        billing_scope,
    ) {
        Ok(scope) => scope,
        Err(error) => {
            app.global::<AppState>()
                .set_image_editor_status(error.user_message().into());
            return;
        }
    };
    let session_scope = billing_scope.request.session.clone();

    let state = app.global::<AppState>();
    let Some(backend) = context.backend.clone() else {
        cleanup_image_edit_input_path(&source_path);
        cleanup_image_edit_input_path(&mask_path);
        state.set_image_editor_generating(false);
        state.set_image_editor_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    };
    let viewer_id = state.get_viewer_id().to_string();
    let viewer_source = state.get_viewer_source().to_string();
    let original = viewer_item(&context.store.borrow(), &viewer_id, &viewer_source).cloned();
    let category = original
        .as_ref()
        .map(|item| item.category.clone())
        .unwrap_or_else(|| resolve_category(&state.get_asset_type().to_string(), &prompt));
    if category_is_generating(&context, &category) {
        cleanup_image_edit_input_path(&source_path);
        cleanup_image_edit_input_path(&mask_path);
        state.set_image_editor_generating(false);
        state.set_image_editor_status("当前分类已有生成任务，请稍后再编辑".into());
        return;
    }
    let mode = original
        .as_ref()
        .map(|item| item.kind.clone())
        .unwrap_or_else(|| state.get_mode().to_string());
    let conversation_id = original
        .as_ref()
        .map(|item| item.conversation_id.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let lineage_reference_paths = original
        .as_ref()
        .map(|item| {
            let source = item.source_path.trim();
            if !source.is_empty() && source != "failed" && Path::new(source).is_file() {
                vec![source.to_string()]
            } else {
                item.reference_paths
                    .iter()
                    .filter(|path| Path::new(path).is_file())
                    .cloned()
                    .collect()
            }
        })
        .unwrap_or_else(|| {
            references_for_category(&context.store.borrow().references, &category)
                .iter()
                .find(|reference| reference.id == viewer_id)
                .map(|reference| vec![reference.source_path.clone()])
                .unwrap_or_default()
        });
    let width = state.get_image_editor_source_width().max(1) as u32;
    let height = state.get_image_editor_source_height().max(1) as u32;
    let ratio = ratio_from_actual_dimensions(width as i32, height as i32);
    let request_id = Uuid::new_v4().simple().to_string();
    let local_task_id = Uuid::new_v4().to_string();
    let source_path_text = source_path.display().to_string();
    let mask_path_text = mask_path.display().to_string();
    let (reference_sha256, reference_size_bytes) =
        match reference_fingerprints_for_namespace(&authority, &[source_path.clone(), mask_path.clone()]) {
            Ok(fingerprints) => fingerprints,
            Err(error) => {
                cleanup_image_edit_input_path(&source_path);
                cleanup_image_edit_input_path(&mask_path);
                state.set_image_editor_generating(false);
                state.set_image_editor_status(format!("图片编辑输入校验失败：{error}").into());
                return;
            }
        };
    let record = PendingGenerationRecord {
        source_asset_id: String::new(),        video_request: None,
        schema_version: 2,
            cancel_requested: false,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        owner_user_id: session_scope.owner_user_id.clone(),
        billing_account_group_id: billing_scope.request.account_group_id.clone(),
        auth_epoch: session_scope.auth_epoch,
        local_task_id: local_task_id.clone(),
        server_task_id: String::new(),
        raw_prompt: prompt.clone(),
        generation_prompt: prompt.clone(),
        task_type: "image_edit".to_string(),
        category: category.clone(),
        mode: mode.clone(),
        ratio: ratio.clone(),
        quality: quality.clone(),
        model_code: model_code.clone(),
        conversation_id: conversation_id.clone(),
        count: 1,
        target_width: width,
        target_height: height,
        create_conversation: false,
        reference_paths: vec![source_path_text.clone(), mask_path_text],
        reference_sha256,
        reference_size_bytes,
        lineage_reference_paths: lineage_reference_paths.clone(),
        uploaded_file_ids: Vec::new(),
        deliveries: Vec::new(),
        terminal: false,
        expected_success_count: 0,
        canvas_source_node_id: String::new(),
        canvas_ui_extraction: false,
    };
    if upsert_pending_generation_for_namespace(&authority, &billing_scope, record.clone()).is_err()
    {
        cleanup_image_edit_record_inputs(&record);
        state.set_image_editor_generating(false);
        state.set_image_editor_status("图片编辑任务准备失败，请重试".into());
        return;
    }
    insert_active_generation(
        &context,
        ActiveGeneration {
            task_id: local_task_id.clone(),
            client_request_id: Some(request_id),
            server_task_id: None,
            category: category.clone(),
            conversation_id: conversation_id.clone(),
            prompt: prompt.clone(),
            credit_cost: state.get_image_editor_estimated_credit_cost(),
            total_count: 1,
            loading_count: 1,
            completed_count: 0,
            success_count: 0,
            failed_count: 0,
            last_failure_reason: None,
            progress: 1,
            eta: 0,
            latest_success_id: None,
            session_scope: session_scope.clone(),
            destination: GenerationDestination::Gallery,
            delivery_download_reservations: Vec::new(),
            registered_cancel_owner:None,
        },
    );
    set_generation_status_for_category(&context, app, &category, "正在上传原图和遮罩...");
    sync_generation_state_for_current_category(&context, app);
    state.set_image_editor_generating(false);
    navigate_to_with_store(app, &context.store.borrow(), "generation");

    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let cancellations = context.cancelled_generation_requests.clone();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        run_generation_with_billing_scope(
            backend,
            authority,
            billing_scope,
            worker_scope,
            record,
            sender,
            cancellations,
        )
    });
    poll_generation_stream(
        app.as_weak(),
        context,
        original_persistence,
        session_scope,
        Vec::new(),
        Rc::new(RefCell::new(Some(receiver))),
        prompt,
        category,
        mode,
        ratio,
        quality,
        model_code,
        "image_edit".to_string(),
        conversation_id,
        false,
        lineage_reference_paths,
        Vec::new(),
        QuoteContext {
            title: String::new(),
            prompt: String::new(),
            ratio: String::new(),
            quality: String::new(),
            width: 0,
            height: 0,
        },
        false,
        local_task_id,
        Instant::now(),
    );
}

pub(super) fn start_backend_upscale(
    app: &AppWindow,
    context: AppContext,
    scale: u32,
    quality: String,
) {
    let (scope, authority, _activity) = match context.capture_billing_action(KnownCapability::Bill) {
        Ok(captured) => captured,
        Err(error) => { app.global::<AppState>().set_viewer_message(error.user_message().into()); return; }
    };
    start_backend_upscale_with_billing_scope(app, context, authority, &scope, scale, quality);
}

struct PreparedUpscaleSubmission {
    backend: Arc<BackendRuntime>,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: BillingScope,
    record: PendingGenerationRecord,
}

impl PreparedUpscaleSubmission {
    // Admission persists the exact captured identity before a worker can upload or bill.
    fn new(
        backend: Arc<BackendRuntime>,
        authority: Arc<NamespaceStorageAuthority>,
        billing_scope: &BillingScope,
        record: PendingGenerationRecord,
    ) -> std::result::Result<Self, ApiError> {
        let billing_scope =
            capture_billing_scope_for_submission(Some(&backend), &authority, billing_scope)?;
        if record.task_type != "image_upscale" || record.reference_paths.len() != 1 {
            return Err(ApiError::LocalState {
                message: "放大任务输入不完整，请重新发起任务".into(),
            });
        }
        upsert_pending_generation_for_namespace(&authority, &billing_scope, record.clone())
            .map_err(|error| ApiError::LocalState {
                message: format!("无法保存放大任务恢复记录：{error}"),
            })?;
        Ok(Self {
            backend,
            authority,
            billing_scope,
            record,
        })
    }

    fn run(
        self,
        sender: mpsc::Sender<GenerationOutcome>,
        cancellations: Arc<Mutex<BTreeSet<String>>>,
        source_prompt_for_result: String,
    ) {
        let Self {
            backend,
            authority,
            billing_scope,
            record: recovery_record,
        } = self;
        let worker_scope = billing_scope.request.session.clone();
        let Ok(_activity) = backend.api.begin_user_work(&worker_scope) else { return; };
        let recovery_identity = recovery_record.identity();
        let request_id = recovery_record.client_request_id.clone();
        let reference_path = recovery_record.reference_paths[0].clone();
        let model_code = recovery_record.model_code.clone();
        let generation_prompt = recovery_record.generation_prompt.clone();
        let quality_for_worker = recovery_record.quality.clone();
        let target_width = recovery_record.target_width;
        let target_height = recovery_record.target_height;
        let api = GenerationApi::new(backend.api.clone());
        if !backend_generation_scope_active(&backend, &worker_scope)
            || !generation_references_match_for_namespace(&authority, &recovery_record)
        {
            return;
        }
        let mut uploaded = Vec::new();
        match api.upload_reference_for_namespace(&PathBuf::from(&reference_path), &authority, &worker_scope, false) {
            Ok(file_id) => uploaded.push(file_id),
            Err(error) => {
                if !backend_generation_scope_active(&backend, &worker_scope) {
                    return;
                }
                if matches!(
                    remove_pending_generation_for_namespace(&authority, &recovery_identity),
                    Ok(true)
                ) {
                    cleanup_upscale_input_path(Path::new(&reference_path));
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: error.generation_message(),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        }
        let uploaded_snapshot = uploaded.clone();
        if !matches!(
            apply_generation_patch_for_namespace(
                &authority,
                &recovery_identity,
                GenerationRecoveryPatch::UploadedAndReleaseInputs(uploaded_snapshot)
            ),
            Ok(true)
        ) {
            if let Some(file_id) = uploaded.last() {
                let _ = api.delete_reference_scoped(file_id, &worker_scope);
            }
            return;
        }
        // The remote file id is now durable in the recovery record, so a restart no longer
        // needs the local managed upload input.
        cleanup_upscale_input_path(Path::new(&reference_path));
        if generation_cancel_requested(&cancellations, &request_id) {
            cleanup_cancelled_generation(
                &backend,
                &authority,
                &api,
                &worker_scope,
                &request_id,
                &uploaded,
                None,
                &cancellations,
            );
            return;
        }
        let request = CreateUpscaleGenerationTask {
            client_request_id: request_id.clone(),
            task_type: "image_upscale".to_string(),
            model_code,
            prompt: generation_prompt,
            quality: quality_for_worker,
            reference_file_ids: uploaded.clone(),
            target_width,
            target_height,
        };
        if begin_generation_submission(&authority, &request.client_request_id).is_err() { return; }
        let mut detail = match api.create_upscale_task_billing(&request, &billing_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &worker_scope) {
                    return;
                }
                if error.is_billing_rejection() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &recovery_identity);
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: error,
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &recovery_identity);
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: error.generation_message(),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        };
        let task_id = detail.id.clone();
        if generation_cancel_requested(&cancellations, &request.client_request_id) {
            cleanup_cancelled_generation(
                &backend,
                &authority,
                &api,
                &worker_scope,
                &request.client_request_id,
                &uploaded,
                Some(&task_id),
                &cancellations,
            );
            return;
        }
        let task_id_for_record = task_id.clone();
        let uploaded_for_record = uploaded.clone();
        if !matches!(
            apply_generation_patch_for_namespace(
                &authority,
                &recovery_identity,
                GenerationRecoveryPatch::Accepted {
                    server_task_id: task_id_for_record,
                    uploaded_file_ids: uploaded_for_record,
                    clear_reference_inputs: false
                }
            ),
            Ok(true)
        ) {
            return;
        }
        if sender
            .send(GenerationOutcome::Accepted {
                task_id: task_id.clone(),
            })
            .is_err()
        {
            let _ = api.cancel_scoped(&task_id, &worker_scope);
            return;
        }
        let mut handled_success = BTreeSet::new();
        let mut handled_failure = BTreeSet::new();
        loop {
            if !backend_generation_scope_active(&backend, &worker_scope) {
                return;
            }
            if generation_cancel_requested(&cancellations, &request.client_request_id) {
                cleanup_cancelled_generation(
                    &backend,
                    &authority,
                    &api,
                    &worker_scope,
                    &request.client_request_id,
                    &[],
                    Some(&task_id),
                    &cancellations,
                );
                return;
            }
            let _ = sender.send(GenerationOutcome::Progress {
                percent: detail.progress_percent,
            });
            for item in &detail.items {
                if item.status == "succeeded" && !handled_success.contains(&item.index) {
                    if let Some(file) = item.file.as_ref() {
                        match prepare_runtime_image_delivery(&api, authority.clone(), &request.client_request_id, item.index) {
                            Ok(Some(prepared)) => {
                                if sender.send(GenerationOutcome::NamespaceImageSuccess { prepared: Box::new(prepared), time: Local::now().format("%Y-%m-%d %H:%M").to_string() }).is_err() { return; }
                                handled_success.insert(item.index); continue;
                            }
                            Err(error) => {
                                if detail.terminal() && handled_failure.insert(item.index) { let _ = sender.send(GenerationOutcome::ImageFailure { reason: format!("交付暂未完成，原任务已保留：{error}"), time: Local::now().format("%Y-%m-%d %H:%M").to_string(), delivery: None }); }
                                continue;
                            }
                            Ok(None) => {}
                        }
                        let local_path = generation_download_staging_path(
                            &request.client_request_id,
                            item.index,
                            file,
                        );
                        match api.download_verified_to_path_scoped(file, &worker_scope, &local_path)
                        {
                            Ok(()) => {
                                handled_success.insert(item.index);
                                if sender
                                    .send(GenerationOutcome::ImageSuccess {
                                        local_path: local_path.display().to_string(),
                                        display_prompt: source_prompt_for_result.clone(),
                                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                        upscale_done: true,
                                        delivery: delivery_confirmation_for_item(
                                            &request.client_request_id,
                                            &detail,
                                            item.index,
                                        ),
                                    })
                                    .is_err()
                                {
                                    let _ = fs::remove_file(local_path);
                                    return;
                                }
                            }
                            Err(error) if detail.terminal() => {
                                handled_failure.insert(item.index);
                                let (reason, delivery) = match failed_delivery_confirmation_for_item(
                                    &worker_scope,
                                    &request.client_request_id,
                                    &detail,
                                    item.index,
                                    None,
                                ) {
                                    Ok(delivery) => (error.generation_message(), Some(delivery)),
                                    Err(_) => (
                                        "本地生成恢复记录无法安全更新，已暂停交付，请重启后重试"
                                            .to_string(),
                                        None,
                                    ),
                                };
                                let _ = sender.send(GenerationOutcome::ImageFailure {
                                    reason,
                                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                    delivery,
                                });
                            }
                            Err(_) => {}
                        }
                    }
                } else if matches!(item.status.as_str(), "failed" | "cancelled")
                    && handled_failure.insert(item.index)
                {
                    let reason = item
                        .failure
                        .as_ref()
                        .map(TaskFailure::generation_message)
                        .unwrap_or_else(|| "服务端未能放大该图片".to_string());
                    let _ = sender.send(GenerationOutcome::ImageFailure {
                        reason,
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                        delivery: None,
                    });
                }
            }
            if detail.terminal() {
                report_unhandled_terminal_failures(
                    &sender,
                    &detail,
                    1,
                    &handled_success,
                    &mut handled_failure,
                    "服务端未能放大该图片",
                );
                let expected_success_count = detail.success_count.max(0) as usize;
                if !matches!(
                    apply_generation_patch_for_namespace(
                        &authority,
                        &recovery_identity,
                        GenerationRecoveryPatch::Terminal {
                            expected_success_count
                        }
                    ),
                    Ok(true)
                ) {
                    return;
                }
                let _ = sender.send(GenerationOutcome::Finished);
                return;
            }
            std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
            detail = match api.task_scoped(&task_id, &worker_scope) {
                Ok(detail) => detail,
                Err(error) => {
                    if !backend_generation_scope_active(&backend, &worker_scope) {
                        return;
                    }
                    let _ = sender.send(GenerationOutcome::Failure {
                        reason: error.generation_message(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    });
                    return;
                }
            };
        }
    }
}

pub(super) fn start_backend_upscale_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: &BillingScope,
    scale: u32,
    quality: String,
) {
    let Some(original_persistence)=context.store.borrow().private_persistence.clone()else{return;};
    if original_persistence.lease()!=authority.lease() || !original_persistence.is_current(){return;}
    let billing_scope = match capture_billing_scope_for_submission(
        context.backend.as_deref(),
        &authority,
        billing_scope,
    ) {
        Ok(scope) => scope,
        Err(error) => {
            app.global::<AppState>()
                .set_viewer_message(error.user_message().into());
            return;
        }
    };
    let session_scope = billing_scope.request.session.clone();

    let state = app.global::<AppState>();
    if state.get_viewer_processing() {
        return;
    }
    if state.get_viewer_upscale_done() {
        state.set_viewer_message(
            processing_done_message(
                app,
                ProcessImageMode::Upscale {
                    scale: 2,
                    target_long_edge: 2048,
                },
            )
            .into(),
        );
        return;
    }
    if !require_online_operation(app, "清晰放大") {
        return;
    }
    let Some(backend) = context.backend.clone() else {
        state.set_viewer_message("服务端尚未初始化，请重启客户端后重试".into());
        return;
    };
    let model_code = state.get_image_model().to_string();
    if model_code.trim().is_empty() {
        state.set_viewer_message("服务端没有可用的图像模型".into());
        return;
    }

    let source = {
        let store = context.store.borrow();
        upscale_source_for_viewer(app, &store)
    };
    let Some(source) = source else {
        state.set_viewer_message("未找到要放大的图片".into());
        return;
    };
    if category_is_generating(&context, &source.category) {
        state.set_viewer_message("当前分类已有生成任务，请稍后再放大".into());
        return;
    }
    let Some((source_width, source_height)) = viewer_source_dimensions(&state, &source) else {
        state.set_viewer_message("图片尺寸不可用，无法放大".into());
        return;
    };
    let selected_quality = if quality.eq_ignore_ascii_case("4K") {
        "4K"
    } else {
        "2K"
    }
    .to_string();
    let target_long_edge = upscale_quality_long_edge(&selected_quality);
    if source_width.max(source_height) >= target_long_edge {
        let message = if target_long_edge >= 4096 {
            "当前图片尺寸已达到或超过 4K，暂不支持继续放大"
        } else {
            "当前图片已达到或超过 2K，请选择 4K 放大"
        };
        state.set_viewer_message(message.into());
        return;
    }
    let (target_width, target_height) = upscale_dimensions(
        source_width,
        source_height,
        scale.clamp(2, 4),
        target_long_edge,
    );
    if target_width <= source_width || target_height <= source_height {
        state.set_viewer_message("当前档位无法增大原图尺寸，请选择更高档位".into());
        return;
    }
    let billing_quality = quality_for_target_dimensions(target_width, target_height);
    let upload_path = match upscale_upload_path(app, &state, &source) {
        Ok(path) => path,
        Err(error) => {
            state.set_viewer_message(format!("放大任务准备失败：{error}").into());
            return;
        }
    };

    let request_id = Uuid::new_v4().simple().to_string();
    let local_task_id = Uuid::new_v4().to_string();
    let conversation_id = source.conversation_id.clone();
    let display_prompt = if source.prompt.trim().is_empty() {
        source.title.clone()
    } else {
        source.prompt.clone()
    };
    let raw_prompt = format!(
        "{} 清晰放大{}X",
        if source.title.trim().is_empty() {
            "图片"
        } else {
            source.title.trim()
        },
        scale.clamp(2, 4),
    );
    let generation_prompt = build_upscale_prompt(
        &display_prompt,
        target_width,
        target_height,
        scale.clamp(2, 4),
        &billing_quality,
    );
    let ratio = ratio_from_actual_dimensions(target_width as i32, target_height as i32);
    let reference_path = upload_path.display().to_string();
    let (reference_sha256, reference_size_bytes) =
        match reference_fingerprints_for_namespace(&authority, std::slice::from_ref(&upload_path)) {
            Ok(fingerprints) => fingerprints,
            Err(error) => {
                cleanup_upscale_input_path(&upload_path);
                state.set_viewer_message(format!("放大输入校验失败：{error}").into());
                return;
            }
        };
    let recovery_record = PendingGenerationRecord {
        source_asset_id: String::new(),        video_request: None,
        schema_version: 2,
            cancel_requested: false,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        owner_user_id: session_scope.owner_user_id.clone(),
        billing_account_group_id: billing_scope.request.account_group_id.clone(),
        auth_epoch: session_scope.auth_epoch,
        local_task_id: local_task_id.clone(),
        server_task_id: String::new(),
        raw_prompt: raw_prompt.clone(),
        generation_prompt: generation_prompt.clone(),
        task_type: "image_upscale".to_string(),
        category: source.category.clone(),
        mode: source.kind.clone(),
        ratio: ratio.clone(),
        quality: billing_quality.clone(),
        model_code: model_code.clone(),
        conversation_id: conversation_id.clone(),
        count: 1,
        target_width,
        target_height,
        create_conversation: false,
        reference_paths: vec![reference_path.clone()],
        reference_sha256,
        reference_size_bytes,
        lineage_reference_paths: source.reference_paths.clone(),
        uploaded_file_ids: vec![],
        deliveries: vec![],
        terminal: false,
        expected_success_count: 0,
        canvas_source_node_id: String::new(),
        canvas_ui_extraction: false,
    };
    let submission = match PreparedUpscaleSubmission::new(
        backend,
        authority,
        &billing_scope,
        recovery_record,
    ) {
        Ok(submission) => submission,
        Err(error) => {
            cleanup_upscale_input_path(&upload_path);
            state.set_viewer_message(error.user_message().into());
            return;
        }
    };

    insert_active_generation(
        &context,
        ActiveGeneration {
            task_id: local_task_id.clone(),
            client_request_id: Some(request_id.clone()),
            server_task_id: None,
            category: source.category.clone(),
            conversation_id: conversation_id.clone(),
            prompt: raw_prompt.clone(),
            credit_cost: 0,
            total_count: 1,
            loading_count: 1,
            completed_count: 0,
            success_count: 0,
            failed_count: 0,
            last_failure_reason: None,
            progress: 1,
            eta: 0,
            latest_success_id: None,
            session_scope: session_scope.clone(),
            destination: GenerationDestination::Gallery,
            delivery_download_reservations: Vec::new(),
            registered_cancel_owner:None,
        },
    );
    state.set_viewer_processing(true);
    state.set_viewer_processing_progress(0);
    state.set_viewer_processing_label("正在提交放大任务".into());
    state.set_upscale_open(false);
    state.set_viewer_open(false);
    state.set_viewer_processing(false);
    state.set_viewer_processing_progress(0);
    set_generation_status_for_category(&context, app, &source.category, "正在上传原图...");
    sync_generation_state_for_current_category(&context, app);
    navigate_to_with_store(app, &context.store.borrow(), "generation");

    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let cancellations = context.cancelled_generation_requests.clone();
    let source_prompt_for_result = display_prompt.clone();
    let source_category = source.category.clone();
    let source_reference_paths = source.reference_paths.clone();
    std::thread::spawn(move || {
        submission.run(sender, cancellations, source_prompt_for_result);
    });

    poll_generation_stream(
        app.as_weak(),
        context,
        original_persistence,
        session_scope,
        Vec::new(),
        Rc::new(RefCell::new(Some(receiver))),
        raw_prompt,
        source_category,
        source.kind,
        ratio,
        billing_quality,
        state.get_image_model().to_string(),
        "generation".to_string(),
        conversation_id,
        false,
        source_reference_paths,
        vec![],
        QuoteContext {
            title: String::new(),
            prompt: String::new(),
            ratio: String::new(),
            quality: String::new(),
            width: 0,
            height: 0,
        },
        false,
        local_task_id,
        Instant::now(),
    );
}

fn upscale_source_for_viewer(app: &AppWindow, store: &Store) -> Option<UpscaleSource> {
    let state = app.global::<AppState>();
    let id = state.get_viewer_id().to_string();
    let source = state.get_viewer_source().to_string();
    if source == "reference" {
        let category = resolve_category(&state.get_asset_type().to_string(), "");
        let canvas = state.get_page().as_str() == "canvas";
        let reference = references_for_context(store, &category, canvas)
            .iter()
            .find(|item| item.id == id)?;
        return Some(UpscaleSource {
            title: "参考图".to_string(),
            category,
            kind: state.get_mode().to_string(),
            prompt: state.get_viewer_prompt().to_string(),
            conversation_id: String::new(),
            source_path: reference.source_path.clone(),
            reference_paths: vec![reference.source_path.clone()],
            width: 0,
            height: 0,
        });
    }
    let item = viewer_item(store, &id, &source)?;
    Some(UpscaleSource {
        title: item.title.clone(),
        category: item.category.clone(),
        kind: item.kind.clone(),
        prompt: item.prompt.clone(),
        conversation_id: item.conversation_id.clone(),
        source_path: item.source_path.clone(),
        reference_paths: item.reference_paths.clone(),
        width: item.width,
        height: item.height,
    })
}

fn viewer_source_dimensions(state: &AppState, source: &UpscaleSource) -> Option<(u32, u32)> {
    if source.width > 0 && source.height > 0 {
        return Some((source.width as u32, source.height as u32));
    }
    let viewer_width = state.get_viewer_width();
    let viewer_height = state.get_viewer_height();
    if viewer_width > 0 && viewer_height > 0 {
        return Some((viewer_width as u32, viewer_height as u32));
    }
    let source_path = Path::new(source.source_path.trim());
    if source_path.is_file() {
        return inspect_image_dimensions(source_path).ok();
    }
    let buffer = state.get_viewer_image().to_rgba8()?;
    if buffer.width() == 0 || buffer.height() == 0 {
        None
    } else {
        Some((buffer.width(), buffer.height()))
    }
}

fn quality_for_target_dimensions(width: u32, height: u32) -> String {
    let long_edge = width.max(height);
    if long_edge <= 1024 {
        "1K".to_string()
    } else if long_edge <= 2048 {
        "2K".to_string()
    } else {
        "4K".to_string()
    }
}

fn upscale_upload_path(
    _app: &AppWindow,
    state: &AppState,
    source: &UpscaleSource,
) -> Result<PathBuf> {
    let trimmed = source.source_path.trim();
    if !trimmed.is_empty() && trimmed != "failed" && trimmed != "asset" {
        let path = PathBuf::from(trimmed);
        if path.is_file() {
            return Ok(path);
        }
    }
    let buffer = state
        .get_viewer_image()
        .to_rgba8()
        .ok_or_else(|| anyhow!("图片数据不可上传"))?;
    let width = buffer.width();
    let height = buffer.height();
    let rgba = image::RgbaImage::from_raw(width, height, buffer.as_bytes().to_vec())
        .ok_or_else(|| anyhow!("图片数据不可上传"))?;
    let bytes = encode_png_rgba(&rgba, width, height)?;
    // This is a recoverable upload input, not a user work. Keep it in the fixed managed
    // subtree so later cleanup can never reach a user-selected output directory.
    let dir = managed_upscale_input_dir();
    if !ensure_managed_subdirectory(&dir) {
        return Err(anyhow!("无法创建安全的放大暂存目录"));
    }
    let stem = sanitize_filename(&format!("{}-upscale-source", source.title));
    let path = unique_path(dir.join(format!(
        "{}-{}.png",
        Local::now().format("%Y%m%d%H%M%S%3f"),
        stem,
    )));
    atomic_write_file(&path, &bytes)?;
    Ok(path)
}

fn build_upscale_prompt(
    original_prompt: &str,
    target_width: u32,
    target_height: u32,
    scale: u32,
    quality: &str,
) -> String {
    let _ = original_prompt; // Creation instructions must not compete with the source pixels.
    format!(
        "请仅以参考图为依据进行保真清晰放大。整张画面等比例放大，保持主体数量、各视图位置、相对尺寸、构图、颜色、材质、笔触、光照、背景和留白不变；仅改善清晰度与现有纹理。禁止扩图、裁切、重新设计；禁止新增边框、装饰、文字、云纹或背景。放大倍率：{}X，目标清晰度：{}，输出尺寸必须为 {}x{}。",
        scale.clamp(2, 4),
        quality,
        target_width,
        target_height,
    )
}

fn generation_cancel_requested(
    cancellations: &Arc<Mutex<BTreeSet<String>>>,
    client_request_id: &str,
) -> bool {
    cancellations
        .lock()
        .map(|items| items.contains(client_request_id))
        .unwrap_or(false)
}

fn begin_generation_submission(authority: &NamespaceStorageAuthority, key: &str) -> Result<()> {
    let row = load_pending_generations_for_namespace(authority)?.into_iter()
        .find(|row| row.client_request_id == key).ok_or_else(|| anyhow!("original generation intent missing"))?;
    anyhow::ensure!(apply_generation_patch_for_namespace(authority, &row.identity(), GenerationRecoveryPatch::BeginSubmission)?, "original generation intent changed");
    Ok(())
}

pub(super) fn cleanup_cancelled_generation(
    backend: &BackendRuntime,
    authority: &NamespaceStorageAuthority,
    api: &GenerationApi,
    session_scope: &SessionScope,
    client_request_id: &str,
    uploaded_file_ids: &[String],
    server_task_id: Option<&str>,
    cancellations: &Arc<Mutex<BTreeSet<String>>>,
) -> bool {
    if !backend_generation_scope_active(backend, session_scope) {
        return false;
    }
    let result = (|| -> Result<bool> {
        let row = load_pending_generations_for_namespace(authority)?.into_iter()
            .find(|row| row.client_request_id == client_request_id).ok_or_else(|| anyhow!("cancellation intent missing"))?;
        anyhow::ensure!(apply_generation_patch_for_namespace(authority, &row.identity(), GenerationRecoveryPatch::RequestCancellation)?, "cancellation identity changed");
        // No ID is not proof that POST never arrived. Preserve the exact
        // tombstone and inputs; startup may not turn this into new billed work.
        let Some(task_id) = server_task_id.filter(|id| !id.is_empty()) else { return Ok(false); };
        let before = api.task_scoped(task_id, session_scope)?;
        require_saved_group(&row.billing_account_group_id, &before.billing_account_group_id)?;
        anyhow::ensure!(before.id == task_id, "cancel resource identity mismatch");
        api.cancel_scoped(task_id, session_scope)?;
        let after = api.task_scoped(task_id, session_scope)?;
        require_saved_group(&row.billing_account_group_id, &after.billing_account_group_id)?;
        anyhow::ensure!(after.id == task_id, "cancel resource identity mismatch");
        if after.status != "cancelled" || after.success_count != 0 { return Ok(false); }
        for file_id in uploaded_file_ids { api.delete_reference_scoped(file_id, session_scope)?; }
        remove_pending_generation_for_namespace(authority, &row.identity())
    })();
    if !matches!(result, Ok(true)) { return false; }
    if let Ok(mut items) = cancellations.lock() {
        items.remove(client_request_id);
    }
    true
}

fn cleanup_image_edit_input_path(path: &Path) {
    let directory = managed_image_edit_input_dir();
    if safe_managed_subdirectory(&directory)
        && is_managed_image_edit_input_path(path)
        && fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_file() && !metadata.file_type().is_symlink()
        })
    {
        let _ = fs::remove_file(path);
    }
}

fn managed_image_edit_input_dir() -> PathBuf {
    configured_output_directory().join("image-edit-inputs")
}

fn is_managed_image_edit_input_path(path: &Path) -> bool {
    let safe_parent = path.parent() == Some(managed_image_edit_input_dir().as_path());
    safe_parent && is_image_edit_input_name(path)
}

fn is_image_edit_input_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| {
            name.ends_with("-source.png")
                || name.contains("-source-") && name.ends_with(".png")
                || name.ends_with("-mask.png")
                || name.contains("-mask-") && name.ends_with(".png")
        })
}

fn managed_upscale_input_dir() -> PathBuf {
    configured_output_directory().join("upscale-references")
}

fn is_managed_upscale_input_path(path: &Path) -> bool {
    path.parent() == Some(managed_upscale_input_dir().as_path()) && is_upscale_input_name(path)
}

fn is_upscale_input_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.contains("-upscale-source") && name.ends_with(".png"))
}

fn cleanup_upscale_input_path(path: &Path) {
    let directory = managed_upscale_input_dir();
    if safe_managed_subdirectory(&directory)
        && is_managed_upscale_input_path(path)
        && fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_file() && !metadata.file_type().is_symlink()
        })
    {
        let _ = fs::remove_file(path);
    }
}

fn cleanup_image_edit_record_inputs(record: &PendingGenerationRecord) {
    if record.task_type != "image_edit" {
        return;
    }
    for path in &record.reference_paths {
        cleanup_image_edit_input_path(Path::new(path));
    }
}

fn cleanup_upscale_record_inputs(record: &PendingGenerationRecord) {
    if record.task_type != "image_upscale" {
        return;
    }
    for path in &record.reference_paths {
        cleanup_upscale_input_path(Path::new(path));
    }
}

fn cleanup_generation_record_inputs(record: &PendingGenerationRecord) {
    cleanup_image_edit_record_inputs(record);
    cleanup_upscale_record_inputs(record);
}

fn release_recovered_upscale_inputs_for_namespace(
    record: &mut PendingGenerationRecord,
    authority: &NamespaceStorageAuthority,
) -> bool {
    if record.task_type != "image_upscale" || record.reference_paths.is_empty() {
        return true;
    }
    if !matches!(
        apply_generation_patch_for_namespace(
            authority,
            &record.identity(),
            GenerationRecoveryPatch::ReleaseReferenceInputs
        ),
        Ok(true)
    ) {
        return false;
    }
    cleanup_upscale_record_inputs(record);
    record.reference_paths.clear();
    record.reference_sha256.clear();
    record.reference_size_bytes.clear();
    true
}

struct GenerationRecoveryScan { records: Vec<PendingGenerationRecord>, blocked: usize }
pub(super) fn recover_pending_generations(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else { return; };
    let Some(scope) = context.current_account_session_scope() else { return; };
    let Ok(lease) = context.namespace_for(&scope) else { return; };
    let Ok(authority) = context.storage_authority_for(&lease).map(Arc::new) else { return; };
    let Ok(activity) = backend.api.begin_user_work(&scope) else { return; };
    let (sender, receiver) = mpsc::channel();
    let worker_scope = scope.clone();
    std::thread::spawn(move || {
        let result = (|| {
            let records = load_pending_generations_for_namespace(&authority).map_err(|_| ())?;
            let mut recovered = Vec::new(); let mut blocked = 0;
            for record in records {
                if activity.is_quiescing() || !backend.api.user_work_is_current(&worker_scope) { break; }
                if !matches!(record.task_type.as_str(), "image_generation" | "image_edit" | "image_upscale"
                    | "image_watermark_removal" | "image_colorization" | "image_enhancement" | "image_cutout"
                    | "image_to_video")
                    || record.canvas_ui_extraction { blocked += 1; continue; }
                if let Ok(Some(record)) = bind_generation_recovery_candidate(&backend, &authority, &worker_scope, record) {
                    recovered.push(record);
                } else { blocked += 1; }
            }
            Ok(GenerationRecoveryScan { records: recovered, blocked })
        })();
        drop(activity);
        let _ = sender.send(result);
    });
    poll_server_generation_recovery(app.as_weak(), context, scope, Rc::new(RefCell::new(Some(receiver))));
}

fn reconcile_recoverable_delivery_cards(
    app: &AppWindow,
    context: &AppContext,
    session_scope: &SessionScope,
) -> bool {
    let recoverable_ids = match recoverable_failed_asset_ids(
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
    ) {
        Ok(ids) => ids,
        Err(_) => {
            let mut store = context.store.borrow_mut();
            for item in &mut store.generations {
                item.delivery_recoverable = false;
                item.delivery_downloading = false;
            }
            push_all(app, &store);
            app.global::<AppState>().set_generation_status(
                "本地生成恢复记录无法安全读取，已暂停恢复下载，请重启后重试".into(),
            );
            return false;
        }
    };
    let mut store = context.store.borrow_mut();
    for item in &mut store.generations {
        item.delivery_recoverable = recoverable_ids.contains(&item.id);
        item.delivery_downloading = false;
    }
    push_all(app, &store);
    true
}

fn bind_generation_recovery_candidate(
    backend: &BackendRuntime, authority: &Arc<NamespaceStorageAuthority>,
    session_scope: &SessionScope, mut record: PendingGenerationRecord,
) -> std::result::Result<Option<PendingGenerationRecord>, ApiError> {
    if record.owner_user_id != session_scope.owner_user_id || authority.user_public_id() != session_scope.owner_user_id {
        return Ok(None);
    }
    let api = GenerationApi::new(backend.api.clone()).with_saved_group(&record.billing_account_group_id);
    if record.cancel_requested && record.server_task_id.is_empty() { return Ok(None); }
    let mut detail = if record.server_task_id.is_empty() {
        let replay = SavedReplayRequest::generation(authority.clone(), session_scope, &record.client_request_id).map_err(transition_error)?;
        backend.api.replay_saved::<GenerationTaskDetail>(&replay)?.data
    } else {
        api.task_scoped(&record.server_task_id, session_scope)?
    };
    require_saved_group(&record.billing_account_group_id, &detail.billing_account_group_id)?;
    if !backend.api.user_work_is_current(session_scope) { return Err(ApiError::AuthenticationRequired); }
    if record.auth_epoch != session_scope.auth_epoch {
        if !rebind_pending_generation_epoch_for_namespace(authority, &record.identity(), session_scope.auth_epoch).map_err(transition_error)? { return Ok(None); }
        record.auth_epoch = session_scope.auth_epoch;
    }
    if record.cancel_requested {
        if !detail.terminal() {
            api.cancel_scoped(&record.server_task_id, session_scope)?;
            detail = api.task_scoped(&record.server_task_id, session_scope)?;
            require_saved_group(&record.billing_account_group_id, &detail.billing_account_group_id)?;
        }
        if !detail.terminal() { return Ok(None); }
        if detail.status == "cancelled" && detail.success_count == 0 {
            remove_pending_generation_for_namespace(authority, &record.identity()).map_err(transition_error)?;
            return Ok(None);
        }
        // Terminal partial output still enters the retained delivery consumer.
    }
    if record.server_task_id.is_empty() {
        if !apply_generation_patch_for_namespace(authority, &record.identity(), GenerationRecoveryPatch::Accepted {
            server_task_id: detail.id.clone(), uploaded_file_ids: record.uploaded_file_ids.clone(), clear_reference_inputs: false,
        }).map_err(transition_error)? { return Ok(None); }
        record.server_task_id = detail.id;
    }
    Ok(Some(record))
}

const ORPHANED_GENERATION_INPUT_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Default, Deserialize)]
struct GenerationCleanupSnapshot {
    #[serde(default)]
    generations: Vec<PendingGenerationRecord>,
}

fn load_generation_cleanup_snapshot() -> Result<Vec<PendingGenerationRecord>> {
    // TEMP(team-accounts): no global bytes confer recovery authority.
    Err(RecoveryError::NamespaceRequired.into())
}

fn cleanup_path_identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn retained_generation_paths(records: &[PendingGenerationRecord]) -> BTreeSet<PathBuf> {
    records
        .iter()
        .flat_map(|record| {
            record.reference_paths.iter().map(String::as_str).chain(
                record
                    .deliveries
                    .iter()
                    .map(|delivery| delivery.local_path.as_str()),
            )
        })
        .filter(|path| !path.trim().is_empty())
        .map(Path::new)
        .map(cleanup_path_identity)
        .collect()
}

fn retained_task_input_paths(
    records: &[PendingGenerationRecord],
    task_type: &str,
) -> BTreeSet<PathBuf> {
    records
        .iter()
        .filter(|record| record.task_type == task_type)
        .flat_map(|record| record.reference_paths.iter())
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .map(|path| cleanup_path_identity(&path))
        .collect()
}

fn cleanup_orphaned_input_directory(
    directory: &Path,
    retained: &BTreeSet<PathBuf>,
    now: std::time::SystemTime,
    managed_name: impl Fn(&Path) -> bool,
) {
    let Ok(directory_metadata) = fs::symlink_metadata(directory) else {
        return;
    };
    if !directory_metadata.file_type().is_dir() || directory_metadata.file_type().is_symlink() {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !managed_name(&path) || retained.contains(&cleanup_path_identity(&path)) {
            continue;
        }
        let stale = fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= ORPHANED_GENERATION_INPUT_GRACE);
        if stale {
            let _ = fs::remove_file(path);
        }
    }
}

fn cleanup_orphaned_image_edit_inputs(_app: &AppWindow, records: &[PendingGenerationRecord]) {
    let directory = managed_image_edit_input_dir();
    if !safe_managed_subdirectory(&directory) {
        return;
    }
    cleanup_orphaned_input_directory(
        &directory,
        &retained_task_input_paths(records, "image_edit"),
        std::time::SystemTime::now(),
        is_image_edit_input_name,
    );
}

fn cleanup_orphaned_upscale_inputs(records: &[PendingGenerationRecord]) {
    let directory = managed_upscale_input_dir();
    if !safe_managed_subdirectory(&directory) {
        return;
    }
    cleanup_orphaned_input_directory(
        &directory,
        &retained_task_input_paths(records, "image_upscale"),
        std::time::SystemTime::now(),
        is_upscale_input_name,
    );
}

pub(super) fn cleanup_generation_transients_at_startup(app: &AppWindow) {
    let records = load_generation_cleanup_snapshot();
    let retained = records
        .as_ref()
        .ok()
        .map(|records| retained_generation_paths(records));
    // System reference-upload cleanup remains safe when recovery JSON is unreadable. App-data
    // cleanup receives None and fails closed so no pending task input can be lost.
    cleanup_stale_generation_transients(retained.as_ref());
    let Ok(records) = records else {
        return;
    };
    cleanup_orphaned_image_edit_inputs(app, &records);
    cleanup_orphaned_upscale_inputs(&records);
}

// TEMP(team-accounts): remove in Task 10 after namespace admission is wired.
fn recover_server_generation_tasks(
    app: &AppWindow,
    _context: AppContext,
    _session_scope: SessionScope,
    _known_server_ids: BTreeSet<String>,
) {
    app.global::<AppState>().set_generation_status(
        ApiError::LocalState {
            message: "任务恢复暂不可用，请稍后重试".to_owned(),
        }
        .user_message()
        .into(),
    );
}

fn poll_server_generation_recovery(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<GenerationRecoveryScan, ()>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err(()))
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_server_generation_recovery(app_weak, context, session_scope, receiver);
            return;
        };
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let Ok(scan) = outcome else {
            receiver.borrow_mut().take();
            if let Some(app) = app_weak.upgrade() { app.global::<AppState>().set_generation_status("恢复记录无法安全读取；原记录已保留".into()); }
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if scan.blocked > 0 { app.global::<AppState>().set_generation_status("部分旧任务恢复受阻，原付款账号和记录已保留；不会切换付款账号重试".into()); }
        for record in scan.records {
            if record.task_type == "image_watermark_removal" {
                resume_pending_watermark_removal(&app, context.clone(), record);
                continue;
            }
            if record.task_type == "image_enhancement" {
                resume_pending_image_enhancement(&app, context.clone(), record);
                continue;
            }
            if record.task_type == "image_cutout" {
                resume_pending_image_cutout(&app, context.clone(), record);
                continue;
            }
            if record.task_type == "image_colorization" {
                resume_pending_image_colorization(&app, context.clone(), record);
                continue;
            }
            if !category_is_generating(&context, &record.category) {
                resume_pending_generation(&app, context.clone(), record);
            }
        }
    });
}

fn resume_pending_generation(
    app: &AppWindow,
    context: AppContext,
    record: PendingGenerationRecord,
) {
    let Some(original_persistence)=context.store.borrow().private_persistence.clone()else{return;};
    if original_persistence.lease().namespace.user_public_id()!=record.owner_user_id
        || original_persistence.lease().auth_epoch!=record.auth_epoch || !original_persistence.is_current(){return;}
    if record.canvas_ui_extraction {
        app.global::<AppState>().set_generation_status("旧画布任务记录已保留，当前版本不支持自动恢复此记录".into());
        return;
    }
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let Ok(lease) = context.namespace_for(&SessionScope { owner_user_id: record.owner_user_id.clone(), auth_epoch: record.auth_epoch }) else { return; };
    let Ok(authority) = context.storage_authority_for(&lease).map(Arc::new) else { return; };
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    if !generation_scope_matches_context(&context, &session_scope) {
        return;
    }
    let Some(delivery_download_reservations) =
        reserve_recovered_delivery_downloads(app, &context, &record)
    else {
        return;
    };
    let saved_count = record
        .deliveries
        .iter()
        .filter(|item| item.acknowledged && !item.abandoned)
        .count() as i32;
    let is_canvas_generation = !record.canvas_source_node_id.is_empty();
    insert_active_generation(
        &context,
        ActiveGeneration {
            task_id: record.local_task_id.clone(),
            client_request_id: Some(record.client_request_id.clone()),
            server_task_id: (!record.server_task_id.is_empty())
                .then(|| record.server_task_id.clone()),
            category: record.category.clone(),
            conversation_id: record.conversation_id.clone(),
            prompt: record.raw_prompt.clone(),
            credit_cost: 0,
            total_count: record.count,
            loading_count: (record.count - saved_count).max(0),
            completed_count: saved_count,
            success_count: saved_count,
            failed_count: 0,
            last_failure_reason: None,
            progress: if saved_count > 0 { 50 } else { 1 },
            eta: 0,
            latest_success_id: None,
            session_scope: session_scope.clone(),
            destination: if record.canvas_source_node_id.is_empty() {
                GenerationDestination::Gallery
            } else {
                GenerationDestination::Canvas {
                    source_node_id: record.canvas_source_node_id.clone(),
                }
            },
            delivery_download_reservations: delivery_download_reservations.clone(),
            registered_cancel_owner:None,
        },
    );
    let state = app.global::<AppState>();
    if record.create_conversation
        && !state
            .get_conversations()
            .iter()
            .any(|item| item.id.as_str() == record.conversation_id)
    {
        let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
        conversations.insert(
            0,
            ConversationItem {
                id: record.conversation_id.clone().into(),
                title: short_text(&record.raw_prompt, 10).into(),
                image: Image::default(),
                loading: true,
            },
        );
        state.set_conversations(ModelRc::new(VecModel::from(conversations)));
    }
    set_generation_status_for_category(&context, app, &record.category, "正在恢复未完成任务...");
    sync_generation_state_for_current_category(&context, app);

    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let worker_record = record.clone();
    let generation_reference_paths = if !record.lineage_reference_paths.is_empty() {
        record.lineage_reference_paths.clone()
    } else if matches!(record.task_type.as_str(), "image_edit" | "image_upscale") {
        Vec::new()
    } else {
        record.reference_paths.clone()
    };
    let result_origin = if record.task_type == "image_edit" {
        "image_edit"
    } else {
        "generation"
    }
    .to_string();
    let cancellations = context.cancelled_generation_requests.clone();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        run_recovered_generation_worker(backend, authority, worker_scope, worker_record, sender, cancellations)
    });
    poll_generation_stream(
        app.as_weak(),
        context,
        original_persistence,
        session_scope,
        delivery_download_reservations,
        Rc::new(RefCell::new(Some(receiver))),
        record.raw_prompt,
        record.category,
        record.mode,
        record.ratio,
        record.quality,
        record.model_code,
        result_origin,
        record.conversation_id,
        record.create_conversation,
        generation_reference_paths,
        vec![],
        QuoteContext {
            title: String::new(),
            prompt: String::new(),
            ratio: String::new(),
            quality: String::new(),
            width: 0,
            height: 0,
        },
        !is_canvas_generation,
        record.local_task_id,
        Instant::now(),
    );
}

fn run_recovered_generation_worker(
    backend: Arc<BackendRuntime>, authority: Arc<NamespaceStorageAuthority>,
    session_scope: SessionScope, record: PendingGenerationRecord,
    sender: mpsc::Sender<GenerationOutcome>, cancellations: Arc<Mutex<BTreeSet<String>>>,
) {
    if record.server_task_id.is_empty() { return; }
    run_generation_record(backend, authority, None, session_scope, record, sender, cancellations);
}

fn run_generation_with_billing_scope(
    backend: Arc<BackendRuntime>,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: BillingScope,
    session_scope: SessionScope,
    mut record: PendingGenerationRecord,
    sender: mpsc::Sender<GenerationOutcome>,
    cancellations: Arc<Mutex<BTreeSet<String>>>,
) {
    if capture_billing_scope_for_submission(Some(&backend), &authority, &billing_scope).is_err() { return; }
    run_generation_record(backend, authority, Some(billing_scope), session_scope, record, sender, cancellations);
}
fn run_generation_record(
    backend: Arc<BackendRuntime>, authority: Arc<NamespaceStorageAuthority>, billing_scope: Option<BillingScope>,
    session_scope: SessionScope, record: PendingGenerationRecord,
    sender: mpsc::Sender<GenerationOutcome>, cancellations: Arc<Mutex<BTreeSet<String>>>,
) {
    let _=run_generation_record_checked(backend,authority,billing_scope,session_scope,record,sender,cancellations,None);
}

fn run_generation_record_checked(
    backend: Arc<BackendRuntime>, authority: Arc<NamespaceStorageAuthority>, billing_scope: Option<BillingScope>,
    session_scope: SessionScope, mut record: PendingGenerationRecord,
    sender: mpsc::Sender<GenerationOutcome>, cancellations: Arc<Mutex<BTreeSet<String>>>,
    cooperative_cancel:Option<Arc<std::sync::atomic::AtomicBool>>,
) -> std::result::Result<(),DeliveryRetryError> {
    let cancelled=||cooperative_cancel.as_ref().is_some_and(|cancel|cancel.load(Ordering::Acquire));
    if cancelled(){return Ok(());}
    let Ok(_activity) = backend.api.begin_user_work(&session_scope) else { return Ok(()); };
    if billing_scope.as_ref().is_some_and(|scope| record.billing_account_group_id != scope.request.account_group_id)
        || record.owner_user_id != session_scope.owner_user_id
        || record.auth_epoch != session_scope.auth_epoch
        || !backend_generation_scope_active(&backend, &session_scope)
    {
        return Ok(());
    }
    let api = GenerationApi::new(backend.api.clone()).with_saved_group(&record.billing_account_group_id);
    let mut uploaded = record.uploaded_file_ids.clone();
    if record.server_task_id.is_empty() && !generation_references_match_for_namespace(&authority, &record) {
        let _ = sender.send(GenerationOutcome::Failure {
            reason: "参考图内容已变化，恢复任务已暂停，请重新发起".to_string(),
            time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
        });
        return Ok(());
    }
    if record.task_type == "image_edit" && !record.server_task_id.is_empty() {
        if !matches!(
            apply_generation_patch_for_namespace(
                &authority,
                &record.identity(),
                GenerationRecoveryPatch::ReleaseReferenceInputs
            ),
            Ok(true)
        ) {
            return Ok(());
        }
        cleanup_image_edit_record_inputs(&record);
        record.reference_paths.clear();
        record.reference_sha256.clear();
        record.reference_size_bytes.clear();
    }
    if record.task_type == "image_upscale"
        && (!record.server_task_id.is_empty()
            || (!record.reference_paths.is_empty()
                && uploaded.len() >= record.reference_paths.len()))
        && !release_recovered_upscale_inputs_for_namespace(&mut record, &authority)
    {
        return Ok(());
    }
    for (index,path) in record.reference_paths.iter().enumerate().skip(uploaded.len()) {
        if cancelled(){return Ok(());}
        let hash=record.reference_sha256.get(index).ok_or_else(||anyhow!("original reference fingerprint missing"))?;
        let size=*record.reference_size_bytes.get(index).ok_or_else(||anyhow!("original reference size missing"))?;
        let uploaded_reference=api.upload_reference_for_namespace_checked(Path::new(path),&authority,&session_scope,
            record.task_type=="image_edit",hash,size);
        match uploaded_reference {
            Ok(file_id) => {
                uploaded.push(file_id);
                let snapshot = uploaded.clone();
                if !matches!(
                    apply_generation_patch_for_namespace(
                        &authority,
                        &record.identity(),
                        GenerationRecoveryPatch::UploadedFileIds(snapshot)
                    ),
                    Ok(true)
                ) {
                    if let Some(file_id) = uploaded.last() {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    return Ok(());
                }
                if generation_cancel_requested(&cancellations, &record.client_request_id) {
                    if cleanup_cancelled_generation_checked(
                        &backend,
                        &authority,
                        &api,
                        &session_scope,
                        &record.client_request_id,
                        &uploaded,
                        None,
                        &cancellations,
                    )? {
                        cleanup_generation_record_inputs(&record);
                    }
                    return Ok(());
                }
            }
            Err(error) => {
                if error.is_terminal_session_error(){return Err(error.into());}
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return Ok(());
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复参考图上传失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return Ok(());
            }
        }
    }
    if record.task_type == "image_upscale"
        && !record.reference_paths.is_empty()
        && uploaded.len() >= record.reference_paths.len()
        && !release_recovered_upscale_inputs_for_namespace(&mut record, &authority)
    {
        return Ok(());
    }
    let task_type = if record.task_type.trim().is_empty() {
        "image_generation"
    } else {
        record.task_type.as_str()
    };
    let aspect_ratio = api_aspect_ratio(&record.ratio);
    if generation_cancel_requested(&cancellations, &record.client_request_id) {
        if cleanup_cancelled_generation_checked(
            &backend,
            &authority,
            &api,
            &session_scope,
            &record.client_request_id,
            &uploaded,
            None,
            &cancellations,
        )? {
            cleanup_generation_record_inputs(&record);
        }
        return Ok(());
    }
    if cancelled(){return Ok(());}
    let mut detail = if record.server_task_id.is_empty() {
        let Some(billing_scope) = billing_scope.as_ref() else { return Ok(()); };
        if begin_generation_submission(&authority, &record.client_request_id).is_err() { return Ok(()); }
        let created = if task_type == "image_to_video" {
            api.create_video_task_billing(record.video_request.as_ref().ok_or_else(|| anyhow!("video request missing"))?, billing_scope)
        } else if task_type == "image_upscale" {
            let request = CreateUpscaleGenerationTask {
                client_request_id: record.client_request_id.clone(),
                task_type: "image_upscale".to_string(),
                model_code: record.model_code.clone(),
                prompt: record.generation_prompt.clone(),
                quality: record.quality.clone(),
                reference_file_ids: uploaded.clone(),
                target_width: record.target_width,
                target_height: record.target_height,
            };
            api.create_upscale_task_billing(&request, &billing_scope)
        } else if task_type == "image_edit" {
            if uploaded.len() != 2 {
                if !matches!(
                    remove_pending_generation_for_namespace(&authority, &record.identity()),
                    Ok(true)
                ) {
                    return Ok(());
                }
                for file_id in &uploaded {
                    let _ = api.delete_reference_scoped(file_id, &session_scope);
                }
                cleanup_image_edit_record_inputs(&record);
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: "图片编辑恢复数据不完整：缺少原图或遮罩".to_string(),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return Ok(());
            }
            let request = CreateImageEditTask {
                client_request_id: record.client_request_id.clone(),
                task_type: "image_edit".to_string(),
                model_code: record.model_code.clone(),
                prompt: record.generation_prompt.clone(),
                quality: record.quality.clone(),
                aspect_ratio: aspect_ratio.clone(),
                source_file_id: uploaded[0].clone(),
                mask_file_id: uploaded[1].clone(),
            };
            api.create_image_edit_task_billing(&request, &billing_scope)
        } else {
            let request = CreateGenerationTask {
                client_request_id: record.client_request_id.clone(),
                task_type: "image_generation".to_string(),
                model_code: record.model_code.clone(),
                prompt: record.generation_prompt.clone(),
                quality: Some(record.quality.clone()),
                count: Some(record.count),
                aspect_ratio: Some(aspect_ratio),
                reference_file_ids: Some(uploaded.clone()),
                target_language: None,
            };
            api.create_task_billing(&request, &billing_scope)
        };
        match created {
            Ok(detail) => detail,
            Err(error) => {
                if error.is_terminal_session_error(){return Err(error.into());}
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return Ok(());
                }
                if error.is_billing_rejection() {
                    if !matches!(
                        remove_pending_generation_for_namespace(&authority, &record.identity()),
                        Ok(true)
                    ) {
                        return Ok(());
                    }
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    cleanup_generation_record_inputs(&record);
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: error,
                    });
                    return Ok(());
                }
                if !error.should_preserve_generation_recovery() {
                    if !matches!(
                        remove_pending_generation_for_namespace(&authority, &record.identity()),
                        Ok(true)
                    ) {
                        return Ok(());
                    }
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    cleanup_generation_record_inputs(&record);
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复任务提交失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return Ok(());
            }
        }
    } else {
        match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if error.is_terminal_session_error(){return Err(error.into());}
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return Ok(());
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复任务查询失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return Ok(());
            }
        }
    };
    if generation_cancel_requested(&cancellations, &record.client_request_id) {
        if cleanup_cancelled_generation_checked(
            &backend,
            &authority,
            &api,
            &session_scope,
            &record.client_request_id,
            &uploaded,
            Some(&detail.id),
            &cancellations,
        )? {
            cleanup_generation_record_inputs(&record);
        }
        return Ok(());
    }
    // Check the original identity before Accepted can update the retained row.
    if !record.server_task_id.is_empty() && record.server_task_id != detail.id {
        return Err(anyhow!("recovered delivery task identity mismatch").into());
    }
    require_saved_group(&record.billing_account_group_id, &detail.billing_account_group_id)?;
    record.server_task_id = detail.id.clone();
    let server_task_id = detail.id.clone();
    let uploaded_snapshot = uploaded.clone();
    let server_id_snapshot = server_task_id.clone();
    if !matches!(
        apply_generation_patch_for_namespace(
            &authority,
            &record.identity(),
            GenerationRecoveryPatch::Accepted {
                server_task_id: server_id_snapshot,
                uploaded_file_ids: uploaded_snapshot,
                clear_reference_inputs: record.task_type == "image_edit"
            }
        ),
        Ok(true)
    ) {
        return Ok(());
    }
    cleanup_generation_record_inputs(&record);
    record.reference_paths.clear();
    record.reference_sha256.clear();
    record.reference_size_bytes.clear();
    let _ = sender.send(GenerationOutcome::Accepted {
        task_id: server_task_id.clone(),
    });

    // A local path alone still needs the complete commit/acknowledgment path.
    // Only an exact acknowledged result is already settled; its remote file
    // may have been deleted as a normal consequence of that acknowledgment.
    let mut handled_success: BTreeSet<usize> = record.deliveries.iter()
        .filter(|delivery| delivery.acknowledged && !delivery.abandoned)
        .filter(|delivery| record.deliveries.iter().filter(|other|
            other.item_index == delivery.item_index || other.file_id == delivery.file_id).count() == 1)
        .filter_map(|delivery| {
            let mut items = detail.items.iter().filter(|item| item.index == delivery.item_index);
            let item = items.next()?;
            if items.next().is_some() || item.status != "succeeded" || item.index >= record.count.max(0) as usize {
                return None;
            }
            let file = item.file.as_ref()?;
            (file.id == delivery.file_id && file.sha256 == delivery.sha256
                && file.size_bytes == delivery.size_bytes.to_string())
                .then_some(item.index)
        }).collect();
    let mut handled_failure = BTreeSet::new();

    loop {
        if cancelled(){return Ok(());}
        if !backend_generation_scope_active(&backend, &session_scope) {
            return Ok(());
        }
        if generation_cancel_requested(&cancellations, &record.client_request_id) {
            cleanup_cancelled_generation_checked(
                &backend,
                &authority,
                &api,
                &session_scope,
                &record.client_request_id,
                &[],
                Some(&server_task_id),
                &cancellations,
            )?;
            return Ok(());
        }
        let _ = sender.send(GenerationOutcome::Progress {
            percent: detail.progress_percent,
        });
        for item in &detail.items {
            if cancelled(){return Ok(());}
            if item.status == "succeeded" && !handled_success.contains(&item.index) {
                if let Some(file) = item.file.as_ref() {
                    if record.video_request.is_some() {
                        let index = authority.delivery_index()?;
                        match prepare_namespace_video_delivery(&api, authority.clone(), index, &record.identity(), item.index) {
                            Ok(prepared) => { sender.send(GenerationOutcome::NamespaceVideoSuccess { prepared: Box::new(prepared), time: Local::now().format("%Y-%m-%d %H:%M").to_string() }).ok(); handled_success.insert(item.index); }
                            Err(error) => { if matches!(&error, DeliveryRetryError::Api(api) if api.is_terminal_session_error()) { return Err(error); } }
                        }
                        continue;
                    }
                    match prepare_runtime_image_delivery(&api, authority.clone(), &record.client_request_id, item.index) {
                        Ok(Some(prepared)) => {
                            if sender.send(GenerationOutcome::NamespaceImageSuccess { prepared: Box::new(prepared), time: Local::now().format("%Y-%m-%d %H:%M").to_string() }).is_err() { return Ok(()); }
                            handled_success.insert(item.index); continue;
                        }
                        Err(error) => {
                            if matches!(&error,DeliveryRetryError::Api(api) if api.is_terminal_session_error()) {return Err(error);}
                            if detail.terminal() && handled_failure.insert(item.index) { let _ = sender.send(GenerationOutcome::ImageFailure { reason: "交付暂未完成，原任务已保留".into(), time: Local::now().format("%Y-%m-%d %H:%M").to_string(), delivery: None }); }
                            continue;
                        }
                        Ok(None) => {}
                    }
                    let local_path = generation_download_staging_path(
                        &record.client_request_id,
                        item.index,
                        file,
                    );
                    match api.download_verified_to_path_scoped(file, &session_scope, &local_path) {
                        Ok(()) => {
                            handled_success.insert(item.index);
                            if sender
                                .send(GenerationOutcome::ImageSuccess {
                                    local_path: local_path.display().to_string(),
                                    display_prompt: record.raw_prompt.clone(),
                                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                    upscale_done: record.task_type == "image_upscale",
                                    delivery: delivery_confirmation_for_item(
                                        &record.client_request_id,
                                        &detail,
                                        item.index,
                                    )
                                    .map(|mut delivery| {
                                        delivery.failed_asset_id =
                                            failed_asset_id_for_delivery(&record, &file.id);
                                        delivery
                                    }),
                                })
                                .is_err()
                            {
                                let _ = fs::remove_file(local_path);
                                return Ok(());
                            }
                        }
                        Err(error) if error.is_terminal_session_error() => { return Err(error.into()); }
                        Err(error) if detail.terminal() => {
                            handled_failure.insert(item.index);
                            let existing_failed_asset_id =
                                failed_asset_id_for_delivery(&record, &file.id);
                            let (reason, delivery) = match failed_delivery_confirmation_for_item(
                                &session_scope,
                                &record.client_request_id,
                                &detail,
                                item.index,
                                existing_failed_asset_id.as_deref(),
                            ) {
                                Ok(delivery) => (error.generation_message(), Some(delivery)),
                                Err(_) => (
                                    "本地生成恢复记录无法安全更新，已暂停交付，请重启后重试"
                                        .to_string(),
                                    None,
                                ),
                            };
                            let _ = sender.send(GenerationOutcome::ImageFailure {
                                reason,
                                time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                delivery,
                            });
                        }
                        Err(_) => {}
                    }
                }
            } else if matches!(item.status.as_str(), "failed" | "cancelled")
                && handled_failure.insert(item.index)
            {
                let _ = sender.send(GenerationOutcome::ImageFailure {
                    reason: item
                        .failure
                        .as_ref()
                        .map(TaskFailure::generation_message)
                        .unwrap_or_else(|| "服务端未能生成该图片".to_string()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    delivery: None,
                });
            }
        }
        if detail.terminal() {
            report_unhandled_terminal_failures(
                &sender,
                &detail,
                record.count.max(1) as usize,
                &handled_success,
                &mut handled_failure,
                "服务端未能生成该图片",
            );
            let expected = detail.success_count.max(0) as usize;
            if !matches!(
                apply_generation_patch_for_namespace(
                    &authority,
                    &record.identity(),
                    GenerationRecoveryPatch::Terminal {
                        expected_success_count: expected
                    }
                ),
                Ok(true)
            ) {
                return Ok(());
            }
            let _ = sender.send(GenerationOutcome::Finished);
            return Ok(());
        }
        let next_poll=Instant::now()+Duration::from_millis(IMAGE_POLL_INTERVAL_MS);
        while Instant::now()<next_poll {
            if cancelled(){return Ok(());}
            std::thread::sleep(Duration::from_millis(25).min(next_poll.saturating_duration_since(Instant::now())));
        }
        detail = match api.task_scoped(&server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if error.is_terminal_session_error(){return Err(error.into());}
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return Ok(());
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复任务轮询失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return Ok(());
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upscale_prompt_uses_image_as_authority_without_creation_instructions() {
        let prompt = build_upscale_prompt("添加华丽边框与云纹背景", 3840, 2160, 2, "4K");
        assert!(!prompt.contains("添加华丽边框与云纹背景"));
        assert!(!prompt.contains("原始描述"));
        assert!(prompt.contains("3840x2160"));
        assert!(prompt.contains("禁止扩图"));
        assert!(prompt.contains("留白"));
    }

    #[test]
    fn retry_recovery_upsert_failure_keeps_the_old_delivery_recoverable() {
        let events = RefCell::new(Vec::new());
        let abandoned = RefCell::new(false);

        let result = commit_retry_generation_recovery_with(
            Some("failed-card"),
            Some("failed-card"),
            || {
                events.borrow_mut().push("upsert-new-recovery");
                Err(anyhow!("new recovery persistence failed"))
            },
            |_| {
                *abandoned.borrow_mut() = true;
                events.borrow_mut().push("abandon-old-delivery");
                Ok(true)
            },
            || {
                events.borrow_mut().push("rollback-new-recovery");
                Ok(true)
            },
            |_| events.borrow_mut().push("remove-old-card"),
        );

        assert!(result.is_err());
        assert!(!*abandoned.borrow());
        assert_eq!(events.into_inner(), vec!["upsert-new-recovery"]);
    }

    #[test]
    fn committed_retry_abandons_the_old_delivery_immediately_before_removing_the_card() {
        let events = RefCell::new(Vec::new());

        let result = commit_retry_generation_recovery_with(
            Some("failed-card"),
            Some("failed-card"),
            || {
                events.borrow_mut().push("upsert-new-recovery");
                Ok(())
            },
            |failed_asset_id| {
                assert_eq!(failed_asset_id, "failed-card");
                events.borrow_mut().push("abandon-old-delivery");
                Ok(true)
            },
            || {
                events.borrow_mut().push("rollback-new-recovery");
                Ok(true)
            },
            |failed_asset_id| {
                assert_eq!(failed_asset_id, "failed-card");
                events.borrow_mut().push("remove-old-card");
            },
        );

        assert!(result.is_ok());
        assert_eq!(
            events.into_inner(),
            vec![
                "upsert-new-recovery",
                "abandon-old-delivery",
                "remove-old-card",
            ]
        );
    }

    #[test]
    fn retry_recovery_abandonment_failure_rolls_back_new_recovery_and_keeps_old_card() {
        let events = RefCell::new(Vec::new());
        let new_recovery_persisted = std::cell::Cell::new(false);
        let old_card_removed = std::cell::Cell::new(false);

        let result = commit_retry_generation_recovery_with(
            Some("failed-card"),
            Some("failed-card"),
            || {
                events.borrow_mut().push("persist-new-recovery");
                new_recovery_persisted.set(true);
                Ok(())
            },
            |failed_asset_id| {
                assert_eq!(failed_asset_id, "failed-card");
                events.borrow_mut().push("abandon-old-delivery");
                Err(anyhow!("old delivery persistence failed"))
            },
            || {
                events.borrow_mut().push("rollback-new-recovery");
                new_recovery_persisted.set(false);
                Ok(true)
            },
            |_| {
                events.borrow_mut().push("remove-old-card");
                old_card_removed.set(true);
            },
        );

        assert_eq!(
            result,
            Err(RetryGenerationRecoveryCommitError::OldDelivery)
        );
        assert!(!new_recovery_persisted.get());
        assert!(!old_card_removed.get());
        assert_eq!(
            events.into_inner(),
            vec![
                "persist-new-recovery",
                "abandon-old-delivery",
                "rollback-new-recovery",
            ]
        );
    }

    #[test]
    fn retry_recovery_reports_when_rollback_persistence_also_fails() {
        let events = RefCell::new(Vec::new());
        let old_card_removed = std::cell::Cell::new(false);

        let result = commit_retry_generation_recovery_with(
            Some("failed-card"),
            Some("failed-card"),
            || {
                events.borrow_mut().push("persist-new-recovery");
                Ok(())
            },
            |_| {
                events.borrow_mut().push("abandon-old-delivery");
                Ok(false)
            },
            || {
                events.borrow_mut().push("rollback-new-recovery");
                Err(anyhow!("rollback persistence failed"))
            },
            |_| {
                events.borrow_mut().push("remove-old-card");
                old_card_removed.set(true);
            },
        );

        assert_eq!(
            result,
            Err(RetryGenerationRecoveryCommitError::NewRecoveryRollback)
        );
        assert!(!old_card_removed.get());
        assert_eq!(
            events.into_inner(),
            vec![
                "persist-new-recovery",
                "abandon-old-delivery",
                "rollback-new-recovery",
            ]
        );
    }

    fn failed_generation_task(code: &str, message: &str) -> GenerationTaskDetail {
        GenerationTaskDetail {
            id: "failed-task".to_string(),
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".to_string(),
            status: "failed".to_string(),
            progress_percent: 100,
            success_count: 0,
            failure_count: 2,
            failure: Some(TaskFailure {
                code: code.to_string(),
                message: message.to_string(),
            }),
            prompt: None,
            result_prompt: None,
            request: serde_json::Value::Null,
            model: None,
            quality: "1K".to_string(),
            requested_count: 2,
            task_type: "image_generation".to_string(),
            items: Vec::new(),
        }
    }

    fn completed_task_with_available_file(file_id: &str) -> GenerationTaskDetail {
        GenerationTaskDetail {
            id: "completed-task".to_string(),
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".to_string(),
            status: "completed".to_string(),
            progress_percent: 100,
            success_count: 1,
            failure_count: 0,
            failure: None,
            prompt: None,
            result_prompt: None,
            request: serde_json::Value::Null,
            model: None,
            quality: "1K".to_string(),
            requested_count: 1,
            task_type: "image_generation".to_string(),
            items: vec![GenerationTaskItem {
                index: 0,
                status: "succeeded".to_string(),
                credit_cost: "0".to_string(),
                failure: None,
                file: Some(TaskOutputFile {
                    id: file_id.to_string(),
                    status: "available".to_string(),
                    mime_type: "image/png".to_string(),
                    size_bytes: "3".to_string(),
                    sha256: "abc".to_string(),
                    width: Some(1),
                    height: Some(1),
                    download_url: Some("https://example.invalid/file.png".to_string()),
                }),
            }],
        }
    }

    #[test]
    fn succeeded_item_download_error_keeps_delivery_identity() {
        let detail = completed_task_with_available_file("file-1");
        let delivery = delivery_confirmation_for_item("request-1", &detail, 0).unwrap();

        assert_eq!(delivery.file_id, "file-1");
        assert_eq!(delivery.item_index, 0);
    }

    #[test]
    fn failed_provider_item_has_no_recoverable_delivery() {
        let detail = failed_generation_task("provider_error", "failed");

        assert!(delivery_confirmation_for_item("request-1", &detail, 0).is_none());
    }

    #[test]
    fn task_level_policy_block_reports_every_missing_image() {
        let (sender, receiver) = mpsc::channel();
        let detail = failed_generation_task(
            "content_policy_violation",
            "生成内容违反了关于裸露内容的防护规则",
        );
        let mut handled_failure = BTreeSet::new();

        report_unhandled_terminal_failures(
            &sender,
            &detail,
            2,
            &BTreeSet::new(),
            &mut handled_failure,
            "服务端未能生成该图片",
        );

        let outcomes = receiver.try_iter().collect::<Vec<_>>();
        assert_eq!(outcomes.len(), 2);
        for outcome in outcomes {
            let GenerationOutcome::ImageFailure { reason, .. } = outcome else {
                panic!("expected an image failure");
            };
            assert!(reason.contains("上游安全系统拦截"));
            assert!(!reason.contains("不返还积分"));
            assert!(reason.contains("积分记录"));
        }
    }

    fn test_png_bytes() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([12, 34, 56, 255]));
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut output, image::ImageFormat::Png)
            .expect("encode test png");
        output.into_inner()
    }

    fn delivery(file_id: &str, path: &Path, bytes: &[u8]) -> PendingDeliveryRecord {
        PendingDeliveryRecord {
            item_index: 0,
            file_id: file_id.to_string(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size_bytes: bytes.len() as u64,
            local_path: path.display().to_string(),
            acknowledged: false,
            failed_asset_id: String::new(),
            abandoned: false,
        }
    }

    fn recovery_record(deliveries: Vec<PendingDeliveryRecord>) -> PendingGenerationRecord {
        PendingGenerationRecord {
            source_asset_id: String::new(),            video_request: None,
            schema_version: 2,
            cancel_requested: false,
            created_at_epoch_ms: Local::now().timestamp_millis(),
            client_request_id: "delivery_test_request".to_string(),
            owner_user_id: "delivery-test-user".to_string(),
            billing_account_group_id: "22222222-2222-4222-8222-222222222222".to_owned(),
            auth_epoch: 7,
            local_task_id: "local-task".to_string(),
            server_task_id: "server-task".to_string(),
            raw_prompt: "prompt".to_string(),
            generation_prompt: "prompt".to_string(),
            task_type: "image_generation".to_string(),
            category: "other".to_string(),
            mode: "game".to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model_code: "openai_image".to_string(),
            conversation_id: "conversation".to_string(),
            count: 1,
            target_width: 0,
            target_height: 0,
            create_conversation: false,
            reference_paths: vec![],
            reference_sha256: vec![],
            reference_size_bytes: vec![],
            lineage_reference_paths: vec![],
            uploaded_file_ids: vec![],
            deliveries,
            terminal: true,
            expected_success_count: 1,
            canvas_source_node_id: String::new(),
            canvas_ui_extraction: false,
        }
    }

    #[test]
    fn image_edit_cleanup_is_limited_to_managed_input_files() {
        let directory = managed_image_edit_input_dir();
        let managed = directory.join("20260810-source.png");
        let managed_unique = directory.join("20260810-mask-2.png");
        let wrong_parent = app_data_dir().join("20260810-source.png");
        let wrong_name = directory.join("user-image.png");

        assert!(is_managed_image_edit_input_path(&managed));
        assert!(is_managed_image_edit_input_path(&managed_unique));
        assert!(!is_managed_image_edit_input_path(&wrong_parent));
        assert!(!is_managed_image_edit_input_path(&wrong_name));
        assert!(!is_managed_image_edit_input_path(Path::new(
            "/tmp/image-edit-inputs/20260810-source.png"
        )));
    }

    #[test]
    fn orphan_cleanup_retains_pending_inputs_and_ignores_unmanaged_names() {
        let directory = std::env::temp_dir().join(format!(
            "artforge-generation-input-cleanup-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).expect("create cleanup directory");
        let retained = directory.join("20260810-source.png");
        let orphan = directory.join("20260810-mask.png");
        let unmanaged = directory.join("user-image.png");
        fs::write(&retained, b"retained").expect("write retained input");
        fs::write(&orphan, b"orphan").expect("write orphan input");
        fs::write(&unmanaged, b"user").expect("write unmanaged input");

        cleanup_orphaned_input_directory(
            &directory,
            &BTreeSet::from([cleanup_path_identity(&retained)]),
            std::time::SystemTime::now() + ORPHANED_GENERATION_INPUT_GRACE,
            is_image_edit_input_name,
        );

        assert!(retained.is_file());
        assert!(!orphan.exists());
        assert!(unmanaged.is_file());
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn orphan_cleanup_rejects_a_symlinked_input_directory() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "artforge-generation-input-symlink-{}",
            Uuid::new_v4()
        ));
        let target = root.join("user-works");
        let linked = root.join("image-edit-inputs");
        fs::create_dir_all(&target).expect("create target directory");
        let work = target.join("20260810-source.png");
        fs::write(&work, b"user work").expect("write user work");
        symlink(&target, &linked).expect("create input directory symlink");

        cleanup_orphaned_input_directory(
            &linked,
            &BTreeSet::new(),
            std::time::SystemTime::now() + ORPHANED_GENERATION_INPUT_GRACE,
            is_image_edit_input_name,
        );

        assert!(work.is_file());
        fs::remove_file(&linked).expect("remove input directory symlink");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn retained_cleanup_paths_include_every_account_and_delivery() {
        let mut first = recovery_record(Vec::new());
        first.owner_user_id = "user-a".to_string();
        first.task_type = "image_edit".to_string();
        first.reference_paths = vec!["/managed/edit-source.png".to_string()];
        let mut second = recovery_record(vec![PendingDeliveryRecord {
            local_path: "/managed/delivery.png".to_string(),
            ..PendingDeliveryRecord::default()
        }]);
        second.owner_user_id = "user-b".to_string();
        second.client_request_id = "other-request".to_string();
        second.task_type = "image_upscale".to_string();
        second.reference_paths = vec!["/managed/upscale-source.png".to_string()];

        let retained = retained_generation_paths(&[first, second]);

        assert!(retained.contains(Path::new("/managed/edit-source.png")));
        assert!(retained.contains(Path::new("/managed/upscale-source.png")));
        assert!(retained.contains(Path::new("/managed/delivery.png")));
    }

    #[test]
    fn recovered_delivery_requires_matching_size_sha256_and_decodable_image() {
        let directory =
            std::env::temp_dir().join(format!("artforge-delivery-validation-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create delivery validation directory");
        let valid_path = directory.join("valid.png");
        let invalid_path = directory.join("invalid.png");
        let valid_bytes = test_png_bytes();
        let invalid_bytes = b"not an encoded image";
        fs::write(&valid_path, &valid_bytes).expect("write valid delivery");
        fs::write(&invalid_path, invalid_bytes).expect("write invalid delivery");
        let valid_sha256 = format!("{:x}", Sha256::digest(&valid_bytes));
        let invalid_sha256 = format!("{:x}", Sha256::digest(invalid_bytes));

        assert!(recovered_delivery_path_matches(
            valid_path.to_str().expect("valid path"),
            &valid_sha256,
            valid_bytes.len() as u64,
        ));
        assert!(!recovered_delivery_path_matches(
            valid_path.to_str().expect("valid path"),
            &valid_sha256,
            valid_bytes.len() as u64 + 1,
        ));
        assert!(!recovered_delivery_path_matches(
            valid_path.to_str().expect("valid path"),
            &"0".repeat(64),
            valid_bytes.len() as u64,
        ));
        assert!(!recovered_delivery_path_matches(
            invalid_path.to_str().expect("invalid path"),
            &invalid_sha256,
            invalid_bytes.len() as u64,
        ));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn invalid_recovered_delivery_is_cleared_for_redownload_before_ack() {
        let directory =
            std::env::temp_dir().join(format!("artforge-delivery-redownload-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create delivery redownload directory");
        let valid_path = directory.join("valid.png");
        let corrupted_path = directory.join("corrupted.png");
        let valid_bytes = test_png_bytes();
        fs::write(&valid_path, &valid_bytes).expect("write valid delivery");
        fs::write(&corrupted_path, b"truncated").expect("write corrupted delivery");
        let valid = delivery("valid-file", &valid_path, &valid_bytes);
        let invalid = delivery("invalid-file", &corrupted_path, &valid_bytes);
        let mut record = recovery_record(vec![valid, invalid]);

        let verified = sanitize_recovered_delivery_paths_with(&mut record, |file_ids| {
            assert_eq!(file_ids, &BTreeSet::from(["invalid-file".to_string()]));
            Ok(true)
        })
        .expect("sanitize recovered deliveries");

        assert_eq!(verified, BTreeSet::from(["valid-file".to_string()]));
        assert!(!record.deliveries[0].local_path.is_empty());
        assert!(recovered_delivery_file_matches(&record.deliveries[0]));
        assert!(recovered_delivery_ready_for_ack(
            &record.deliveries[0],
            &verified
        ));
        assert!(record.deliveries[1].local_path.is_empty());
        assert!(!recovered_delivery_file_matches(&record.deliveries[1]));
        assert!(!recovered_delivery_ready_for_ack(
            &record.deliveries[1],
            &verified
        ));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn recovered_delivery_fails_closed_when_invalid_path_cannot_be_persisted() {
        let directory =
            std::env::temp_dir().join(format!("artforge-delivery-fail-closed-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create fail-closed directory");
        let corrupted_path = directory.join("corrupted.png");
        let expected_bytes = test_png_bytes();
        fs::write(&corrupted_path, b"truncated").expect("write corrupted delivery");
        let mut record = recovery_record(vec![delivery(
            "invalid-file",
            &corrupted_path,
            &expected_bytes,
        )]);
        let original_path = record.deliveries[0].local_path.clone();

        let result = sanitize_recovered_delivery_paths_with(&mut record, |_| Ok(false));

        assert!(result.is_err());
        assert_eq!(record.deliveries[0].local_path, original_path);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn every_toolbox_recovery_path_uses_shared_delivery_validation() {
        // Structural wiring guard, not execution of HTTP, held-file validation or SQLite.
        fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
            source.split_once(start).expect("production entry exists").1
                .split_once(end).expect("next production boundary exists").0
        }
        let cutout = include_str!("../callbacks/image_cutout.rs");
        let enhancement = include_str!("../callbacks/image_enhancement.rs");
        let toolbox = include_str!("../callbacks/toolbox.rs");
        let shared = include_str!("delivery_retry.rs");

        for (source, resume, next, worker, finish, prepare, end) in [
            (cutout, "pub(super) fn resume_pending_image_cutout(", "fn finish_cutout_work(",
             "run_cutout_record(", "finish_cutout_work(", "prepare_namespace_cutout_delivery(",
             "pub(super) fn decode_cutout_result_bytes("),
            (enhancement, "pub(super) fn resume_pending_image_enhancement(", "fn finish_enhancement_work(",
             "run_enhancement_record(", "finish_enhancement_work(", "prepare_namespace_delivery(",
             "#[cfg(test)]"),
        ] {
            let entry = section(source, resume, next);
            assert!(entry.contains("persistence.storage_authority()?"));
            assert!(entry.contains(worker) && entry.contains("None,&session,record,cancel,progress"));
            assert!(entry.contains(finish));
            let work = section(source, &format!("fn {worker}"), end);
            assert!(work.contains("record.identity()==expected.identity()"));
            assert!(work.contains("with_saved_group(&record.billing_account_group_id)"));
            assert!(work.contains(&format!("return {prepare}&api,authority.clone(),authority.delivery_index()?,&record.identity(),item.index)")));
        }

        for (resume, launch, worker, poll, next) in [
            ("resume_pending_watermark_removal", "launch_watermark_removal_with_billing_scope",
             "run_watermark_worker", "poll_watermark_outcomes", "fn start_image_colorization("),
            ("resume_pending_image_colorization", "launch_image_colorization_with_billing_scope",
             "run_image_colorization_worker", "poll_image_colorization_outcomes", "#[cfg(test)]"),
        ] {
            let entry = section(toolbox, &format!("pub(super) fn {resume}("), &format!("fn {launch}("));
            assert!(entry.contains("context.storage_authority_for(&lease)"));
            assert!(entry.contains(&format!("{launch}(")));
            assert!(entry.contains("authority,\n        None,\n        record,\n        true,"));
            let launched = section(toolbox, &format!("fn {launch}("), &format!("fn {worker}("));
            assert!(launched.contains(&format!("{worker}(")) && launched.contains(&format!("{poll}(")));
            let work = section(toolbox, &format!("fn {worker}("), &format!("fn {poll}("));
            let terminal = section(work, "if record.terminal {", "let mut uploaded =");
            let succeeded = section(work, "if let Some(item) = detail.items.iter().find(|item| item.status == \"succeeded\") {", "if detail.terminal() {");
            for branch in [terminal, succeeded] {
                assert!(branch.contains("authority\n                .delivery_index()")
                    || branch.contains("authority\n            .delivery_index()"));
                assert!(branch.contains("prepare_namespace_delivery("));
                assert!(branch.contains("&record.identity(),"));
                assert!(branch.find("prepare_namespace_delivery(").unwrap()
                    < branch.find("ToolboxRemoteOutcome::Prepared(Box::new(prepared))").unwrap());
            }
            let completion = section(toolbox, &format!("fn {poll}("), next);
            assert!(completion.find("rx.finish_message(outcome)").unwrap()
                < completion.find("ToolboxRemoteOutcome::Prepared(prepared) =>").unwrap());
            assert!(completion.contains("enqueue_toolbox_remote_delivery("));
        }

        // The common implementation, not merely its name: retain exact task/payer,
        // clear only an acknowledged stale display path, and verify existing/new bytes.
        let image = section(shared, "pub(super) fn prepare_namespace_delivery(", "pub(super) fn prepare_namespace_video_delivery(");
        assert!(image.contains("prepare_namespace_delivery_proof(api,authority,index,expected,item_index,NamespaceDeliveryKind::Image)?"));
        let proof = section(shared, "fn prepare_namespace_delivery_proof(", "pub(super) fn acknowledge_namespace_delivery(");
        for required in [
            "load_exact_delivery_record(&authority, expected)?",
            "detail.id == record.server_task_id",
            "&record.billing_account_group_id,\n        &detail.billing_account_group_id",
            "GenerationRecoveryPatch::ClearDeliveryLocalPaths(ids))?",
            "authority.open_optional_regular(&destination)?",
            "verify_namespace_delivery_file(&authority, &mut file, &confirmation)?",
            "api.download_verified_for_namespace(remote, &scope, &authority, &mut temporary)?",
            "index.reconcile_verified_delivery_content(&proof)",
        ] { assert!(proof.contains(required), "missing proof step: {required}"); }
        let verify = section(shared, "fn verify_namespace_delivery_file(", "pub(super) struct AcknowledgedCutoutDelivery");
        assert!(verify.contains("authority.read_regular_to(file, &mut sink)?"));
        assert!(verify.contains("sink.count == confirmation.size_bytes"));
        assert!(verify.contains("eq_ignore_ascii_case(&confirmation.sha256)"));
        let derived = section(shared, "pub(super) fn prepare_namespace_cutout_delivery(", "#[derive(Debug, thiserror::Error)]");
        for required in [
            "expected, item_index, NamespaceDeliveryKind::Cutout)?",
            "record.reference_size_bytes[0], &record.reference_sha256[0])?",
            "proof.confirmation.size_bytes, &proof.confirmation.sha256)?",
            "sha2::Sha256::digest(&derived_bytes)",
            "reconcile_verified_delivery_content(&verified)",
            "proof.derived_cutout = Some(DerivedCutoutDelivery",
        ] { assert!(derived.contains(required), "missing derived proof step: {required}"); }
    }
}

/// Captures one admitted payer lease before file preparation or worker creation.
/// Current selection is deliberately absent from this boundary.
pub(super) fn capture_billing_scope_for_submission(
    backend: Option<&BackendRuntime>,
    authority: &NamespaceStorageAuthority,
    billing_scope: &BillingScope,
) -> std::result::Result<BillingScope, ApiError> {
    // A pre-captured identity is not permission to create a new retained intent
    // once process-wide ordinary admission has closed. Reject before input I/O.
    if let Some(required) = backend.and_then(|backend| backend.api.upgrade_latch().snapshot()) {
        return Err(required.as_error());
    }
    let session = &billing_scope.request.session;
    let canonical_owner =
        api::uuid_path_segment(&session.owner_user_id).is_ok_and(|id| id == session.owner_user_id);
    let canonical_payer = api::uuid_path_segment(&billing_scope.request.account_group_id)
        .is_ok_and(|id| id == billing_scope.request.account_group_id);
    if !canonical_owner
        || !canonical_payer
        || authority.user_public_id() != session.owner_user_id
        || authority.lease().auth_epoch != session.auth_epoch
        || !backend.is_some_and(|backend| backend.api.session().is_scope_current(session))
    {
        return Err(ApiError::LocalState {
            message: "登录状态已变化，请重新发起任务".to_owned(),
        });
    }
    Ok(billing_scope.clone())
}

#[cfg(test)]
pub(in crate::runtime) mod billing_capture_test_support {
    use super::*;
    use crate::runtime::test_support::MemoryRefreshTokenStore;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    pub(in crate::runtime) const OWNER: &str = "11111111-1111-4111-8111-111111111111";
    pub(in crate::runtime) const PAYER: &str = "22222222-2222-4222-8222-222222222222";
    pub(in crate::runtime) const OTHER: &str = "33333333-3333-4333-8333-333333333333";
    pub(in crate::runtime) struct Fixture {
        pub(in crate::runtime) authority: Arc<NamespaceStorageAuthority>,
        pub(in crate::runtime) scope: BillingScope,
        pub(in crate::runtime) backend: Arc<BackendRuntime>,
        pub(in crate::runtime) context: AppContext,
        pub(in crate::runtime) root: tempfile::TempDir,
    }
    pub(in crate::runtime) fn fixture(base_url: &str) -> Fixture {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let session = Arc::new(SessionManager::new(Arc::new(
            MemoryRefreshTokenStore::default(),
        )));
        let session_scope = session
            .install_tokens_for_user(
                &TokenSet {
                    access_token: "capture-access".into(),
                    access_expires_in_seconds: 1800,
                    refresh_token: "capture-refresh".into(),
                    refresh_expires_at: "2099-01-01T00:00:00Z".into(),
                    token_type: "X-Token".into(),
                },
                OWNER,
            )
            .unwrap();
        let scope = BillingScope {
            request: GroupRequestScope {
                session: session_scope.clone(),
                account_group_id: PAYER.into(),
            },
            context_epoch: 17,
        };
        let root_capability = Arc::new(NamespaceFs::open_data_root(root.path()).unwrap());
        let lease = NamespaceLease {
            namespace: UserNamespace::new(root.path(), OWNER).unwrap(),
            auth_epoch: session_scope.auth_epoch,
            namespace_epoch: 1,
        };
        let authority =
            Arc::new(NamespaceStorageAuthority::open(root_capability.clone(), &lease).unwrap());
        let backend = Arc::new(BackendRuntime {
            api: ApiClient::new(
                ApiClientConfig {
                    base_url: reqwest::Url::parse(base_url).unwrap(),
                    app_version: "fixture".into(),
                    timeout: Duration::from_secs(2),
                },
                DeviceIdentity {
                    id: OTHER.into(),
                    name: "fixture".into(),
                    platform: "macos".into(),
                },
                session,
            )
            .unwrap(),
        });
        let context = AppContext {
            data_root_capability: Some(root_capability),
            backend: Some(backend.clone()),
            current_user_id: Arc::new(Mutex::new(Some(OWNER.into()))),
            account_snapshot_scope: Arc::new(Mutex::new(Some(session_scope))),
            ..Default::default()
        };
        context.user_activity.activate(lease.clone()).unwrap();
        *context.active_namespace.lock().unwrap() = Some(lease);
        backend.api.bind_user_work(UserWorkAdmission::new(context.active_namespace.clone(), context.user_activity.clone())).unwrap();
        Fixture {
            authority,
            scope,
            backend,
            context,
            root,
        }
    }
    pub(in crate::runtime) fn listener() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        (listener, url)
    }
    pub(in crate::runtime) struct Captured {
        pub(in crate::runtime) request: String,
        pub(in crate::runtime) document: Value,
    }
    pub(in crate::runtime) fn read_request(stream: &mut TcpStream) -> String {
        String::from_utf8(read_request_bytes(stream)).unwrap()
    }
    pub(in crate::runtime) fn read_request_bytes(stream: &mut TcpStream) -> Vec<u8> {
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0u8; 4096];
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0, "request ended before headers/body");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map(|length| length.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    return bytes;
                }
            }
        }
    }
    pub(in crate::runtime) fn capture(
        listener: TcpListener,
        authority: Arc<NamespaceStorageAuthority>,
        filename: &'static str,
    ) -> (mpsc::Sender<()>, std::thread::JoinHandle<Captured>) {
        capture_response(
            listener,
            authority,
            filename,
            "400 Bad Request",
            r#"{"request_id":"capture-rejected","data":null,"error":{"code":"invalid_parameter","message":"fixture-stop","details":null},"meta":null}"#,
        )
    }
    pub(in crate::runtime) fn capture_response(
        listener: TcpListener,
        authority: Arc<NamespaceStorageAuthority>,
        filename: &'static str,
        status: &'static str,
        body: &'static str,
    ) -> (mpsc::Sender<()>, std::thread::JoinHandle<Captured>) {
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => {
                        panic!("billable dispatch did not reach recording transport: {error}")
                    }
                }
            };
            let request = read_request(&mut stream);
            let key = ManagedFileKey::new(ManagedUserArea::Recovery, filename).unwrap();
            let mut file = authority.open_existing_regular(&key).unwrap();
            let mut bytes = Vec::new();
            authority.read_regular_to(&mut file, &mut bytes).unwrap();
            let document = serde_json::from_slice(&bytes).unwrap();
            write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            Captured { request, document }
        });
        (release_tx, worker)
    }
    pub(in crate::runtime) fn assert_capture(captured: &Captured, vector: &str) {
        let headers = captured
            .request
            .split("\r\n\r\n")
            .next()
            .unwrap()
            .to_lowercase();
        assert!(headers.contains(&format!("x-account-group-id: {PAYER}")));
        assert!(headers.contains("x-token: capture-access"));
        let body: Value =
            serde_json::from_str(captured.request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let row = &captured.document[vector][0];
        assert_eq!(captured.document["schema_version"], 2);
        assert_eq!(row["schema_version"], 2);
        assert_eq!(row["owner_user_id"], OWNER);
        assert_eq!(row["billing_account_group_id"], PAYER);
        assert_eq!(row["client_request_id"], body["client_request_id"]);
        assert!(!row["client_request_id"].as_str().unwrap().is_empty());
    }
    pub(in crate::runtime) fn corrupt(authority: &NamespaceStorageAuthority, filename: &str) {
        let key = ManagedFileKey::new(ManagedUserArea::Recovery, filename).unwrap();
        let mut file = authority.create_new_regular(&key).unwrap();
        authority
            .write_new_regular_from(&mut file, &mut &b"invalid-owned-fixture"[..])
            .unwrap();
        authority.sync_regular(&mut file).unwrap();
    }
    pub(in crate::runtime) fn assert_no_request(listener: &TcpListener) {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_millis(150);
        while Instant::now() < deadline {
            assert!(
                matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock),
                "unexpected dispatch"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    pub(in crate::runtime) fn generation_record(
        scope: &BillingScope,
        task_type: &str,
    ) -> PendingGenerationRecord {
        PendingGenerationRecord {
            source_asset_id: String::new(),            video_request: None,
            schema_version: 2,
            cancel_requested: false,
            created_at_epoch_ms: 1,
            client_request_id: "0123456789abcdef0123456789abcdef".into(),
            owner_user_id: scope.request.session.owner_user_id.clone(),
            billing_account_group_id: scope.request.account_group_id.clone(),
            auth_epoch: scope.request.session.auth_epoch,
            local_task_id: "local".into(),
            server_task_id: String::new(),
            raw_prompt: "fixture prompt".into(),
            generation_prompt: "fixture prompt".into(),
            task_type: task_type.into(),
            category: "other".into(),
            mode: "game".into(),
            ratio: "1:1".into(),
            quality: if task_type == "image_cutout" { "general" } else { "2K" }.into(),
            model_code: "fixture-model".into(),
            conversation_id: OTHER.into(),
            count: 1,
            target_width: 2048,
            target_height: 2048,
            create_conversation: false,
            reference_paths: Vec::new(),
            reference_sha256: Vec::new(),
            reference_size_bytes: Vec::new(),
            lineage_reference_paths: Vec::new(),
            uploaded_file_ids: if task_type == "image_edit" {
                vec![OTHER.into(), OWNER.into()]
            } else {
                vec![OTHER.into()]
            },
            deliveries: Vec::new(),
            terminal: false,
            expected_success_count: 0,
            canvas_source_node_id: String::new(),
            canvas_ui_extraction: false,
        }
    }
    pub(in crate::runtime) fn assert_generation_worker<T: Send + 'static>(
        task_type: &str,
        run: impl FnOnce(
                Arc<BackendRuntime>,
                Arc<NamespaceStorageAuthority>,
                BillingScope,
                SessionScope,
                PendingGenerationRecord,
                mpsc::Sender<T>,
            ) + Send
            + 'static,
    ) {
        let (listener, url) = listener();
        let mut fixture = fixture(&url);
        let captured_scope = capture_billing_scope_for_submission(
            Some(&fixture.backend),
            &fixture.authority,
            &fixture.scope,
        )
        .unwrap();
        let record = generation_record(&captured_scope, task_type);
        upsert_pending_generation_for_namespace(
            &fixture.authority,
            &captured_scope,
            record.clone(),
        )
        .unwrap();
        let (release, transport) = capture(
            listener,
            fixture.authority.clone(),
            "pending-generations.json",
        );
        fixture.scope.request.account_group_id = OTHER.into();
        fixture.scope.context_epoch += 1;
        let (sender, _receiver) = mpsc::channel();
        let backend = fixture.backend.clone();
        let authority = fixture.authority.clone();
        let session = captured_scope.request.session.clone();
        let worker = std::thread::spawn(move || {
            run(backend, authority, captured_scope, session, record, sender)
        });
        release.send(()).unwrap();
        let observed = transport.join().unwrap();
        assert_capture(&observed, "generations");
        worker.join().unwrap();
    }
}

#[cfg(test)]
mod billing_capture_tests {
    use super::billing_capture_test_support::*;
    use super::*;

    fn multi_image_recovery_outcomes(change: Option<&str>) -> (Vec<usize>, usize, Vec<String>, bool) {
        use std::io::Write;
        let (listener, url) = listener();
        let f = fixture(&url);
        let index = FileIndex::initialize(f.root.path().join("delivery-index.sqlite3")).unwrap();
        let authority = Arc::new(NamespaceStorageAuthority::open_active(
            f.context.data_root_capability.as_ref().unwrap().clone(), f.authority.lease(),
            f.backend.api.clone(), index).unwrap());
        let mut record = generation_record(&f.scope, "image_generation");
        record.server_task_id = OTHER.into();
        record.count = 2;
        record.uploaded_file_ids.clear();
        record.terminal = true;
        record.expected_success_count = 2;
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(1, 1).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let png = bytes.into_inner();
        let hash = format!("{:x}", Sha256::digest(&png));
        let size = png.len() as u64;
        let mut saved = PendingDeliveryRecord {
            item_index: 0, file_id: OWNER.into(), sha256: hash.clone(), size_bytes: size,
            local_path: "fixture/already-saved.png".into(), acknowledged: true,
            ..Default::default()
        };
        match change {
            Some("unacknowledged") => saved.acknowledged = false,
            Some("abandoned") => saved.abandoned = true,
            Some("file") => saved.file_id = PAYER.into(),
            Some("index") => saved.item_index = 1,
            Some("hash") => saved.sha256 = "0".repeat(64),
            Some("size") => saved.size_bytes += 1,
            Some("task" | "payer" | "duplicate" | "collision") => {},
            None => {},
            _ => unreachable!(),
        }
        record.deliveries = vec![saved.clone()];
        if matches!(change, Some("duplicate" | "collision")) {
            if change == Some("collision") { saved.file_id = OTHER.into(); }
            record.deliveries.push(saved);
        }
        let original = serde_json::to_value(&record).unwrap();
        upsert_pending_generation_for_namespace(&authority, &f.scope, record.clone()).unwrap();
        let output = |index, id, status| serde_json::json!({
            "index":index,"status":"succeeded","credit_cost":"1","failure":null,
            "file":{"id":id,"status":status,"mime_type":"image/png",
                "size_bytes":size.to_string(),"sha256":hash,"width":1,"height":1,
                "download_url":format!("{url}blob")}
        });
        let mut items = vec![output(0, OWNER, "deleted")];
        // Negative cases isolate the acknowledged image so another item's
        // failure cannot hide an incorrectly skipped identity mismatch.
        if change.is_none() { items.push(output(1, PAYER, "available")); }
        let mut detail = serde_json::json!({
            "id":OTHER,"billing_account_group_id":PAYER,"status":"completed",
            "progress_percent":100,"success_count":items.len(),"failure_count":0,
            "failure":null,"prompt":null,"result_prompt":null,"request":{},"model":null,
            "quality":"2K","requested_count":2,"type":"image_generation","items":items
        });
        if change == Some("task") { detail["id"] = OWNER.into(); }
        if change == Some("payer") { detail["billing_account_group_id"] = OWNER.into(); }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        let transport = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut requests = Vec::new();
            while !stopped.load(Ordering::SeqCst) && Instant::now() < deadline {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2)); continue;
                    },
                    Err(error) => panic!("fixture accept failed: {error}"),
                };
                let request = String::from_utf8(read_request_bytes(&mut stream)).unwrap();
                let body = if request.starts_with("GET /blob ") { png.clone() } else {
                    serde_json::to_vec(&serde_json::json!({"request_id":"fixture","data":detail,"error":null,"meta":null})).unwrap()
                };
                stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                stream.write_all(&body).unwrap();
                requests.push(request);
            }
            requests
        });
        let (sender, receiver) = mpsc::channel();
        let backend = f.backend.clone();
        let scope = f.scope.request.session.clone();
        let worker = std::thread::spawn(move || run_generation_record_checked(
            backend, authority, None, scope, record, sender,
            Arc::new(Mutex::new(BTreeSet::new())), None));
        let result = worker.join().unwrap();
        stop.store(true, Ordering::SeqCst);
        let requests = transport.join().unwrap();
        if matches!(change, Some("task" | "payer")) {
            let retained = load_pending_generations_for_namespace(&f.authority).unwrap();
            assert_eq!(retained.len(), 1);
            assert_eq!(serde_json::to_value(&retained[0]).unwrap(), original,
                "a foreign task or payer response cannot rewrite the retained identity");
        } else {
            assert!(result.is_ok(), "fixture worker error: {:?}", result.as_ref().err());
        }
        let mut worker_failed = result.is_err();
        let mut successes = Vec::new();
        let mut failures = 0;
        for outcome in receiver.try_iter() {
            match outcome {
                GenerationOutcome::NamespaceImageSuccess { prepared, .. } => successes.push(prepared.confirmation().item_index),
                GenerationOutcome::ImageFailure { reason, .. } => {
                    eprintln!("fixture image failure: {reason}");
                    failures += 1;
                },
                // The API rejects a foreign payer before returning task detail;
                // this existing error path reports failure through the channel.
                GenerationOutcome::Failure { .. } if change == Some("payer") => worker_failed = true,
                GenerationOutcome::Failure { reason, .. } => panic!("unexpected task failure: {reason}"),
                _ => {},
            }
        }
        (successes, failures, requests, worker_failed)
    }

    #[test]
    fn multi_image_recovery_skips_acknowledged_image_and_delivers_missing_item() {
        let (successes, failures, requests, worker_failed) = multi_image_recovery_outcomes(None);
        assert!(!worker_failed);
        assert_eq!(successes, vec![1], "only the missing image should be delivered again");
        assert_eq!(failures, 0, "an acknowledged image must not create another failed card");
        assert!(!requests.iter().any(|request| request.starts_with("POST ")));
        assert_eq!(requests.iter().filter(|request| request.starts_with("GET /blob ")).count(), 1);
    }

    #[test]
    fn multi_image_recovery_never_skips_unacknowledged_or_mismatched_delivery() {
        for change in ["unacknowledged", "abandoned", "file", "index", "hash", "size", "duplicate", "collision"] {
            let (successes, failures, requests, worker_failed) = multi_image_recovery_outcomes(Some(change));
            assert!(!worker_failed, "{change}");
            assert!(successes.is_empty(), "{change}");
            assert_eq!(failures, 1, "{change} cannot authorize skipping a server result");
            assert!(!requests.iter().any(|request| request.starts_with("POST ")));
        }
    }

    #[test]
    fn multi_image_recovery_rejects_foreign_task_or_payer_before_updating_identity() {
        for change in ["task", "payer"] {
            let (successes, failures, requests, worker_failed) = multi_image_recovery_outcomes(Some(change));
            assert!(worker_failed, "{change}");
            assert!(successes.is_empty(), "{change}");
            assert_eq!(failures, 0, "{change}");
            assert_eq!(requests.len(), 1, "{change}: reject after the original task GET");
            assert!(requests[0].starts_with(&format!("GET /v1/generation/tasks/{OTHER} ")));
        }
    }

    #[test]
    fn forced_upgrade_retains_actual_generation_recovery_and_immutable_payer() {
        for task_type in ["image_generation", "image_edit", "image_upscale"] {
            for accepted in [false, true] {
                let (listener, url) = listener();
                let remaining_requests = listener.try_clone().unwrap();
                let mut fixture = fixture(&url);
                let captured_scope = fixture.scope.clone();
                let mut record = generation_record(&captured_scope, task_type);
                if accepted {
                    record.server_task_id = OTHER.into();
                }
                upsert_pending_generation_for_namespace(
                    &fixture.authority, &captured_scope, record.clone(),
                ).unwrap();
                let before = upscale_document(&fixture.authority);
                let (release, transport) = capture_response(
                    listener, fixture.authority.clone(), "pending-generations.json",
                    "426 Upgrade Required",
                    r#"{"request_id":"private-upgrade-request","data":null,"error":{"code":"client_upgrade_required","message":"blocked by safety: private fixture","details":{"minimum_version":"9.9.9"}},"meta":null}"#,
                );
                // The visible selection can change after dispatch capture. The
                // rejected/replayed request still belongs to the saved payer.
                fixture.scope.request.account_group_id = OTHER.into();
                fixture.scope.context_epoch += 1;
                let (sender, receiver) = mpsc::channel();
                release.send(()).unwrap();
                run_generation_with_billing_scope(
                    fixture.backend.clone(), fixture.authority.clone(),
                    captured_scope.clone(), captured_scope.request.session.clone(),
                    record, sender, Arc::new(Mutex::new(BTreeSet::new())),
                );
                let observed = transport.join().unwrap();
                let headers = observed.request.split("\r\n\r\n").next().unwrap().to_lowercase();
                assert!(headers.contains("x-token: capture-access"));
                if accepted {
                    assert!(observed.request.starts_with(&format!("GET /v1/generation/tasks/{OTHER} ")));
                    assert!(!headers.contains("x-account-group-id:"));
                } else {
                    assert!(observed.request.starts_with("POST /v1/generation/tasks "));
                    assert_capture(&observed, "generations");
                }
                assert_eq!(upscale_document(&fixture.authority), before, "{task_type}/{accepted}");
                assert!(fixture.backend.api.session().is_scope_current(&captured_scope.request.session));
                assert!(fixture.backend.api.upgrade_latch().is_tripped());
                assert!(matches!(receiver.try_recv(),Err(mpsc::TryRecvError::Disconnected)),
                    "exact426 denies ordinary generation publication; the shared upgrade projection owns the message");
                // No token refresh, reference deletion or replacement task.
                assert_no_request(&remaining_requests);
            }
        }
    }

    #[test]
    fn billing_capture_recorder_waits_for_delayed_fragmented_request() {
        use std::io::Write;
        use std::net::TcpStream;

        let (listener, _url) = listener();
        listener.set_nonblocking(true).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        let (mut accepted, _) = listener.accept().unwrap();
        // Exercise the inherited macOS mode explicitly on every host.
        accepted.set_nonblocking(true).unwrap();
        let writer = std::thread::spawn(move || -> std::io::Result<()> {
            // Controlled network delay: the reader must survive both a missing
            // first byte and a body that has not arrived in its entirety yet.
            std::thread::sleep(Duration::from_millis(150));
            client.write_all(b"POST /fixture HTTP/1.1\r\nContent-Length: 4\r\n\r\nA\0")?;
            std::thread::sleep(Duration::from_millis(50));
            client.write_all(b"\xffB")
        });
        let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            read_request_bytes(&mut accepted)
        }));
        // Keep the accepted socket alive and join the writer before asserting,
        // including when the old reader panics during the required RED run.
        let writer_result = writer.join();
        drop(accepted);
        drop(listener);
        writer_result.expect("socket writer must settle").unwrap();
        assert_eq!(
            observed.expect("recorder must wait for delayed request bytes"),
            b"POST /fixture HTTP/1.1\r\nContent-Length: 4\r\n\r\nA\0\xffB"
        );
    }

    fn upscale_input_record(fixture: &Fixture) -> PendingGenerationRecord {
        let key=ManagedFileKey::new(ManagedUserArea::Input,"upscale-source.png").unwrap();
        let path=fixture.authority.lease().namespace.path(ManagedUserArea::Input).join("upscale-source.png");
        let bytes=encode_png_rgba(&image::RgbaImage::from_pixel(2,2,image::Rgba([20,40,60,255])),2,2).unwrap();
        let mut file=fixture.authority.create_new_regular(&key).unwrap();
        fixture.authority.write_new_regular_from(&mut file,&mut Cursor::new(&bytes)).unwrap();
        fixture.authority.sync_regular(&mut file).unwrap();
        let (sha256,sizes)=reference_fingerprints_for_namespace(&fixture.authority,std::slice::from_ref(&path)).unwrap();
        let mut record = generation_record(&fixture.scope, "image_upscale");
        record.reference_paths = vec![path.display().to_string()];
        record.reference_sha256 = sha256;
        record.reference_size_bytes = sizes;
        record.lineage_reference_paths = vec!["retained-lineage".into()];
        record.uploaded_file_ids.clear();
        record.generation_prompt = "actual upscale generation prompt".into();
        record
    }

    fn upscale_document(authority: &NamespaceStorageAuthority) -> Value {
        let key =
            ManagedFileKey::new(ManagedUserArea::Recovery, "pending-generations.json").unwrap();
        let mut file = authority.open_existing_regular(&key).unwrap();
        let mut bytes = Vec::new();
        authority.read_regular_to(&mut file, &mut bytes).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn billing_capture_actual_upscale_submission_persists_before_upload_and_billing() {
        use std::io::Write;
        let (listener, url) = listener();
        let mut fixture = fixture(&url);
        let record = upscale_input_record(&fixture);
        let original = serde_json::to_value(&record).unwrap();
        let submission = PreparedUpscaleSubmission::new(
            fixture.backend.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            record,
        )
        .unwrap();
        assert_eq!(
            upscale_document(&fixture.authority)["generations"][0],
            original
        );
        fixture.scope.request.account_group_id = OTHER.into();
        fixture.scope.context_epoch += 1;
        let authority = fixture.authority.clone();
        let transport = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let mut billed = None;
            for step in 0..5 {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(error) => panic!("actual upscale request {step} missing: {error}"),
                    }
                };
                let bytes = read_request_bytes(&mut stream);
                let request = String::from_utf8_lossy(&bytes);
                let success = serde_json::json!({"request_id":"fixture", "data":{}, "error":null, "meta":null});
                let (status, response) = match step {
                    0 => {
                        assert!(request.starts_with("POST /v1/uploads/references HTTP/"));
                        assert!(request.to_lowercase().contains("x-token: capture-access"));
                        assert!(!request.to_lowercase().contains("x-account-group-id:"));
                        assert_eq!(upscale_document(&authority)["generations"][0], original, "initial identity and inputs must be durable before the first network request");
                        (
                            "200 OK",
                            serde_json::json!({"request_id":"fixture", "data":{"file":{"id":OTHER}, "upload":{"method":"POST", "url":format!("{url}fixture-upload"), "fields":{}, "file_field":"file"}}, "error":null, "meta":null}),
                        )
                    }
                    1 => {
                        assert!(request.starts_with("POST /fixture-upload HTTP/"));
                        assert!(request.contains("multipart/form-data"));
                        assert!(bytes
                            .windows(8)
                            .any(|window| window == b"\x89PNG\r\n\x1a\n"));
                        ("200 OK", success)
                    }
                    2 => {
                        assert!(request.starts_with(&format!(
                            "POST /v1/uploads/references/{OTHER}/complete HTTP/"
                        )));
                        ("200 OK", success)
                    }
                    3 => {
                        assert!(request.starts_with("POST /v1/generation/tasks HTTP/"));
                        let document = upscale_document(&authority);
                        let row = &document["generations"][0];
                        assert_eq!(row["reference_paths"], serde_json::json!([]));
                        assert_eq!(row["reference_sha256"], serde_json::json!([]));
                        assert_eq!(row["reference_size_bytes"], serde_json::json!([]));
                        assert_eq!(
                            row["lineage_reference_paths"],
                            original["lineage_reference_paths"]
                        );
                        assert_eq!(row["uploaded_file_ids"], serde_json::json!([OTHER]));
                        let captured = Captured {
                            request: String::from_utf8(bytes).unwrap(),
                            document,
                        };
                        assert_capture(&captured, "generations");
                        let body: Value = serde_json::from_str(
                            captured.request.split("\r\n\r\n").nth(1).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(body["task_type"], "image_upscale");
                        assert_eq!(body["prompt"], original["generation_prompt"]);
                        assert_eq!(body["model_code"], original["model_code"]);
                        assert_eq!(body["quality"], original["quality"]);
                        assert_eq!(body["target_width"], original["target_width"]);
                        assert_eq!(body["target_height"], original["target_height"]);
                        assert_eq!(body["reference_file_ids"], serde_json::json!([OTHER]));
                        billed = Some(captured);
                        (
                            "400 Bad Request",
                            serde_json::json!({"request_id":"fixture-stop", "data":null, "error":{"code":"invalid_parameter", "message":"fixture-stop", "details":null}, "meta":null}),
                        )
                    }
                    4 => {
                        assert!(request
                            .starts_with(&format!("DELETE /v1/uploads/references/{OTHER} HTTP/")));
                        ("200 OK", success)
                    }
                    _ => unreachable!(),
                };
                let body = response.to_string();
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            billed.expect("actual billable upscale request must have been observed")
        });
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            submission.run(
                sender,
                Arc::new(Mutex::new(BTreeSet::new())),
                "display prompt".into(),
            )
        });
        let _observed = transport.join().unwrap();
        worker.join().unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            GenerationOutcome::Failure { .. }
        ));
        assert!(load_pending_generations_for_namespace(&fixture.authority)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn billing_capture_actual_upscale_storage_failure_prevents_dispatch() {
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let record = upscale_input_record(&fixture);
        corrupt(&fixture.authority, "pending-generations.json");
        assert!(matches!(
            PreparedUpscaleSubmission::new(
                fixture.backend.clone(),
                fixture.authority.clone(),
                &fixture.scope,
                record
            ),
            Err(ApiError::LocalState { .. })
        ));
        assert_no_request(&listener);
    }

    #[test]
    fn billing_capture_actual_upscale_scope_failure_prevents_dispatch() {
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let record = upscale_input_record(&fixture);
        let mut wrong_scope = fixture.scope.clone();
        wrong_scope.request.session.auth_epoch += 1;
        assert!(matches!(
            PreparedUpscaleSubmission::new(
                fixture.backend.clone(),
                fixture.authority.clone(),
                &wrong_scope,
                record
            ),
            Err(ApiError::LocalState { .. })
        ));
        assert!(load_pending_generations_for_namespace(&fixture.authority)
            .unwrap()
            .is_empty());
        assert_no_request(&listener);
    }

    #[test]
    fn billing_capture_actual_upscale_expired_worker_preserves_record_without_dispatch() {
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let submission = PreparedUpscaleSubmission::new(
            fixture.backend.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            upscale_input_record(&fixture),
        )
        .unwrap();
        let before = upscale_document(&fixture.authority);
        fixture.backend.api.session().clear().unwrap();
        let (sender, receiver) = mpsc::channel();
        submission.run(
            sender,
            Arc::new(Mutex::new(BTreeSet::new())),
            "display prompt".into(),
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(upscale_document(&fixture.authority), before);
        assert_no_request(&listener);
    }

    #[test]
    fn billing_capture_actual_upscale_invalid_input_prevents_dispatch() {
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let mut record = upscale_input_record(&fixture);
        record.reference_paths.clear();
        assert!(matches!(
            PreparedUpscaleSubmission::new(
                fixture.backend.clone(),
                fixture.authority.clone(),
                &fixture.scope,
                record
            ),
            Err(ApiError::LocalState { .. })
        ));
        assert!(load_pending_generations_for_namespace(&fixture.authority)
            .unwrap()
            .is_empty());
        assert_no_request(&listener);
    }

    struct PublishedGenerationFixture {
        inner: Fixture,
        _writer: client_state::tests::Fixture,
    }
    impl std::ops::Deref for PublishedGenerationFixture {
        type Target=Fixture;fn deref(&self)->&Fixture{&self.inner}
    }
    impl std::ops::DerefMut for PublishedGenerationFixture {
        fn deref_mut(&mut self)->&mut Fixture{&mut self.inner}
    }
    impl Drop for PublishedGenerationFixture {
        fn drop(&mut self){
            let lease=self.authority.lease().clone();
            *self.context.active_namespace.lock().unwrap()=None;
            let delivery=drain_delivery_commit_workers_for_lease_for_test(&lease);
            let previews=drain_activation_preview_workers_for_lease_for_test(&lease);
            let quiet=self.context.user_activity.begin_quiesce(&lease).map(|guard|guard.retire());
            if !std::thread::panicking(){delivery.unwrap();previews.unwrap();quiet.unwrap();}
        }
    }
    fn published_generation_fixture(base_url:&str)->PublishedGenerationFixture {
        let mut inner=fixture(base_url);
        *inner.context.active_namespace.lock().unwrap()=None;
        inner.context.user_activity.begin_quiesce(inner.authority.lease()).unwrap().retire();
        let writer=client_state::tests::Fixture::new(false,false);
        let lease=writer.lease(OWNER,inner.scope.request.session.auth_epoch,1);
        writer.activate(lease.clone()).unwrap();
        let root=writer.data_root_capability_arc();
        let index=FileIndex::initialize(inner.root.path().join("actual-ui-index.sqlite3")).unwrap();
        inner.context.user_activity.activate(lease.clone()).unwrap();
        *inner.context.active_namespace.lock().unwrap()=Some(lease.clone());
        inner.context.data_root_capability=Some(root.clone());inner.context.file_index=Some(index.clone());
        let persistence=PrivatePersistence::for_test_with_storage((*writer).clone(),lease.clone(),
            inner.context.user_activity.clone(),inner.backend.api.upgrade_latch().clone(),root,inner.backend.api.clone(),index);
        inner.authority=persistence.storage_authority().unwrap();
        {
            let transition=inner.context.namespace_operations.try_begin_transition().unwrap();
            let recovery=transition.begin_prepublication_recovery(&lease).unwrap();
            recovery.verify_no_unsupported_imports(&inner.authority).unwrap();
            let proof=recovery.finish().unwrap();
            transition.prepare_publication(&lease,proof).unwrap().publish();
        }
        inner.context.store.borrow_mut().private_persistence=Some(persistence);
        PublishedGenerationFixture{inner,_writer:writer}
    }

    fn app() -> AppWindow {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        app.global::<AppState>().set_session_state("online".into());
        app.global::<AppState>()
            .set_image_model("fixture-model".into());
        app.global::<AppState>().set_quality("1K".into());
        app
    }
    #[test]
    fn billing_capture_generation_start_persists_identity_before_real_dispatch() {
        let app = app();
        let (listener, url) = listener();
        let mut fixture = published_generation_fixture(&url);
        let (release, transport) = capture(
            listener,
            fixture.authority.clone(),
            "pending-generations.json",
        );
        start_backend_generation_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            "fixture prompt".into(),
            false,
            None,
            Some(1),
            ExistingGenerationPolicy::KeepExisting,
            GenerationDestination::Gallery,
        );
        fixture.scope.request.account_group_id = OTHER.into();
        fixture.scope.context_epoch += 1;
        release.send(()).unwrap();
        let observed = transport.join().unwrap();
        assert_capture(&observed, "generations");
    }
    #[test]
    fn billing_capture_generation_storage_and_scope_failure_prevent_dispatch() {
        let app = app();
        let (listener, url) = listener();
        let fixture = published_generation_fixture(&url);
        corrupt(&fixture.authority, "pending-generations.json");
        start_backend_generation_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            "fixture prompt".into(),
            false,
            None,
            Some(1),
            ExistingGenerationPolicy::KeepExisting,
            GenerationDestination::Gallery,
        );
        assert!(fixture.context.generations.active.borrow().is_empty());
        let mut wrong = fixture.scope.clone();
        wrong.request.session.auth_epoch += 1;
        start_backend_generation_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.authority.clone(),
            &wrong,
            "fixture prompt".into(),
            false,
            None,
            Some(1),
            ExistingGenerationPolicy::KeepExisting,
            GenerationDestination::Gallery,
        );
        assert!(fixture.context.generations.active.borrow().is_empty());
        assert_no_request(&listener);
    }
    #[test]
    fn billing_capture_image_edit_worker_keeps_persisted_payer() {
        assert_generation_worker(
            "image_edit",
            |backend, authority, scope, session, record, sender| {
                run_generation_with_billing_scope(
                    backend,
                    authority,
                    scope,
                    session,
                    record,
                    sender,
                    Arc::new(Mutex::new(BTreeSet::new())),
                )
            },
        );
    }
    #[test]
    fn billing_capture_alternate_upscale_worker_keeps_persisted_payer() {
        assert_generation_worker(
            "image_upscale",
            |backend, authority, scope, session, record, sender| {
                run_generation_with_billing_scope(
                    backend,
                    authority,
                    scope,
                    session,
                    record,
                    sender,
                    Arc::new(Mutex::new(BTreeSet::new())),
                )
            },
        );
    }
}
