use super::*;

#[derive(Clone, PartialEq, Eq)]
struct CustomEditorSnapshot {
    session: String, input: String, name: String, category: String, format: String,
    negative: String, original: String, recovered: String, return_page: String,
    reference_path: String, references: Vec<(String,String)>, analyzing: bool,
    open: bool, page: String,
}
impl CustomEditorSnapshot {
    fn capture(app:&AppWindow)->Self {
        let state=app.global::<AppState>();
        let model=state.get_custom_prompt_reference_items();
        Self {
            session:state.get_custom_prompt_editor_session_id().into(),
            input:state.get_custom_prompt_input().into(),name:state.get_custom_prompt_name().into(),
            category:state.get_custom_prompt_category().into(),format:state.get_custom_prompt_format().into(),
            negative:state.get_custom_prompt_negative().into(),original:state.get_custom_prompt_editing_original().into(),
            recovered:state.get_custom_prompt_recovered_request_id().into(),
            return_page:state.get_custom_prompt_editor_return_page().into(),
            reference_path:state.get_custom_prompt_reference_path().into(),
            references:(0..model.row_count()).filter_map(|i|model.row_data(i))
                .map(|row|(row.id.to_string(),row.source_path.to_string())).collect(),
            analyzing:state.get_custom_prompt_analyzing(),open:state.get_custom_prompt_editor_open(),
            page:state.get_page().into(),
        }
    }
}
struct CustomStagedSave {
    lease:NamespaceLease, editor:String, body:String, profile:CustomPromptProfile,
    timestamp:String, original_argument:String, flight:Option<Uuid>,
}
type CustomSaveState=Rc<RefCell<Option<CustomStagedSave>>>;
struct CustomSaveFlight { state:CustomSaveState, id:Uuid }
impl Drop for CustomSaveFlight {
    fn drop(&mut self) {
        if let Some(staged)=self.state.borrow_mut().as_mut() {
            if staged.flight==Some(self.id) { staged.flight=None; }
        }
    }
}
fn custom_profile_equal(a:&CustomPromptProfile,b:&CustomPromptProfile)->bool {
    a.name==b.name && a.category==b.category && a.format==b.format
        && a.negative_prompt==b.negative_prompt && a.reference_path==b.reference_path
        && a.reference_paths==b.reference_paths
}
fn custom_save_message(app:&AppWindow,english:&str,chinese:&str) {
    let state=app.global::<AppState>();
    state.set_custom_prompt_message(if state.get_language()=="en"{english}else{chinese}.into());
}
fn custom_editor_binding(context:&AppContext,persistence:&PrivatePersistence)->bool {
    context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.same_binding_metadata(persistence)) && persistence.is_current()
}
fn captured_custom_editor(context:&AppContext)->Option<PrivatePersistence> {
    let persistence=context.store.borrow().private_persistence.clone()?;
    let activity=persistence.begin_activity().ok()?;
    let current=custom_editor_binding(context,&persistence);
    drop(activity);
    current.then_some(persistence)
}
fn start_custom_prompt_save(app:&AppWindow,context:AppContext,save_state:CustomSaveState,original:String,prompt:String) {
    let Some(persistence)=captured_custom_editor(&context)else{return;};
    let Ok(write)=persistence.prepare_ordered_save()else{return;};
    let mut write=Some(write);let mut editor=None;let mut flight=None;
    let outcome=context.apply_user_completion(persistence.lease(),||{
        if !context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.same_binding_metadata(&persistence)){return None;}
        let captured=CustomEditorSnapshot::capture(app);
        let state=app.global::<AppState>();let name=state.get_custom_prompt_name().trim().to_owned();
        if name.is_empty(){custom_save_message(app,"Enter a prompt name","请输入提示词名称");return None;}
        let format=normalized_custom_prompt_format(state.get_custom_prompt_format().as_str());
        let references=custom_prompt_reference_paths(app);
        let profile=CustomPromptProfile {
            name:name.clone(),category:normalized_custom_prompt_category(state.get_custom_prompt_category().as_str()),
            format:format.clone(),negative_prompt:if format=="json"{state.get_custom_prompt_negative().trim().into()}else{String::new()},
            reference_path:references.first().cloned().unwrap_or_default(),reference_paths:references,
        };
        let mut tracking=save_state.borrow_mut();
        let same=tracking.as_ref().is_some_and(|saved|saved.lease==*persistence.lease() && saved.editor==captured.session && saved.original_argument==original.trim());
        if same && tracking.as_ref().unwrap().flight.is_some(){return None;}
        let mut store=context.store.borrow_mut();
        let mut effective_original=original.trim().to_owned();
        let mut already_staged=false;
        if same {
            let saved=tracking.as_ref().unwrap();
            let unchanged=store.custom_prompts.iter().filter(|body|*body==&saved.body).count()==1
                && store.custom_prompt_profiles.get(&saved.body).is_some_and(|value|custom_profile_equal(value,&saved.profile))
                && store.custom_prompt_times.get(&saved.body)==Some(&saved.timestamp);
            if !unchanged {
                custom_save_message(app,"The staged prompt changed elsewhere. Reopen that saved prompt to continue.",
                    "已保留的提示词已在其他位置更改，请重新打开该提示词继续编辑");
                return None;
            }
            effective_original=saved.body.clone();
            already_staged=prompt.trim()==saved.body && custom_profile_equal(&profile,&saved.profile);
        }
        let category=current_workspace_category(app);
        let original_name=custom_prompt_display_name(&store,&effective_original);
        let selected=custom_prompt_selected_for_category(&store,&category,&effective_original);
        if !already_staged {
            let result=save_custom_prompt_to_store(&mut store,&effective_original,&prompt,
                &Local::now().format("%Y-%m-%d %H:%M").to_string());
            if result!=SaveCustomPromptResult::Saved {
                let (en,zh)=match result {
                    SaveCustomPromptResult::Empty=>("Enter a prompt first","请输入提示词"),
                    SaveCustomPromptResult::Duplicate=>("This prompt already exists","该提示词已存在"),
                    _=>("This prompt no longer exists","该提示词已不存在，请关闭后重试"),
                };
                custom_save_message(app,en,zh);return None;
            }
            save_custom_prompt_profile(&mut store,&effective_original,&prompt,profile.clone());
            if !effective_original.is_empty(){replace_selected_custom_prompt(&mut store,&effective_original,prompt.trim());}
            if selected && original_name!=name {
                state.set_prompt(replace_custom_prompt_name(state.get_prompt().as_str(),&original_name,&name).text.into());
            }
        }
        let id=Uuid::new_v4();
        *tracking=Some(CustomStagedSave {lease:persistence.lease().clone(),editor:captured.session.clone(),
            body:prompt.trim().into(),profile,original_argument:original.trim().into(),timestamp:store.custom_prompt_times.get(prompt.trim()).cloned().unwrap(),
            flight:Some(id)});
        drop(tracking);
        flight=Some(CustomSaveFlight {state:save_state.clone(),id});
        push_custom_prompts(app,&store);
        custom_save_message(app,"Saving...","正在保存...");
        editor=Some(captured);
        // A retry saves the CURRENT Store without replaying an already staged
        // insertion; later unrelated edits stay in the ordered snapshot.
        Some(write.take().unwrap().enqueue(local_store_data(app,&store)))
    });
    drop(write);
    let receiver=match outcome.ok().flatten(){
        Some(Ok(receiver))=>receiver,
        Some(Err(error))=>{
            drop(error);drop(flight);
            let _=context.apply_user_completion(persistence.lease(),||custom_save_message(app,
                "Unable to confirm local save. Your content is retained; please retry.",
                "本地保存未确认，内容仍已保留，请重试"));
            return;
        }
        None=>return,
    };
    let captured=editor.unwrap();let flight=flight.unwrap();
    let key_for_worker=captured.recovered.clone();
    let job=spawn_delivery_preparation(&persistence,move|captured,activity,cancel|{
        receiver.recv().map_err(|_|anyhow!("custom Store acknowledgment disconnected"))?.map_err(anyhow::Error::from)?;
        if cancel.load(Ordering::SeqCst)||activity.is_quiescing()||!captured.is_current(){return Err(DeliveryRetryError::AuthenticationRequired);}
        if key_for_worker.is_empty(){return Ok(None);}
        let authority=captured.storage_authority()?;
        let row=load_pending_prompt_tasks_for_namespace(&authority)?.into_iter()
            .find(|row|row.client_request_id==key_for_worker && row.target_kind=="custom_prompt" && row.applied_to_target)
            .ok_or_else(||anyhow!("original custom recovery record unavailable; retained"))?;
        Ok(Some(row.identity()))
    });
    match job {
        Ok((cancel,receiver))=>poll_custom_prompt_save(app.as_weak(),context,persistence,cancel,receiver,captured,flight),
        Err(_)=>{
            drop(flight);
            let _=context.apply_user_completion(persistence.lease(),||custom_save_message(app,
                "Unable to confirm the save. Please retry; recovery data is retained.",
                "保存结果未确认，请重试；恢复记录仍已保留"));
        }
    }
}
fn poll_custom_prompt_save(
    weak:Weak<AppWindow>,context:AppContext,persistence:PrivatePersistence,
    cancel:Arc<std::sync::atomic::AtomicBool>,
    receiver:mpsc::Receiver<std::result::Result<Option<RecoveryRecordIdentity>,DeliveryRetryError>>,
    editor:CustomEditorSnapshot,flight:CustomSaveFlight,
) {
    slint::Timer::single_shot(Duration::from_millis(50),move||{
        let Some(app)=weak.upgrade()else{cancel.store(true,Ordering::SeqCst);custom_input_orphan_poll(cancel);return;};
        match finish_delivery_preparation(&cancel){
            Ok(true)=>{poll_custom_prompt_save(weak,context,persistence,cancel,receiver,editor,flight);return;},
            Err(_)=>return,
            Ok(false)=>{},
        }
        let result=match receiver.try_recv(){
            Ok(result)=>result,
            Err(TryRecvError::Empty)=>{poll_custom_prompt_save(weak,context,persistence,cancel,receiver,editor,flight);return;}
            Err(TryRecvError::Disconnected)=>Err(anyhow!("custom save worker disconnected").into()),
        };
        if !custom_editor_binding(&context,&persistence){return;}
        #[cfg(test)]
        if result.is_ok() {
            CUSTOM_SAVE_AFTER_ACK.with(|hook|if let Some(hook)=hook.borrow_mut().take(){hook();});
        }
        let identity=match result {
            Ok(identity)=>identity,
            Err(_)=>{
                let _=context.apply_user_completion(persistence.lease(),||{
                    if CustomEditorSnapshot::capture(&app)==editor {
                        custom_save_message(&app,"Unable to confirm local save or recovery. Your content is retained; please retry.",
                            "本地保存或恢复确认未完成，内容与恢复记录仍可重试");
                    }
                });return;
            }
        };
        if let Some(identity)=identity {
            acknowledge_custom_prompt_recovered_result_captured(&app,&context,persistence.clone(),identity);
        }
        let visuals=prepare_delivery_visuals(&app,&context.store.borrow());
        let effects=context.apply_user_completion(persistence.lease(),||{
            // Include every field cleared by reset/close. Name-only, profile-only,
            // reference and editing-target changes are as significant as body edits.
            if CustomEditorSnapshot::capture(&app)!=editor {return None;}
            let state=app.global::<AppState>();
            let return_page=if editor.return_page=="generation"{"generation"}else{"settings"};
            reset_custom_prompt_editor(&app);
            state.set_custom_prompt_editor_open(false);
            state.set_custom_prompt_editor_session_id("".into());
            state.set_custom_prompt_recovered_request_id("".into());
            state.set_page(return_page.into());
            Some(visuals.publish_metadata(&app,persistence.clone()))
        }).ok().flatten();
        if let Some(effects)=effects {
            if flight.state.borrow().as_ref().is_some_and(|saved|saved.flight==Some(flight.id)) {
                flight.state.borrow_mut().take();
            }
            start_activation_visual_effects(&app,context,effects);
        }
        // No TLS lookup or persistence occurs on Drop. Failed saves leave an
        // exact staged retry intent; successful changed editors retain that intent.
        drop(flight);
    });
}

#[cfg(test)]
thread_local! {
    static CUSTOM_SAVE_AFTER_ACK:RefCell<Option<Box<dyn FnOnce()>>>=const{RefCell::new(None)};
}

fn persist_custom_prompt_before_ack(
    persist: impl FnOnce() -> Result<()>,
    acknowledge: impl FnOnce(),
) -> Result<()> {
    persist()?;
    #[cfg(test)]
    CUSTOM_SAVE_AFTER_ACK.with(|hook|if let Some(hook)=hook.borrow_mut().take(){hook();});
    acknowledge();
    Ok(())
}

pub(super) const INLINE_CUSTOM_PROMPT_ICON_PLACEHOLDER: char = '\u{3000}';

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PromptEditorEdit {
    pub(super) text: String,
    pub(super) cursor_offset: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InlineCustomPromptOccurrence {
    pub(super) name: String,
    pub(super) content: String,
    pub(super) prefix: String,
    pub(super) start_offset: i32,
    pub(super) end_offset: i32,
}

pub(super) fn inline_custom_prompt_display_text(name: &str) -> String {
    format!("{INLINE_CUSTOM_PROMPT_ICON_PLACEHOLDER}{}", name.trim())
}

fn clamped_char_boundary(text: &str, requested: usize) -> usize {
    let mut offset = requested.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn needs_spacing(character: char) -> bool {
    !character.is_whitespace()
        && !matches!(
            character,
            '，' | '。' | '、' | '；' | '：' | '！' | '？' | ',' | '.' | ';' | ':' | '!' | '?'
        )
}

pub(super) fn remove_custom_prompt_trigger_before_cursor(
    editor_text: &str,
    requested_offset: usize,
) -> PromptEditorEdit {
    let offset = clamped_char_boundary(editor_text, requested_offset);
    if offset == 0 || !editor_text[..offset].ends_with('/') {
        return PromptEditorEdit {
            text: editor_text.to_string(),
            cursor_offset: -1,
        };
    }
    let trigger_start = offset - 1;
    let mut text = String::with_capacity(editor_text.len() - 1);
    text.push_str(&editor_text[..trigger_start]);
    text.push_str(&editor_text[offset..]);
    PromptEditorEdit {
        text,
        cursor_offset: trigger_start.min(i32::MAX as usize) as i32,
    }
}

pub(super) fn insert_custom_prompt_name_at_byte_offset(
    editor_text: &str,
    requested_offset: usize,
    name: &str,
) -> PromptEditorEdit {
    let offset = clamped_char_boundary(editor_text, requested_offset);
    let before = &editor_text[..offset];
    let after = &editor_text[offset..];
    let leading_space = before.chars().last().is_some_and(needs_spacing);
    let trailing_space = after.chars().next().is_some_and(needs_spacing);
    let display = inline_custom_prompt_display_text(name);
    let mut inserted = String::new();
    if leading_space {
        inserted.push(' ');
    }
    inserted.push_str(&display);
    if trailing_space {
        inserted.push(' ');
    }
    let cursor_offset = offset.saturating_add(inserted.len());
    let mut text = String::with_capacity(editor_text.len() + inserted.len());
    text.push_str(before);
    text.push_str(&inserted);
    text.push_str(after);
    PromptEditorEdit {
        text,
        cursor_offset: cursor_offset.min(i32::MAX as usize) as i32,
    }
}

pub(super) fn remove_custom_prompt_name(editor_text: &str, name: &str) -> PromptEditorEdit {
    let display = inline_custom_prompt_display_text(name);
    let Some(start) = editor_text.find(&display) else {
        return PromptEditorEdit {
            text: editor_text.to_string(),
            cursor_offset: editor_text.len().min(i32::MAX as usize) as i32,
        };
    };
    let mut remove_start = start;
    let mut remove_end = start + display.len();
    let before = &editor_text[..remove_start];
    let after = &editor_text[remove_end..];
    let before_space = before.chars().last().filter(|value| value.is_whitespace());
    let after_space = after.chars().next().filter(|value| value.is_whitespace());
    if let Some(character) = after_space.filter(|_| before_space.is_some() || before.is_empty()) {
        remove_end += character.len_utf8();
    } else if let Some(character) = before_space.filter(|_| after.is_empty()) {
        remove_start -= character.len_utf8();
    }
    let mut text = String::with_capacity(editor_text.len() - (remove_end - remove_start));
    text.push_str(&editor_text[..remove_start]);
    text.push_str(&editor_text[remove_end..]);
    PromptEditorEdit {
        text,
        cursor_offset: remove_start.min(i32::MAX as usize) as i32,
    }
}

pub(super) fn replace_custom_prompt_name(
    editor_text: &str,
    original_name: &str,
    replacement_name: &str,
) -> PromptEditorEdit {
    let original = inline_custom_prompt_display_text(original_name);
    let Some(start) = editor_text.find(&original) else {
        return PromptEditorEdit {
            text: editor_text.to_string(),
            cursor_offset: editor_text.len().min(i32::MAX as usize) as i32,
        };
    };
    let replacement = inline_custom_prompt_display_text(replacement_name);
    let end = start + original.len();
    let mut text = String::with_capacity(editor_text.len() + replacement.len() - original.len());
    text.push_str(&editor_text[..start]);
    text.push_str(&replacement);
    text.push_str(&editor_text[end..]);
    PromptEditorEdit {
        text,
        cursor_offset: (start + replacement.len()).min(i32::MAX as usize) as i32,
    }
}

pub(super) fn inline_custom_prompt_occurrences(
    editor_text: &str,
    replacements: &[(String, String)],
) -> Vec<InlineCustomPromptOccurrence> {
    let mut occurrences = replacements
        .iter()
        .filter_map(|(name, content)| {
            let display = inline_custom_prompt_display_text(name);
            let start = editor_text.find(&display)?;
            let end = start + display.len();
            Some(InlineCustomPromptOccurrence {
                name: name.clone(),
                content: content.clone(),
                prefix: editor_text[..start].to_string(),
                start_offset: start.min(i32::MAX as usize) as i32,
                end_offset: end.min(i32::MAX as usize) as i32,
            })
        })
        .collect::<Vec<_>>();
    occurrences.sort_by_key(|item| item.start_offset);
    occurrences
}

fn slint_prompt_text_edit(edit: PromptEditorEdit) -> PromptTextEdit {
    PromptTextEdit {
        text: edit.text.into(),
        cursor_offset: edit.cursor_offset,
    }
}

#[derive(Clone,PartialEq,Eq)]
struct CustomEffectTarget{page:String,category:String,editor:String,original:String,return_page:String,open:bool}
impl CustomEffectTarget{
    fn capture(app:&AppWindow)->Self{let state=app.global::<AppState>();Self{
        page:state.get_page().into(),category:current_workspace_category(app),editor:state.get_custom_prompt_editor_session_id().into(),
        original:state.get_custom_prompt_editing_original().into(),return_page:state.get_custom_prompt_editor_return_page().into(),open:state.get_custom_prompt_editor_open(),
    }}
}
#[derive(Clone)]
struct CustomEffectCapture{context:AppContext,persistence:PrivatePersistence,session:SessionScope,target:CustomEffectTarget}
impl CustomEffectCapture{
    fn capture(app:&AppWindow,context:&AppContext)->Option<Self>{
        let p=context.store.borrow().private_persistence.clone()?;let session=context.current_account_session_scope()?;
        let captured=Self{context:context.clone(),persistence:p,session,target:CustomEffectTarget::capture(app)};
        captured.current(app).then_some(captured)
    }
    fn binding(&self)->bool{
        self.session.owner_user_id==self.persistence.lease().namespace.user_public_id()
            && self.session.auth_epoch==self.persistence.lease().auth_epoch
            && self.context.store.borrow().private_persistence.as_ref().is_some_and(|p|p.same_binding_metadata(&self.persistence))
            && self.context.active_namespace.lock().ok().is_some_and(|lease|lease.as_ref()==Some(self.persistence.lease()))
            && self.context.backend.as_ref().is_some_and(|backend|backend.api.session().is_scope_current(&self.session))
    }
    fn current(&self,app:&AppWindow)->bool{self.binding()&&self.persistence.is_current()&&CustomEffectTarget::capture(app)==self.target}
    fn apply<R>(&self,app:&AppWindow,apply:impl FnOnce()->R)->Option<R>{
        if !self.current(app){return None;}
        let activity=self.persistence.begin_activity().ok()?;
        let result=self.context.apply_user_completion(self.persistence.lease(),||{
            if !self.binding()||CustomEffectTarget::capture(app)!=self.target{return None;}Some(apply())
        }).ok().flatten();
        drop(activity);result
    }
    fn message(&self,app:&AppWindow,en:&str,zh:&str){self.apply(app,||custom_save_message(app,en,zh));}
}
#[derive(Default)]
struct CustomEffects{input:Option<CustomInputSlot>,sources:Vec<(String,String)>,save_revision:Option<Uuid>,save_retry:bool,save_binding:Option<PrivatePersistence>,save_action:Option<(CustomEffectTarget,CustomStoreMutation)>}
#[derive(Clone,PartialEq,Eq)]
enum CustomStoreMutation{NormalizeSelection(BTreeSet<String>),Inline{prompt:String,input:String,cursor:i32},Toggle(String),ClearSelection,Remove(String)}
struct CustomInputSlot{id:Uuid,cancel:Option<Arc<std::sync::atomic::AtomicBool>>}
impl Drop for CustomInputSlot{fn drop(&mut self){if let Some(cancel)=&self.cancel{cancel.store(true,Ordering::Release);}}}
#[derive(Clone)]
struct CustomInputTicket{state:Rc<RefCell<CustomEffects>>,id:Uuid}
impl CustomInputTicket{
    fn current(&self)->bool{self.state.borrow().input.as_ref().is_some_and(|slot|slot.id==self.id)}
    fn cancel(&self){if let Some(slot)=self.state.borrow().input.as_ref().filter(|slot|slot.id==self.id){if let Some(cancel)=&slot.cancel{cancel.store(true,Ordering::Release);}}}
}
fn begin_custom_input(app:&AppWindow,capture:&CustomEffectCapture,state:&Rc<RefCell<CustomEffects>>)->Option<CustomInputTicket>{
    capture.apply(app,||{let id=Uuid::new_v4();state.borrow_mut().input=Some(CustomInputSlot{id,cancel:None});CustomInputTicket{state:state.clone(),id}})
}
fn invalidate_custom_input(state:&Rc<RefCell<CustomEffects>>){state.borrow_mut().input=None;}
struct CustomInputJob<T>{cancel:Arc<std::sync::atomic::AtomicBool>,receiver:mpsc::Receiver<std::result::Result<T,DeliveryRetryError>>}
impl<T> Drop for CustomInputJob<T>{fn drop(&mut self){self.cancel.store(true,Ordering::Release);}}
#[cfg(test)]
thread_local!{
    static CUSTOM_INPUT_TEST_PREPARED:RefCell<Option<Box<dyn FnOnce(&Arc<std::sync::atomic::AtomicBool>)+Send>>>=const{RefCell::new(None)};
}
fn custom_input_orphan_poll(cancel:Arc<std::sync::atomic::AtomicBool>){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        if matches!(finish_delivery_preparation(&cancel),Ok(true)){custom_input_orphan_poll(cancel);}
    });
}
fn spawn_custom_input<T:Send+'static>(
    app:&AppWindow,capture:CustomEffectCapture,ticket:CustomInputTicket,
    work:impl FnOnce(&PrivatePersistence,&UserActivityPermit,&Arc<std::sync::atomic::AtomicBool>)->std::result::Result<T,DeliveryRetryError>+Send+'static,
    complete:impl FnOnce(&AppWindow,&CustomEffectCapture,&CustomInputTicket,std::result::Result<T,DeliveryRetryError>)+'static,
){
    if !ticket.current()||!capture.current(app){return;}
    #[cfg(test)]
    let prepared=CUSTOM_INPUT_TEST_PREPARED.with(|hook|hook.borrow_mut().take());
    let result=spawn_delivery_preparation(&capture.persistence,move|p,activity,cancel|{
        let result=work(p,activity,cancel);
        #[cfg(test)]
        if let Some(prepared)=prepared{prepared(cancel);}
        result
    });
    let(cancel,receiver)=match result{Ok(job)=>job,Err(_)=>{
        if ticket.current(){capture.message(app,"Unable to prepare references; please retry","参考图暂时无法准备，请重试");}return;
    }};
    if !ticket.current()||!capture.current(app){cancel.store(true,Ordering::Release);custom_input_orphan_poll(cancel);return;}
    ticket.state.borrow_mut().input.as_mut().unwrap().cancel=Some(cancel.clone());
    poll_custom_input(app.as_weak(),capture,ticket,CustomInputJob{cancel,receiver},complete);
}
fn poll_custom_input<T:Send+'static>(
    weak:Weak<AppWindow>,capture:CustomEffectCapture,ticket:CustomInputTicket,job:CustomInputJob<T>,
    complete:impl FnOnce(&AppWindow,&CustomEffectCapture,&CustomInputTicket,std::result::Result<T,DeliveryRetryError>)+'static,
){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        let Some(app)=weak.upgrade()else{job.cancel.store(true,Ordering::Release);custom_input_orphan_poll(job.cancel.clone());return;};
        if !ticket.current()||!capture.current(&app){job.cancel.store(true,Ordering::Release);}
        match finish_delivery_preparation(&job.cancel){
            Ok(true)=>{poll_custom_input(weak,capture,ticket,job,complete);return;},
            Err(_)=>return, // Sticky join failure: never publish even a queued success.
            Ok(false)=>{},
        }
        let result=match job.receiver.try_recv(){
            Ok(result)=>result,Err(TryRecvError::Disconnected)=>Err(anyhow!("custom input worker disconnected").into()),
            Err(TryRecvError::Empty)=>{poll_custom_input(weak,capture,ticket,job,complete);return;},
        };
        if ticket.current()&&capture.current(&app){complete(&app,&capture,&ticket,result);}
    });
}
fn custom_input_current(p:&PrivatePersistence,activity:&UserActivityPermit,cancel:&Arc<std::sync::atomic::AtomicBool>)->Result<()>{
    anyhow::ensure!(!cancel.load(Ordering::Acquire)&&!activity.is_quiescing()&&p.is_current(),"custom input retired");Ok(())
}
struct CustomOwnedReference{original:String,path:String,preview:PreparedDeliveryPreview}
fn import_custom_selected_paths(app:&AppWindow,capture:CustomEffectCapture,ticket:CustomInputTicket,paths:Vec<PathBuf>){
    if !ticket.current()||!capture.current(app){return;}
    let current=custom_prompt_reference_paths(app);let available=MAX_CUSTOM_PROMPT_REFERENCES.saturating_sub(current.len());
    if available==0{capture.message(app,"Reference image limit reached","参考图数量已达上限");return;}
    let known={let state=ticket.state.borrow();state.sources.iter().filter(|(_,owned)|current.contains(owned)).map(|(source,_)|source.clone()).chain(current.clone()).collect::<Vec<_>>()};
    let mut selected=Vec::new();
    for path in paths{
        let name=path.to_string_lossy().into_owned();
        if known.iter().any(|value|value.eq_ignore_ascii_case(&name))||selected.iter().any(|value:&PathBuf|value.to_string_lossy().eq_ignore_ascii_case(&name)){continue;}
        selected.push(path);if selected.len()==available{break;}
    }
    if selected.is_empty(){return;}
    spawn_custom_input(app,capture,ticket,move|p,activity,cancel|{
        let authority=p.storage_authority()?;let mut ready=Vec::new();let mut rejected=false;
        for source in selected{
            custom_input_current(p,activity,cancel)?;
            let prepared=(||->Result<CustomOwnedReference>{
                let bytes=authority.read_image_source(&source,100*1024*1024)?;
                let decoded=decode_reference_bytes(&bytes)?;custom_input_current(p,activity,cancel)?;
                let path=persist_reference_image_for_namespace(&authority,&decoded)?;
                let preview=prepare_owned_preview(p,&path,PreviewPurpose::Reference)?;
                Ok(CustomOwnedReference{original:source.to_string_lossy().into_owned(),path:path.to_str().ok_or_else(||anyhow!("unsupported owned reference"))?.into(),preview})
            })();
            match prepared{Ok(value)=>ready.push(value),Err(_)=>{custom_input_current(p,activity,cancel)?;rejected=true;}}
        }
        custom_input_current(p,activity,cancel)?;Ok((ready,rejected))
    },|app,capture,ticket,result|{
        match result{
            Ok((ready,rejected))=>{capture.apply(app,||{
                if !ticket.current(){return;}
                let mut rows=app.global::<AppState>().get_custom_prompt_reference_items().iter().collect::<Vec<_>>();
                let mut state=ticket.state.borrow_mut();
                for item in ready{
                    if rows.len()>=MAX_CUSTOM_PROMPT_REFERENCES{break;}
                    state.sources.push((item.original,item.path.clone()));
                    rows.push(ReferenceItem{id:Uuid::new_v4().to_string().into(),source_path:item.path.into(),image:materialize_delivery_preview(&item.preview)});
                }
                set_custom_prompt_references(app,rows);
                custom_save_message(app,if rejected{"Some selected images could not be safely imported"}else{""},if rejected{"部分所选图片未能安全导入"}else{""});
            });},
            Err(_)=>capture.message(app,"Unable to import references safely; originals are unchanged","参考图未能安全导入，原文件保持不变"),
        }
    });
}
type CustomReferencePickerCompletion=Box<dyn FnOnce(Option<Vec<PathBuf>>) >;
#[cfg(test)]
thread_local!{
    // Explicit fixture root is retained for test data ownership, not a preview fallback.
    static CUSTOM_REFERENCE_TEST_PREVIEW_ROOT:RefCell<Option<PathBuf>>=const{RefCell::new(None)};
    static CUSTOM_REFERENCE_TEST_PICKER:RefCell<Option<Box<dyn FnOnce(CustomReferencePickerCompletion)>>>=const{RefCell::new(None)};
}
fn start_custom_picker(app:&AppWindow,capture:CustomEffectCapture,ticket:CustomInputTicket){
    let weak=app.as_weak();
    #[cfg(test)]
    {
        // Same start boundary as the production future: no permit is held by
        // the retained test/native callback while waiting for selected paths.
        if !ticket.current()||!capture.current(app){return;}
        let Ok(effect)=capture.persistence.begin_effect()else{return;};drop(effect);
        let picker=CUSTOM_REFERENCE_TEST_PICKER.with(|hook|hook.borrow_mut().take()).expect("custom picker fixture missing");
        picker(Box::new(move|paths|{
            let Some(app)=weak.upgrade()else{return;};let Some(paths)=paths else{return;};
            if ticket.current()&&capture.current(&app){import_custom_selected_paths(&app,capture,ticket,paths);}
        }));
    }
    #[cfg(not(test))]
    {
        let error_capture=capture.clone();let error_ticket=ticket.clone();
        let spawned=slint::spawn_local(async move{
            let Some(app)=weak.upgrade()else{return;};
            if !ticket.current()||!capture.current(&app){return;}
            let Ok(effect)=capture.persistence.begin_effect()else{return;};drop(effect);
            if !ticket.current()||!capture.current(&app){return;}
            let dialog=rfd::AsyncFileDialog::new().add_filter("Images",crate::image_formats::picker_image_extensions());
            // rfd may synchronously fall back to a modal dialog even when
            // constructing its future. No counted permit spans that call/await.
            let pending=dialog.pick_files();drop(app);
            let files=pending.await;
            let Some(app)=weak.upgrade()else{return;};let Some(files)=files else{return;};
            if !ticket.current()||!capture.current(&app){return;}
            import_custom_selected_paths(&app,capture,ticket,files.into_iter().map(|file|file.path().to_path_buf()).collect());
        });
        if spawned.is_err()&&error_ticket.current(){error_capture.message(app,"Unable to open the image picker","暂时无法打开图片选择窗口");}
    }
}
fn prepare_custom_existing_previews(app:&AppWindow,capture:CustomEffectCapture,ticket:CustomInputTicket){
    let rows=app.global::<AppState>().get_custom_prompt_reference_items().iter().map(|row|(row.id.to_string(),row.source_path.to_string())).collect::<Vec<_>>();
    let owned=rows.into_iter().filter(|(_,path)|capture.persistence.owns_path(Path::new(path))).collect::<Vec<_>>();
    if owned.is_empty(){return;}
    spawn_custom_input(app,capture,ticket,move|p,activity,cancel|{
        let mut ready=Vec::new();for(id,path)in owned{
            custom_input_current(p,activity,cancel)?;
            if let Ok(preview)=prepare_owned_preview(p,Path::new(&path),PreviewPurpose::Reference){ready.push((id,path,preview));}
        }custom_input_current(p,activity,cancel)?;Ok(ready)
    },|app,capture,ticket,result|{
        let Ok(ready)=result else{return;};capture.apply(app,||{
            if !ticket.current(){return;}
            let mut rows=app.global::<AppState>().get_custom_prompt_reference_items().iter().collect::<Vec<_>>();
            for(id,path,preview)in ready{if let Some(row)=rows.iter_mut().find(|row|row.id==id&&row.source_path==path){row.image=materialize_delivery_preview(&preview);}}
            set_custom_prompt_references(app,rows);
        });
    });
}
fn custom_open_editor_metadata(app:&AppWindow){
    let state=app.global::<AppState>();let page=state.get_page().to_string();
    if page!="custom-prompt-editor"{state.set_custom_prompt_editor_return_page(if page=="generation"{"generation"}else{"settings"}.into());}
    state.set_custom_prompt_editor_open(true);state.set_page("custom-prompt-editor".into());
}
fn custom_flush_or_mutate<R>(app:&AppWindow,capture:&CustomEffectCapture,effects:&Rc<RefCell<CustomEffects>>,operation:CustomStoreMutation,mutate:impl FnOnce(&mut Store)->R)->Option<R>{
    if !capture.current(app){return None;}
    let Ok(prepared)=capture.persistence.prepare_ordered_save()else{return None;};let mut prepared=Some(prepared);
    let retry={let mut state=effects.borrow_mut();
        if !state.save_binding.as_ref().is_some_and(|p|p.same_binding_metadata(&capture.persistence)){
            state.save_retry=false;state.save_revision=None;state.save_action=None;state.save_binding=Some(capture.persistence.clone());
        }
        state.save_retry&&state.save_action.as_ref().is_some_and(|(target,saved)|target==&capture.target&&saved==&operation)
    };
    let queued=capture.apply(app,||{
        let mut store=capture.context.store.borrow_mut();
        let value=if retry{None}else{Some(mutate(&mut store))};
        if !retry{push_custom_prompts(app,&store);}
        let revision=Uuid::new_v4();{
            let mut tracking=effects.borrow_mut();tracking.save_revision=Some(revision);
            tracking.save_action=Some((capture.target.clone(),operation));
        }
        (value,revision,prepared.take().unwrap().enqueue(local_store_data(app,&store)))
    });
    drop(prepared);
    let(value,revision,queued)=queued?;
    let receiver=match queued{Ok(receiver)=>receiver,Err(error)=>{
        drop(error);effects.borrow_mut().save_retry=true;
        capture.message(app,"Local changes are retained; retry to confirm saving","本地修改已保留，请重试确认保存");return value;
    }};
    let result=spawn_delivery_preparation(&capture.persistence,move|_,_,_|{
        receiver.recv().map_err(|_|anyhow!("custom selection save disconnected"))?.map_err(anyhow::Error::from)?;Ok(())
    });
    match result{
        Ok((cancel,receiver))=>poll_custom_store_effect(app.as_weak(),capture.clone(),effects.clone(),revision,CustomInputJob{cancel,receiver}),
        Err(_)=>{effects.borrow_mut().save_retry=true;capture.message(app,"Local changes are retained; retry to confirm saving","本地修改已保留，请重试确认保存");},
    }value
}
fn poll_custom_store_effect(weak:Weak<AppWindow>,capture:CustomEffectCapture,effects:Rc<RefCell<CustomEffects>>,revision:Uuid,job:CustomInputJob<()>){
    slint::Timer::single_shot(Duration::from_millis(40),move||{
        let Some(app)=weak.upgrade()else{job.cancel.store(true,Ordering::Release);custom_input_orphan_poll(job.cancel.clone());return;};
        if !capture.current(&app){job.cancel.store(true,Ordering::Release);}
        match finish_delivery_preparation(&job.cancel){
            Ok(true)=>{poll_custom_store_effect(weak,capture,effects,revision,job);return;},
            Err(_)=>{
                if effects.borrow().save_revision==Some(revision)&&capture.binding(){effects.borrow_mut().save_retry=true;}
                return;
            },
            Ok(false)=>{},
        }
        let result=job.receiver.try_recv();
        if matches!(result,Err(TryRecvError::Empty)){poll_custom_store_effect(weak,capture,effects,revision,job);return;}
        if effects.borrow().save_revision!=Some(revision)||!capture.binding(){return;}
        let success=matches!(result,Ok(Ok(())));effects.borrow_mut().save_retry=!success;
        if !success{capture.message(&app,"Local changes are retained; retry to confirm saving","本地修改已保留，请重试确认保存");}
    });
}

pub(super) fn wire_custom_prompt_callbacks(app:&AppWindow,context:AppContext){
    let state=app.global::<AppState>();let effects=Rc::new(RefCell::new(CustomEffects::default()));
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_normalize_prompt_editor_text(move|text,_|{
            let Some(app)=weak.upgrade()else{return text;};
            let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return text;};
            let editor=text.to_string();let category=capture.target.category.clone();
            // Keystrokes only project in memory. A write is justified solely by
            // an actual selected-set change, never by unrelated saved debt.
            let changed=capture.apply(&app,||{
                app.global::<AppState>().set_prompt(editor.clone().into());
                let store=context.store.borrow();
                let selected=store.selected_custom_prompts.get(&category).cloned().unwrap_or_default();
                let retained=selected.iter().filter(|prompt|editor.contains(&inline_custom_prompt_display_text(&custom_prompt_display_name(&store,prompt)))).cloned().collect::<BTreeSet<_>>();
                if retained==selected{push_custom_prompts(&app,&store);None}else{Some(retained)}
            }).flatten();
            if let Some(retained)=changed{
                custom_flush_or_mutate(&app,&capture,&effects,CustomStoreMutation::NormalizeSelection(retained.clone()),|store|{
                    if retained.is_empty(){store.selected_custom_prompts.remove(&category);}else{store.selected_custom_prompts.insert(category,retained);}
                });
            }text
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_prepare_custom_prompt_insertion(move|text,cursor|{
            let fallback=PromptTextEdit{text:text.clone(),cursor_offset:cursor};
            let Some(app)=weak.upgrade()else{return fallback;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return fallback;};
            capture.apply(&app,||{
                let edit=remove_custom_prompt_trigger_before_cursor(text.as_str(),cursor.max(0) as usize);
                if edit.cursor_offset>=0{app.global::<AppState>().set_prompt(edit.text.clone().into());}
                slint_prompt_text_edit(edit)
            }).unwrap_or(fallback)
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_apply_inline_custom_prompt(move|prompt,text,cursor|{
            let fallback=PromptTextEdit{text:text.clone(),cursor_offset:cursor};
            let Some(app)=weak.upgrade()else{return fallback;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return fallback;};
            let category=capture.target.category.clone();
            custom_flush_or_mutate(&app,&capture,&effects,CustomStoreMutation::Inline{prompt:prompt.to_string(),input:text.to_string(),cursor},|store|{
                let name=custom_prompt_display_name(store,&prompt);
                let selected=custom_prompt_selected_for_category(store,&category,&prompt);
                let edit=if selected{remove_custom_prompt_name(&text,&name)}else{insert_custom_prompt_name_at_byte_offset(&text,cursor.max(0) as usize,&name)};
                if store.custom_prompts.contains(&prompt.to_string()){toggle_custom_prompt_selection_for_category(store,&category,&prompt);}
                app.global::<AppState>().set_prompt(edit.text.clone().into());slint_prompt_text_edit(edit)
            }).unwrap_or(fallback)
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_clear_custom_prompt_selections(move||{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            custom_flush_or_mutate(&app,&capture,&effects,CustomStoreMutation::ClearSelection,|store|{store.selected_custom_prompts.remove(&capture.target.category);});
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_toggle_custom_prompt_selection(move|prompt|{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            let weak=weak.clone();let effects=effects.clone();
            slint::Timer::single_shot(Duration::ZERO,move||{
                let Some(app)=weak.upgrade()else{return;};
                custom_flush_or_mutate(&app,&capture,&effects,CustomStoreMutation::Toggle(prompt.to_string()),|store|{
                    if !store.custom_prompts.contains(&prompt.to_string()){return;}
                    toggle_custom_prompt_selection_for_category(store,&capture.target.category,&prompt);
                    if app.global::<AppState>().get_prompt().trim()=="//"{app.global::<AppState>().set_prompt("".into());}
                });
            });
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_begin_new_custom_prompt(move||{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            let released=capture.apply(&app,||{
                let key=app.global::<AppState>().get_custom_prompt_recovered_request_id().to_string();
                invalidate_custom_input(&effects);effects.borrow_mut().sources.clear();
                reset_custom_prompt_editor(&app);app.global::<AppState>().set_custom_prompt_recovered_request_id("".into());custom_open_editor_metadata(&app);key
            });
            if let Some(key)=released.filter(|key|!key.is_empty()){
                release_custom_prompt_recovered_result_captured(&app,&context,capture.persistence,&key);
            }
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_begin_edit_custom_prompt(move|prompt|{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            let released=capture.apply(&app,||{
                let profile=context.store.borrow().custom_prompt_profiles.get(prompt.as_str()).cloned().unwrap_or_default();
                let paths=custom_prompt_profile_reference_paths(&profile);let state=app.global::<AppState>();
                let old_key=state.get_custom_prompt_recovered_request_id().to_string();
                invalidate_custom_input(&effects);effects.borrow_mut().sources.clear();
                let name=if profile.name.trim().is_empty(){prompt.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(48).collect()}else{profile.name};
                state.set_custom_prompt_name(name.into());state.set_custom_prompt_input(prompt.clone());
                state.set_custom_prompt_editor_session_id(Uuid::new_v4().to_string().into());state.set_custom_prompt_editing_original(prompt.clone());
                state.set_custom_prompt_category(normalized_custom_prompt_category(&profile.category).into());state.set_custom_prompt_format(normalized_custom_prompt_format(&profile.format).into());
                state.set_custom_prompt_negative(profile.negative_prompt.into());state.set_custom_prompt_recovered_request_id("".into());state.set_custom_prompt_analyzing(false);
                // Preserve readable legacy/missing metadata, never adopt it as owned input.
                set_custom_prompt_references(&app,paths.into_iter().map(|path|ReferenceItem{id:Uuid::new_v4().to_string().into(),source_path:path.into(),image:Image::default()}).collect());
                state.set_custom_prompt_message("".into());custom_open_editor_metadata(&app);old_key
            });
            let Some(key)=released else{return;};
            if !key.is_empty(){release_custom_prompt_recovered_result_captured(&app,&context,capture.persistence,&key);}
            if let Some(capture)=CustomEffectCapture::capture(&app,&context){
                if let Some(ticket)=begin_custom_input(&app,&capture,&effects){prepare_custom_existing_previews(&app,capture,ticket);}
            }
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_close_custom_prompt_editor(move||{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            if capture.apply(&app,||invalidate_custom_input(&effects)).is_some(){close_custom_prompt_editor(&app,&context);}
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_choose_custom_prompt_reference(move||{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            let Some(ticket)=begin_custom_input(&app,&capture,&effects)else{return;};start_custom_picker(&app,capture,ticket);
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_clear_custom_prompt_reference(move||{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            capture.apply(&app,||{invalidate_custom_input(&effects);effects.borrow_mut().sources.clear();set_custom_prompt_references(&app,Vec::new());});
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_remove_custom_prompt_reference(move|id|{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            capture.apply(&app,||{
                invalidate_custom_input(&effects);
                let rows=app.global::<AppState>().get_custom_prompt_reference_items().iter().filter(|row|row.id!=id).collect::<Vec<_>>();
                effects.borrow_mut().sources.retain(|(_,path)|rows.iter().any(|row|row.source_path.as_str()==path));set_custom_prompt_references(&app,rows);
            });
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_open_custom_prompt_reference(move|id|{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            let Some(item)=app.global::<AppState>().get_custom_prompt_reference_items().iter().find(|row|row.id==id)else{return;};
            if !capture.persistence.owns_path(Path::new(item.source_path.as_str())){
                capture.message(&app,"This saved reference is unavailable in the current namespace","此已存参考图在当前账号空间不可用");return;
            }
            let Some(ticket)=begin_custom_input(&app,&capture,&effects)else{return;};
            let path=item.source_path.to_string();let worker_path=path.clone();let id=item.id.to_string();
            let viewer_before={let state=app.global::<AppState>();(state.get_viewer_open(),state.get_viewer_id(),state.get_viewer_source(),state.get_viewer_source_path())};
            spawn_custom_input(&app,capture,ticket,move|p,activity,cancel|{
                custom_input_current(p,activity,cancel)?;
                let preview=prepare_owned_preview(p,Path::new(&worker_path),PreviewPurpose::Reference)?;
                custom_input_current(p,activity,cancel)?;Ok(preview)
            },move|app,capture,ticket,result|{
                let Ok(preview)=result else{capture.message(app,"Unable to open the saved reference","暂时无法打开已存参考图");return;};
                capture.apply(app,||{
                    if !ticket.current()||!app.global::<AppState>().get_custom_prompt_reference_items().iter().any(|row|row.id.as_str()==id&&row.source_path.as_str()==path){return;}
                    let state=app.global::<AppState>();
                    if (state.get_viewer_open(),state.get_viewer_id(),state.get_viewer_source(),state.get_viewer_source_path())!=viewer_before{return;}
                    // All pixels here come from the original held namespace source.
                    state.set_viewer_id(id.into());state.set_viewer_source("reference".into());state.set_viewer_source_path(path.into());
                    state.set_viewer_image(materialize_delivery_preview(&preview));
                    state.set_viewer_title(if state.get_language()=="en"{"Reference image"}else{"参考图"}.into());
                    state.set_viewer_prompt("".into());state.set_viewer_prompt_lines(1);state.set_viewer_time("".into());state.set_viewer_ratio("".into());
                    let(width,height)=preview.dimensions();
                    state.set_viewer_quality("".into());state.set_viewer_model("".into());state.set_viewer_width(width as i32);state.set_viewer_height(height as i32);
                    state.set_viewer_cutout_done(false);state.set_viewer_remove_black_done(false);state.set_viewer_upscale_done(false);state.set_viewer_open(true);
                });
            });
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();
        state.on_analyze_custom_prompt_reference(move||{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            if app.global::<AppState>().get_custom_prompt_analyzing(){return;}
            // This helper may dispatch authentication; never call it under a completion.
            if !require_online_operation(&app,"分析参考图风格"){
                capture.message(&app,"Image style analysis requires an internet connection","图片风格分析需要联网，请检查网络后重试");return;
            }
            let request=capture.apply(&app,||{
                let state=app.global::<AppState>();let paths=custom_prompt_reference_paths(&app).into_iter().map(PathBuf::from).collect::<Vec<_>>();
                if paths.is_empty(){custom_save_message(&app,"Upload a style reference image first","请先上传风格参考图");return None;}
                if context.backend.is_none(){custom_save_message(&app,"The model service is unavailable","模型服务暂不可用");return None;}
                if paths.iter().any(|path|!capture.persistence.owns_path(path)){custom_save_message(&app,"Select the reference again to import it safely","请重新选择参考图并安全导入");return None;}
                let selection=sync_style_analysis_selection(&state);
                if !selection.available{custom_save_message(&app,"No image style analysis model is available","服务端没有可用的图片风格分析模型");return None;}
                let english=state.get_language()=="en";
                state.set_custom_prompt_analyzing(true);custom_save_message(&app,"Analyzing the reference image...","正在分析参考图风格...");
                Some(PromptTaskRequest{
                    model_code:selection.model_code,task_type:"image_style_analysis",
                    prompt:if english{
                        "Analyze the uploaded image's visual style. Return only one concise, reusable English image-generation style description covering composition, palette, lighting, rendering medium, texture, detail, and atmosphere. Do not describe file metadata and do not add headings."
                    }else{
                        "分析上传参考图的视觉风格。只输出一段可直接复用的中文生图风格描述，覆盖构图、配色、光影、绘制媒介、纹理、细节与氛围；不要描述文件元数据，不要添加标题。"
                    }.into(),target_language:None,optimize:true,
                    target:PromptResultTarget::CustomPrompt{session_id:state.get_custom_prompt_editor_session_id().into(),input:state.get_custom_prompt_input().into(),append_result:true},
                    reference_paths:paths,
                })
            }).flatten();
            if let Some(request)=request{
                if capture.current(&app){start_backend_prompt_task(&app,context.clone(),request);}
            }
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let save_state:CustomSaveState=Rc::new(RefCell::new(None));
        state.on_save_custom_prompt(move|original,prompt|{
            let Some(app)=weak.upgrade()else{return;};
            start_custom_prompt_save(&app,context.clone(),save_state.clone(),original.to_string(),prompt.to_string());
        });
    }
    {
        let weak=app.as_weak();let context=context.clone();let effects=effects.clone();
        state.on_remove_custom_prompt(move|prompt|{
            let Some(app)=weak.upgrade()else{return;};let Some(capture)=CustomEffectCapture::capture(&app,&context)else{return;};
            custom_flush_or_mutate(&app,&capture,&effects,CustomStoreMutation::Remove(prompt.to_string()),|store|{
                let name=custom_prompt_display_name(store,&prompt);let selected=custom_prompt_selected_for_category(store,&capture.target.category,&prompt);
                if remove_custom_prompt_from_store(store,&prompt){
                    if selected{let edit=remove_custom_prompt_name(app.global::<AppState>().get_prompt().as_str(),&name);app.global::<AppState>().set_prompt(edit.text.into());}
                    app.global::<AppState>().set_custom_prompt_message("".into());
                }
            });
        });
    }
}

const MAX_CUSTOM_PROMPT_REFERENCES: usize = 8;

fn custom_prompt_profile_reference_paths(profile: &CustomPromptProfile) -> Vec<String> {
    let source = if profile.reference_paths.is_empty() {
        std::slice::from_ref(&profile.reference_path)
    } else {
        profile.reference_paths.as_slice()
    };
    source
        .iter()
        .filter(|path| !path.trim().is_empty())
        .take(MAX_CUSTOM_PROMPT_REFERENCES)
        .cloned()
        .collect()
}

fn custom_prompt_reference_paths(app: &AppWindow) -> Vec<String> {
    let model = app.global::<AppState>().get_custom_prompt_reference_items();
    (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .map(|item| item.source_path.to_string())
        .take(MAX_CUSTOM_PROMPT_REFERENCES)
        .collect()
}

fn set_custom_prompt_references(app: &AppWindow, items: Vec<ReferenceItem>) {
    let state = app.global::<AppState>();
    if let Some(first) = items.first() {
        state.set_custom_prompt_reference_path(first.source_path.clone());
        state.set_custom_prompt_reference_image(first.image.clone());
    } else {
        state.set_custom_prompt_reference_path("".into());
        state.set_custom_prompt_reference_image(Image::default());
    }
    state.set_custom_prompt_reference_items(ModelRc::new(VecModel::from(items)));
}

fn reset_custom_prompt_editor(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_custom_prompt_name("".into());
    state.set_custom_prompt_input("".into());
    state.set_custom_prompt_editor_session_id(Uuid::new_v4().to_string().into());
    state.set_custom_prompt_category("default".into());
    state.set_custom_prompt_format("json".into());
    state.set_custom_prompt_negative("".into());
    set_custom_prompt_references(app, Vec::new());
    state.set_custom_prompt_message("".into());
    state.set_custom_prompt_analyzing(false);
    state.set_custom_prompt_editing_original("".into());
}

fn open_custom_prompt_editor(app: &AppWindow) {
    let state = app.global::<AppState>();
    let current_page = state.get_page().to_string();
    if current_page != "custom-prompt-editor" {
        let return_page = match current_page.as_str() {
            "generation" | "settings" => current_page,
            _ => "settings".to_string(),
        };
        state.set_custom_prompt_editor_return_page(return_page.into());
    }
    state.set_custom_prompt_editor_open(true);
    navigate_to(app, "custom-prompt-editor");
}

pub(super) fn close_custom_prompt_editor(app: &AppWindow, context: &AppContext) {
    let Some(persistence)=captured_custom_editor(context)else{return;};
    let key=app.global::<AppState>().get_custom_prompt_recovered_request_id().to_string();
    let visuals=prepare_delivery_visuals(app,&context.store.borrow());
    let effects=context.apply_user_completion(persistence.lease(),||{
        let state=app.global::<AppState>();
        let return_page=if state.get_custom_prompt_editor_return_page()=="generation"{"generation"}else{"settings"};
        state.set_custom_prompt_editor_open(false);
        state.set_custom_prompt_editor_session_id("".into());
        state.set_custom_prompt_analyzing(false);
        state.set_custom_prompt_message("".into());
        state.set_custom_prompt_recovered_request_id("".into());
        state.set_page(return_page.into());
        visuals.publish_metadata(app,persistence.clone())
    }).ok();
    if let Some(effects)=effects{
        if !key.is_empty(){release_custom_prompt_recovered_result_captured(app,context,persistence,&key);}
        start_activation_visual_effects(app,context.clone(),effects);
    }
}

pub(super) fn normalized_custom_prompt_category(value: &str) -> String {
    match value {
        "character" | "scene" | "ui" | "effect" => value.to_string(),
        _ => "default".to_string(),
    }
}

pub(super) fn normalized_custom_prompt_format(value: &str) -> String {
    if value == "txt" {
        "txt".to_string()
    } else {
        "json".to_string()
    }
}

#[allow(dead_code)]
fn legacy_reference_style(rgba: &[u8], width: u32, height: u32, english: bool) -> Option<String> {
    let pixel_count = rgba.len() / 4;
    if pixel_count == 0 || width == 0 || height == 0 {
        return None;
    }

    let sample_step = (pixel_count / 50_000).max(1);
    let mut samples = 0_f64;
    let mut red = 0_f64;
    let mut green = 0_f64;
    let mut blue = 0_f64;
    let mut luminance = 0_f64;
    let mut luminance_squared = 0_f64;
    let mut saturation = 0_f64;

    for pixel in rgba.chunks_exact(4).step_by(sample_step) {
        if pixel[3] == 0 {
            continue;
        }
        let r = pixel[0] as f64 / 255.0;
        let g = pixel[1] as f64 / 255.0;
        let b = pixel[2] as f64 / 255.0;
        let maximum = r.max(g).max(b);
        let minimum = r.min(g).min(b);
        let value = 0.2126 * r + 0.7152 * g + 0.0722 * b;

        samples += 1.0;
        red += r;
        green += g;
        blue += b;
        luminance += value;
        luminance_squared += value * value;
        saturation += if maximum <= f64::EPSILON {
            0.0
        } else {
            (maximum - minimum) / maximum
        };
    }

    if samples == 0.0 {
        return None;
    }

    let average_red = red / samples;
    let average_green = green / samples;
    let average_blue = blue / samples;
    let average_luminance = luminance / samples;
    let average_saturation = saturation / samples;
    let variance = (luminance_squared / samples - average_luminance * average_luminance).max(0.0);
    let contrast = variance.sqrt();
    let warm_balance = average_red - average_blue + (average_green - average_blue) * 0.12;

    let orientation = if width > height.saturating_mul(6) / 5 {
        if english {
            "landscape"
        } else {
            "横向"
        }
    } else if height > width.saturating_mul(6) / 5 {
        if english {
            "portrait"
        } else {
            "竖向"
        }
    } else if english {
        "square"
    } else {
        "方形"
    };
    let brightness = if average_luminance > 0.68 {
        if english {
            "bright and airy"
        } else {
            "明亮通透"
        }
    } else if average_luminance < 0.34 {
        if english {
            "deep low-key lighting"
        } else {
            "低调暗部"
        }
    } else if english {
        "balanced lighting"
    } else {
        "明暗均衡"
    };
    let temperature = if warm_balance > 0.07 {
        if english {
            "warm palette"
        } else {
            "暖色调"
        }
    } else if warm_balance < -0.06 {
        if english {
            "cool palette"
        } else {
            "冷色调"
        }
    } else if english {
        "neutral palette"
    } else {
        "中性色调"
    };
    let chroma = if average_saturation > 0.55 {
        if english {
            "vivid saturated color"
        } else {
            "色彩高饱和鲜明"
        }
    } else if average_saturation < 0.20 {
        if english {
            "soft restrained color"
        } else {
            "色彩低饱和柔和"
        }
    } else if english {
        "natural color saturation"
    } else {
        "色彩饱和度自然"
    };
    let tonal_contrast = if contrast > 0.24 {
        if english {
            "strong tonal contrast"
        } else {
            "强对比光影"
        }
    } else if contrast < 0.11 {
        if english {
            "soft low contrast"
        } else {
            "柔和低对比光影"
        }
    } else if english {
        "balanced tonal contrast"
    } else {
        "均衡对比光影"
    };
    let detail = if width.max(height) >= 2_000 {
        if english {
            "fine detailed texture"
        } else {
            "细节与纹理丰富"
        }
    } else if english {
        "clean controlled detail"
    } else {
        "细节简洁克制"
    };

    Some(if english {
        format!(
            "Reference style: {orientation} composition, {brightness}, {temperature}, \
             {chroma}, {tonal_contrast}, {detail}."
        )
    } else {
        format!(
            "参考图风格：{orientation}构图，{brightness}，{temperature}，{chroma}，\
             {tonal_contrast}，{detail}。"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CustomWorkerDrain;
    impl Drop for CustomWorkerDrain {
        fn drop(&mut self) {
            let prompt=shutdown_prompt_workers();
            let delivery=drain_delivery_commit_workers_for_shutdown();
            let preview=drain_activation_preview_workers_for_shutdown();
            if !std::thread::panicking(){
                assert!(prompt.is_ok());assert!(delivery.is_ok());assert!(preview.is_ok());
            }
        }
    }
    #[test]
    fn core_custom_save_failed_sqlite_retries_staged_new_and_edited_prompt_without_duplicate_or_missing() {
        assert_custom_save_failed_sqlite_retries_staged_prompt(false);
    }
    #[test]
    fn core_custom_save_failed_sqlite_retries_staged_edited_prompt_without_missing() {
        assert_custom_save_failed_sqlite_retries_staged_prompt(true);
    }
    fn assert_custom_save_failed_sqlite_retries_staged_prompt(edited:bool) {
        i_slint_backend_testing::init_no_event_loop();
        {
            let fixture=video_image_callbacks::tests::scoped_inputs::Fixture::new();
            let _drain=CustomWorkerDrain;
            let app=AppWindow::new().unwrap();wire_custom_prompt_callbacks(&app,fixture.context.clone());
            let state=app.global::<AppState>();
            state.set_language("en".into());state.set_custom_prompt_editor_open(true);
            state.set_custom_prompt_editor_session_id("retry-editor".into());
            state.set_custom_prompt_name("Saved name".into());state.set_custom_prompt_input("new body".into());
            let original=if edited{"original body"}else{""};
            state.set_custom_prompt_editing_original(original.into());
            if edited {fixture.context.store.borrow_mut().custom_prompts.push(original.into());}
            let initial=local_store_data(&app,&fixture.context.store.borrow());
            std::thread::scope(|workers|workers.spawn(||fixture.persistence.save_store(initial)).join().unwrap().unwrap());
            fixture.writer.reject_custom_prompt_inserts_for_test(true);
            state.invoke_save_custom_prompt(original.into(),"new body".into());
            video_image_callbacks::tests::scoped_inputs::pump(||state.get_custom_prompt_message().starts_with("Unable to confirm"));
            assert!(state.get_custom_prompt_editor_open());
            assert_eq!(fixture.context.store.borrow().custom_prompts.iter().filter(|body|*body=="new body").count(),1);
            // This edit did not belong to the failed save and must survive its retry.
            fixture.context.store.borrow_mut().custom_prompts.push("later unrelated body".into());
            fixture.writer.reject_custom_prompt_inserts_for_test(false);
            state.invoke_save_custom_prompt(original.into(),"new body".into());
            video_image_callbacks::tests::scoped_inputs::pump(||!state.get_custom_prompt_editor_open());
            let durable=fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).unwrap().unwrap();
            assert_eq!(durable.custom_prompts.iter().filter(|body|*body=="new body").count(),1);
            assert!(durable.custom_prompts.iter().any(|body|body=="later unrelated body"));
            assert!(!durable.custom_prompts.iter().any(|body|body=="original body"));
            assert_eq!(durable.custom_prompt_profiles["new body"].name,"Saved name");
        }
    }
    #[test]
    fn core_custom_save_ack_preserves_name_and_profile_only_edits_in_same_editor() {
        assert_custom_save_ack_preserves_editor_edits(false);
    }
    #[test]
    fn core_custom_save_ack_preserves_profile_only_edits_in_same_editor() {
        assert_custom_save_ack_preserves_editor_edits(true);
    }
    fn assert_custom_save_ack_preserves_editor_edits(profile_only:bool) {
        i_slint_backend_testing::init_no_event_loop();
        {
            let fixture=video_image_callbacks::tests::scoped_inputs::Fixture::new();
            let _drain=CustomWorkerDrain;
            let app=AppWindow::new().unwrap();wire_custom_prompt_callbacks(&app,fixture.context.clone());
            let state=app.global::<AppState>();state.set_custom_prompt_editor_open(true);
            state.set_custom_prompt_editor_session_id("unchanged-editor-id".into());
            state.set_custom_prompt_name("original name".into());state.set_custom_prompt_input("unchanged body".into());
            let changed=Rc::new(Cell::new(false));let observed=changed.clone();let weak=app.as_weak();
            CUSTOM_SAVE_AFTER_ACK.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{
                let app=weak.upgrade().unwrap();let state=app.global::<AppState>();
                if profile_only {
                    state.set_custom_prompt_format("txt".into());
                    state.set_custom_prompt_negative("new negative edit".into());
                    state.set_custom_prompt_category("scene".into());
                } else {state.set_custom_prompt_name("new name edit".into());}
                changed.set(true);
            })));
            state.invoke_save_custom_prompt("".into(),"unchanged body".into());
            video_image_callbacks::tests::scoped_inputs::pump(||observed.get());
            assert!(state.get_custom_prompt_editor_open());
            assert_eq!(state.get_custom_prompt_editor_session_id(),"unchanged-editor-id");
            assert_eq!(state.get_custom_prompt_input(),"unchanged body");
            if profile_only {
                assert_eq!(state.get_custom_prompt_format(),"txt");
                assert_eq!(state.get_custom_prompt_negative(),"new negative edit");
                assert_eq!(state.get_custom_prompt_category(),"scene");
            } else {assert_eq!(state.get_custom_prompt_name(),"new name edit");}
            let durable=fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).unwrap().unwrap();
            assert_eq!(durable.custom_prompt_profiles["unchanged body"].name,"original name");
            assert_ne!(durable.custom_prompt_profiles["unchanged body"].negative_prompt,"new negative edit");
        }
    }
    #[test]
    fn core_custom_save_ack_keeps_original_recovery_identity_when_editor_changes() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture=video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let _drain=CustomWorkerDrain;
        let app=AppWindow::new().unwrap();
        wire_custom_prompt_callbacks(&app,fixture.context.clone());
        let row=|key:&str,result:&str|serde_json::json!({
            "schema_version":2,"client_request_id":key,"owner_user_id":fixture.persistence.lease().namespace.user_public_id(),
            "billing_account_group_id":"33333333-3333-4333-8333-333333333333",
            "auth_epoch":fixture.persistence.lease().auth_epoch,
            "server_task_id":"55555555-5555-4555-8555-555555555555",
            "task_type":"prompt_optimize","model_code":"fixture-model","prompt":"input",
            "target_kind":"custom_prompt","target_id":"original-editor",
            "result_prompt":result,"applied_to_target":true
        });
        let first="44444444-4444-4444-8444-444444444444";
        let second="66666666-6666-4666-8666-666666666666";
        let document=serde_json::json!({"schema_version":2,"prompt_tasks":[row(first,"paid A"),row(second,"paid B")],"deep_optimizations":[]});
        let key=ManagedFileKey::new(ManagedUserArea::Recovery,"pending-prompt-tasks.json").unwrap();
        let mut file=fixture.authority.create_new_regular(&key).unwrap();
        fixture.authority.write_new_regular_from(&mut file,&mut serde_json::to_vec(&document).unwrap().as_slice()).unwrap();
        fixture.authority.sync_regular(&mut file).unwrap();
        let state=app.global::<AppState>();
        state.set_custom_prompt_editor_open(true);
        state.set_custom_prompt_editor_session_id("original-editor".into());
        state.set_custom_prompt_recovered_request_id(first.into());
        state.set_custom_prompt_name("Saved A".into());
        state.set_custom_prompt_input("paid A".into());
        let weak=app.as_weak();
        CUSTOM_SAVE_AFTER_ACK.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{
            let app=weak.upgrade().unwrap();let state=app.global::<AppState>();
            state.set_custom_prompt_editor_session_id("later-editor".into());
            state.set_custom_prompt_recovered_request_id(second.into());
            state.set_custom_prompt_input("paid B".into());
        })));
        state.invoke_save_custom_prompt("".into(),"paid A".into());
        video_image_callbacks::tests::scoped_inputs::pump(||load_pending_prompt_tasks_for_namespace(&fixture.authority).unwrap().len()==1);
        let retained=load_pending_prompt_tasks_for_namespace(&fixture.authority).unwrap();
        assert_eq!(retained[0].client_request_id,second,"acknowledgment must retain the newly opened result");
        assert_eq!(state.get_custom_prompt_editor_session_id(),"later-editor");
        assert_eq!(state.get_custom_prompt_input(),"paid B");
        assert!(state.get_custom_prompt_editor_open());
        let durable=fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).unwrap().unwrap();
        assert!(durable.custom_prompts.iter().any(|prompt|prompt=="paid A"));
        assert!(!durable.custom_prompts.iter().any(|prompt|prompt=="paid B"));
    }

    #[test]
    fn core_custom_editor_save_and_close_reject_missing_store_before_private_projection() {
        i_slint_backend_testing::init_no_event_loop();
        for close in [false,true] {
            let fixture=video_image_callbacks::tests::scoped_inputs::Fixture::new();
            let app=AppWindow::new().unwrap();
            wire_custom_prompt_callbacks(&app,fixture.context.clone());
            let state=app.global::<AppState>();
            state.set_custom_prompt_editor_open(true);
            state.set_custom_prompt_editor_session_id("original-editor".into());
            state.set_custom_prompt_name("Original name".into());
            state.set_custom_prompt_input("Original paid content".into());
            state.set_custom_prompt_message("unchanged boundary".into());
            fixture.context.store.borrow_mut().private_persistence=None;
            if close {state.invoke_close_custom_prompt_editor();}
            else {state.invoke_save_custom_prompt("".into(),"Original paid content".into());}
            assert!(fixture.context.store.borrow().custom_prompts.is_empty());
            assert!(state.get_custom_prompt_editor_open());
            assert_eq!(state.get_custom_prompt_editor_session_id(),"original-editor");
            assert_eq!(state.get_custom_prompt_message(),"unchanged boundary");
            fixture.drain();
        }
    }

    #[test]
    fn core_custom_editor_save_and_close_reject_exact_upgrade_without_late_reset() {
        i_slint_backend_testing::init_no_event_loop();
        for close in [false,true] {
            let fixture=video_image_callbacks::tests::scoped_inputs::Fixture::new();
            let app=AppWindow::new().unwrap();
            wire_custom_prompt_callbacks(&app,fixture.context.clone());
            let state=app.global::<AppState>();
            state.set_custom_prompt_editor_open(true);
            state.set_custom_prompt_editor_session_id("original-editor".into());
            state.set_custom_prompt_name("Original name".into());
            state.set_custom_prompt_input("Original paid content".into());
            state.set_custom_prompt_message("upgrade boundary".into());
            fixture.persistence.upgrade_latch().trip(RequiredUpgrade{minimum_version:None});
            if close {state.invoke_close_custom_prompt_editor();}
            else {state.invoke_save_custom_prompt("".into(),"Original paid content".into());}
            assert!(fixture.context.store.borrow().custom_prompts.is_empty());
            assert!(state.get_custom_prompt_editor_open());
            assert_eq!(state.get_custom_prompt_editor_session_id(),"original-editor");
            assert_eq!(state.get_custom_prompt_input(),"Original paid content");
            assert_eq!(state.get_custom_prompt_message(),"upgrade boundary");
            fixture.drain();
        }
    }

    #[test]
    fn custom_prompt_save_failure_never_acknowledges_recovered_result() {
        let acknowledged = std::cell::Cell::new(false);

        let result = persist_custom_prompt_before_ack(
            || Err(anyhow!("disk full")),
            || acknowledged.set(true),
        );

        assert!(result.is_err());
        assert!(!acknowledged.get());
    }

    #[test]
    fn custom_prompt_recovery_is_acknowledged_after_durable_save() {
        let events = RefCell::new(Vec::new());

        persist_custom_prompt_before_ack(
            || {
                events.borrow_mut().push("durable_save");
                Ok(())
            },
            || events.borrow_mut().push("acknowledge"),
        )
        .unwrap();

        assert_eq!(*events.borrow(), vec!["durable_save", "acknowledge"]);
    }

    #[test]
    fn custom_prompt_name_is_inserted_at_the_utf8_cursor_position() {
        let edit = insert_custom_prompt_name_at_byte_offset("古城，夜景", 9, "像素模板");

        assert_eq!(edit.text, "古城，\u{3000}像素模板 夜景");
        assert_eq!(edit.cursor_offset, 25);
    }

    #[test]
    fn custom_prompt_trigger_is_removed_at_a_middle_cursor_position() {
        let edit = remove_custom_prompt_trigger_before_cursor("前文 /后文", 8);

        assert_eq!(edit.text, "前文 后文");
        assert_eq!(edit.cursor_offset, 7);
    }

    #[test]
    fn inline_custom_prompt_occurrences_follow_their_text_order() {
        let replacements = vec![
            ("像素模板".to_string(), "PIXEL CONTENT".to_string()),
            ("镜头模板".to_string(), "CAMERA CONTENT".to_string()),
        ];

        let occurrences = inline_custom_prompt_occurrences(
            "前文 \u{3000}镜头模板 中段 \u{3000}像素模板 后文",
            &replacements,
        );

        assert_eq!(occurrences.len(), 2);
        assert_eq!(occurrences[0].name, "镜头模板");
        assert_eq!(occurrences[0].prefix, "前文 ");
        assert_eq!(occurrences[1].name, "像素模板");
        assert_eq!(occurrences[1].prefix, "前文 \u{3000}镜头模板 中段 ");
    }

    #[test]
    fn removing_one_inline_custom_prompt_preserves_the_other_text() {
        let edit = remove_custom_prompt_name(
            "前文 \u{3000}像素模板 中段 \u{3000}镜头模板 后文",
            "像素模板",
        );

        assert_eq!(edit.text, "前文 中段 \u{3000}镜头模板 后文");
        assert_eq!(edit.cursor_offset, 7);
    }

    #[test]
    fn removing_an_edge_inline_custom_prompt_does_not_leave_padding() {
        assert_eq!(
            remove_custom_prompt_name("\u{3000}像素模板 后文", "像素模板").text,
            "后文"
        );
        assert_eq!(
            remove_custom_prompt_name("前文 \u{3000}像素模板", "像素模板").text,
            "前文"
        );
    }

    #[test]
    fn renaming_an_inline_custom_prompt_preserves_its_position_and_other_names() {
        let edit = replace_custom_prompt_name(
            "前文 \u{3000}旧名称 中段 \u{3000}镜头模板 后文",
            "旧名称",
            "新名称",
        );

        assert_eq!(edit.text, "前文 \u{3000}新名称 中段 \u{3000}镜头模板 后文");
        assert_eq!(edit.cursor_offset, 19);
    }
}

#[cfg(test)]
mod core_custom_effect_tests{
    use super::*;
    use std::io::{Read,Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool,Ordering};
    const OWNER:&str="11111111-1111-4111-8111-111111111111";
    const GROUP:&str="22222222-2222-4222-8222-222222222222";
    struct Fixture{inner:video_image_callbacks::tests::scoped_inputs::Fixture,external:tempfile::TempDir,expected_delivery_failure:bool}
    impl std::ops::Deref for Fixture{
        type Target=video_image_callbacks::tests::scoped_inputs::Fixture;
        fn deref(&self)->&Self::Target{&self.inner}
    }
    fn fixture(url:Option<&str>)->(Fixture,AppWindow){
        i_slint_backend_testing::init_no_event_loop();
        let mut inner=video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let external=tempfile::tempdir().unwrap();
        CUSTOM_REFERENCE_TEST_PREVIEW_ROOT.with(|root|*root.borrow_mut()=Some(external.path().to_path_buf()));
        if let Some(url)=url{
            let backend=Arc::new(BackendRuntime{api:ApiClient::new(ApiClientConfig{
                base_url:reqwest::Url::parse(url).unwrap(),app_version:"999.0.0".into(),timeout:Duration::from_secs(2),
            },DeviceIdentity{id:Uuid::new_v4().to_string(),name:"custom-effects-fixture".into(),platform:"macos".into()},
                inner.context.backend.as_ref().unwrap().api.session().clone()).unwrap()});
            backend.api.bind_user_work(UserWorkAdmission::new(inner.context.active_namespace.clone(),inner.context.user_activity.clone())).unwrap();
            let p=PrivatePersistence::for_test_with_storage((*inner.writer).clone(),inner.persistence.lease().clone(),
                inner.context.user_activity.clone(),backend.api.upgrade_latch().clone(),inner.context.data_root_capability.clone().unwrap(),
                backend.api.clone(),inner.context.file_index.clone().unwrap());
            inner.context.backend=Some(backend);inner.context.store.borrow_mut().private_persistence=Some(p.clone());
            inner.authority=p.storage_authority().unwrap();inner.persistence=p;
        }
        let transition=inner.context.namespace_operations.try_begin_transition().unwrap();
        let recovery=transition.begin_prepublication_recovery(inner.persistence.lease()).unwrap();
        recovery.verify_no_unsupported_imports(&inner.authority).unwrap();
        let recovered=recovery.finish().unwrap();transition.prepare_publication(inner.persistence.lease(),recovered).unwrap().publish();
        let app=AppWindow::new().unwrap();let state=app.global::<AppState>();
        state.set_language("en".into());state.set_page("custom-prompt-editor".into());state.set_session_state("online".into());state.set_logged_in(true);
        state.set_asset_type("character".into());state.set_custom_prompt_editor_open(true);
        state.set_custom_prompt_editor_session_id("original-editor".into());state.set_custom_prompt_editor_return_page("generation".into());
        state.set_custom_prompt_name("Original name".into());state.set_custom_prompt_input("Original body".into());
        state.set_custom_prompt_message("original status".into());
        wire_custom_prompt_callbacks(&app,inner.context.clone());
        inner.persistence.save_store(local_store_data(&app,&inner.context.store.borrow())).unwrap();
        (Fixture{inner,external,expected_delivery_failure:false},app)
    }
    impl Drop for Fixture{fn drop(&mut self){
        {
            let mut active=self.context.active_namespace.lock().unwrap_or_else(|error|error.into_inner());
            if active.as_ref()==Some(self.persistence.lease()){*active=None;}
        }
        // Every cleanup runs before any assertion; no temporary root may outlive I/O.
        let prompt=shutdown_prompt_workers();
        let delivery=drain_delivery_commit_workers_for_shutdown();
        let previews=drain_activation_preview_workers_for_shutdown();
        let retired=self.context.user_activity.begin_quiesce(self.persistence.lease());
        let retired=match retired{Ok(guard)=>{guard.retire();Ok(())},Err(error)=>Err(error)};
        if !std::thread::panicking(){prompt.unwrap();previews.unwrap();retired.unwrap();if self.expected_delivery_failure{assert!(delivery.is_err());}else{delivery.unwrap();}}
    }}
    fn image_file(f:&Fixture,name:&str,color:[u8;4])->PathBuf{
        let path=f.external.path().join(name);
        let image=image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(80,80,image::Rgba(color)));
        let mut bytes=std::io::Cursor::new(Vec::new());image.write_to(&mut bytes,image::ImageFormat::Png).unwrap();
        std::fs::write(&path,bytes.into_inner()).unwrap();path
    }
    fn rows(app:&AppWindow)->Vec<ReferenceItem>{app.global::<AppState>().get_custom_prompt_reference_items().iter().collect()}
    fn pump_for(duration:Duration){
        let end=Instant::now()+duration;
        while Instant::now()<end{
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn pump_until(mut ready:impl FnMut()->bool){
        let end=Instant::now()+Duration::from_secs(6);
        while !ready()&&Instant::now()<end{pump_for(Duration::from_millis(5));}
        assert!(ready(),"custom effect did not reach the required actual state");
    }
    fn choose(paths:Vec<PathBuf>){CUSTOM_REFERENCE_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|done|done(Some(paths)))));}
    fn held_picker()->Rc<RefCell<Option<CustomReferencePickerCompletion>>>{
        let pending=Rc::new(RefCell::new(None));let capture=pending.clone();
        CUSTOM_REFERENCE_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|done|*capture.borrow_mut()=Some(done))));pending
    }
    fn complete(pending:&Rc<RefCell<Option<CustomReferencePickerCompletion>>>,paths:Vec<PathBuf>){
        let done=pending.borrow_mut().take().expect("actual picker was not reached");done(Some(paths));
    }
    fn publish_group(f:&Fixture){
        let manager=&f.context.billing_context;let session=f.context.current_account_session_scope().unwrap();
        manager.bind_authenticated_session(session.clone()).unwrap();
        let ticket=manager.begin_switch(&session,"custom-device",GROUP,PreviousBillingAuthority::StillValid).unwrap();
        let snapshot:AccountSnapshot=serde_json::from_value(serde_json::json!({
            "user":{"id":OWNER,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
            "read_only":false,"capabilities":["bill"],"membership":null,"entitlement":{},"credits":null,"quota":null,
            "billing_group":{"group_id":GROUP,"name":"fixture","group_status":"active","role":"owner","member_id":null,"relationship_status":null,
                "readable_context":true,"selectable":true,"group_version":"1","membership_version":null,"capabilities":["bill"],"quota":null}
        })).unwrap();
        let staged=manager.stage_confirmation(&ticket,snapshot.billing_group.clone(),snapshot).unwrap();
        f.writer.save_selected_group(OWNER,"custom-device",GROUP).unwrap();manager.publish_persisted(ticket,staged);
    }
    fn set_model(app:&AppWindow){
        app.global::<AppState>().set_catalog_models(ModelRc::new(VecModel::from(vec![CatalogModelView{
            code:"fixture-style-model".into(),name:"Fixture style".into(),purpose:"prompt_processing".into(),version:1,
            capabilities:"".into(),pricing:"".into(),price_1k:0,price_2k:0,price_4k:0,price_standard:"13".into(),
            supports_image_edit:false,supports_style_analysis:true,
            video_price_480:"".into(),video_price_720:"".into(),video_price_1080:"".into(),
        }])));
        app.global::<AppState>().set_reasoning_model("fixture-style-model".into());
    }
    struct UploadFixture{url:String,seen:Arc<AtomicBool>,stop:Arc<AtomicBool>,worker:Option<std::thread::JoinHandle<()>>}
    impl UploadFixture{
        fn new()->Self{
            let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
            let url=format!("http://{}/",listener.local_addr().unwrap());let seen=Arc::new(AtomicBool::new(false));
            let stop=Arc::new(AtomicBool::new(false));let cancelled=stop.clone();let observed=seen.clone();
            let worker=std::thread::spawn(move||{
                let deadline=Instant::now()+Duration::from_secs(8);
                while !cancelled.load(Ordering::Acquire)&&Instant::now()<deadline{
                    let(mut stream,_)=match listener.accept(){Ok(value)=>value,Err(error)if error.kind()==std::io::ErrorKind::WouldBlock=>{
                        std::thread::sleep(Duration::from_millis(2));continue;
                    },Err(_)=>panic!("controlled custom accept failed")};
                    stream.set_nonblocking(false).unwrap();stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut request=Vec::new();let mut chunk=[0u8;2048];
                    loop{
                        let size=match stream.read(&mut chunk){Ok(0)if request.is_empty()=>break,Ok(size)=>size,Err(_)if request.is_empty()=>break,Err(_)=>panic!("controlled custom request incomplete")};
                        assert!(size>0,"custom request ended before its body");request.extend_from_slice(&chunk[..size]);
                        assert!(request.len()<=1024*1024,"custom fixture request too large");
                        if let Some(end)=request.windows(4).position(|bytes|bytes==b"\r\n\r\n"){
                            let headers=String::from_utf8_lossy(&request[..end]);
                            let length=headers.lines().find_map(|line|line.to_ascii_lowercase().strip_prefix("content-length:").map(|value|value.trim().parse::<usize>().unwrap())).unwrap_or(0);
                            assert!(end+4+length<=1024*1024);if request.len()>=end+4+length{break;}
                        }
                    }
                    if request.is_empty(){continue;}
                    let end=request.windows(4).position(|bytes|bytes==b"\r\n\r\n").unwrap();let headers=String::from_utf8_lossy(&request[..end]);
                    assert!(headers.starts_with("POST /v1/uploads/references "),"unexpected custom endpoint");
                    assert!(!headers.to_ascii_lowercase().contains("x-account-group-id:"));
                    observed.store(true,Ordering::Release);
                    let body=br#"{"request_id":"fixture-private","data":null,"error":{"code":"account_group_not_selectable","message":"private refusal","details":null},"meta":null}"#;
                    write!(stream,"HTTP/1.1 403 Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                    stream.write_all(body).unwrap();return;
                }
            });Self{url,seen,stop,worker:Some(worker)}
        }
        fn finish(&mut self){self.stop.store(true,Ordering::Release);if let Some(worker)=self.worker.take(){worker.join().unwrap();}}
    }
    impl Drop for UploadFixture{fn drop(&mut self){
        self.stop.store(true,Ordering::Release);if let Some(worker)=self.worker.take(){let joined=worker.join();if !std::thread::panicking(){joined.unwrap();}}
    }}
    #[test]
    fn core_custom_picker_import_is_owned_before_selected_model_analysis_and_profile_ack(){
        let mut server=UploadFixture::new();let(f,app)=fixture(Some(&server.url));publish_group(&f);set_model(&app);
        let source=image_file(&f,"selected.png",[12,34,56,255]);let original=std::fs::read(&source).unwrap();
        choose(vec![source.clone()]);app.global::<AppState>().invoke_choose_custom_prompt_reference();
        pump_until(||rows(&app).len()==1);let selected=PathBuf::from(rows(&app)[0].source_path.to_string());
        assert!(selected.starts_with(f.persistence.lease().namespace.root()),"picker published a raw external path instead of an owned reference");
        assert_ne!(source,selected);let bytes=f.authority.read_image_source(&selected,100*1024*1024).unwrap();
        assert_eq!(decode_reference_bytes(&bytes).unwrap().to_rgba8().get_pixel(0,0).0,[12,34,56,255]);
        let relative=selected.strip_prefix(f.persistence.lease().namespace.root().join("references/library")).unwrap();
        assert!(f.context.file_index.as_ref().unwrap().find_file_by_path_for_namespace(&f.authority,ManagedUserArea::ReferencesLibrary,relative.to_str().unwrap()).unwrap().is_some());
        app.global::<AppState>().invoke_analyze_custom_prompt_reference();
        pump_until(||server.seen.load(Ordering::Acquire)&&!app.global::<AppState>().get_custom_prompt_analyzing());server.finish();
        let retained=load_pending_prompt_tasks_for_namespace(&f.authority).unwrap();assert_eq!(retained.len(),1);
        assert_eq!(retained[0].model_code,"fixture-style-model");assert_eq!(retained[0].billing_account_group_id,GROUP);
        assert_eq!(retained[0].task_type,"image_style_analysis");assert_eq!(retained[0].reference_paths,vec![selected.to_string_lossy().into_owned()]);
        assert!(!app.global::<AppState>().get_custom_prompt_message().contains("private refusal"));
        app.global::<AppState>().invoke_save_custom_prompt("".into(),"Original body".into());
        pump_until(||!app.global::<AppState>().get_custom_prompt_editor_open());
        let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(durable.custom_prompt_profiles["Original body"].reference_paths,vec![selected.to_string_lossy().into_owned()]);
        assert_eq!(std::fs::read(&source).unwrap(),original);
    }
    #[test]
    fn core_custom_picker_missing_binding_refuses_before_native_dialog(){
        let(f,app)=fixture(None);f.context.store.borrow_mut().private_persistence=None;
        let calls=Rc::new(Cell::new(0));let observed=calls.clone();
        CUSTOM_REFERENCE_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|done|{observed.set(observed.get()+1);done(None)})));
        app.global::<AppState>().invoke_choose_custom_prompt_reference();
        assert_eq!(calls.get(),0,"missing original binding still opened native picker");assert_eq!(app.global::<AppState>().get_custom_prompt_message(),"original status");
    }
    #[test]
    fn core_custom_picker_upgrade_refuses_before_native_dialog(){
        let(f,app)=fixture(None);f.persistence.upgrade_latch().trip(RequiredUpgrade{minimum_version:None});
        let calls=Rc::new(Cell::new(0));let observed=calls.clone();
        CUSTOM_REFERENCE_TEST_PICKER.with(|hook|*hook.borrow_mut()=Some(Box::new(move|done|{observed.set(observed.get()+1);done(None)})));
        app.global::<AppState>().invoke_choose_custom_prompt_reference();
        assert_eq!(calls.get(),0,"exact426 still opened native picker");assert_eq!(app.global::<AppState>().get_custom_prompt_message(),"original status");
    }
    #[test]
    fn core_custom_analysis_upgrade_keeps_early_private_status(){
        let(f,app)=fixture(None);f.persistence.upgrade_latch().trip(RequiredUpgrade{minimum_version:None});
        app.global::<AppState>().invoke_analyze_custom_prompt_reference();
        assert_eq!(app.global::<AppState>().get_custom_prompt_message(),"original status");assert!(!app.global::<AppState>().get_custom_prompt_analyzing());
    }
    #[test]
    fn core_custom_old_picker_cannot_populate_replaced_editor(){
        let(f,app)=fixture(None);let path=image_file(&f,"old.png",[1,2,3,255]);let pending=held_picker();
        app.global::<AppState>().invoke_choose_custom_prompt_reference();
        app.global::<AppState>().set_custom_prompt_editor_session_id("later-editor".into());app.global::<AppState>().set_custom_prompt_input("later input".into());
        complete(&pending,vec![path]);pump_for(Duration::from_millis(120));
        assert!(rows(&app).is_empty(),"original picker populated a later editor");assert_eq!(app.global::<AppState>().get_custom_prompt_input(),"later input");
    }
    #[test]
    fn core_custom_clear_supersedes_pending_picker_without_late_append(){
        let(f,app)=fixture(None);let path=image_file(&f,"old.png",[1,2,3,255]);let pending=held_picker();
        app.global::<AppState>().invoke_choose_custom_prompt_reference();app.global::<AppState>().invoke_clear_custom_prompt_reference();
        complete(&pending,vec![path]);pump_for(Duration::from_millis(120));
        assert!(rows(&app).is_empty(),"clear did not invalidate pending input");
    }
    #[test]
    fn core_custom_new_picker_supersedes_older_result_in_same_editor(){
        let(f,app)=fixture(None);let old=image_file(&f,"old.png",[1,2,3,255]);let new=image_file(&f,"new.png",[90,80,70,255]);
        let pending=held_picker();app.global::<AppState>().invoke_choose_custom_prompt_reference();
        choose(vec![new]);app.global::<AppState>().invoke_choose_custom_prompt_reference();pump_until(||rows(&app).len()==1);
        let latest=rows(&app)[0].source_path.clone();complete(&pending,vec![old]);pump_for(Duration::from_millis(120));
        assert_eq!(rows(&app).len(),1,"superseded native completion appended an old source");assert_eq!(rows(&app)[0].source_path,latest);
    }
    #[test]
    fn core_custom_retired_picker_keeps_original_private_state_empty(){
        let(f,app)=fixture(None);let path=image_file(&f,"old.png",[1,2,3,255]);let pending=held_picker();
        app.global::<AppState>().invoke_choose_custom_prompt_reference();
        *f.context.active_namespace.lock().unwrap()=None;f.context.store.borrow_mut().private_persistence=None;
        complete(&pending,vec![path]);pump_for(Duration::from_millis(120));
        assert!(rows(&app).is_empty());assert_eq!(app.global::<AppState>().get_custom_prompt_message(),"original status");
    }
    #[test]
    fn core_custom_begin_edit_retains_missing_legacy_reference_metadata_without_adoption(){
        let(f,app)=fixture(None);let missing=f.external.path().join("missing-legacy.png").to_string_lossy().into_owned();
        {
            let mut store=f.context.store.borrow_mut();store.custom_prompts.push("saved body".into());
            store.custom_prompt_profiles.insert("saved body".into(),CustomPromptProfile{name:"Saved".into(),category:"default".into(),format:"txt".into(),
                negative_prompt:String::new(),reference_path:missing.clone(),reference_paths:vec![missing.clone()]});
        }
        app.global::<AppState>().invoke_begin_edit_custom_prompt("saved body".into());pump_for(Duration::from_millis(100));
        assert_eq!(rows(&app).len(),1,"missing saved references vanished from editable metadata");assert_eq!(rows(&app)[0].source_path,missing);
        assert!(!Path::new(&missing).exists());assert_eq!(f.context.store.borrow().custom_prompt_profiles["saved body"].reference_paths,vec![missing]);
    }
    #[test]
    fn core_custom_deferred_selection_cannot_mutate_replacement_binding(){
        let(f,app)=fixture(None);f.context.store.borrow_mut().custom_prompts.push("shared body".into());
        app.global::<AppState>().set_page("generation".into());app.global::<AppState>().invoke_toggle_custom_prompt_selection("shared body".into());
        let backend=f.context.backend.as_ref().unwrap();
        let replacement=PrivatePersistence::for_test_with_storage((*f.writer).clone(),f.persistence.lease().clone(),f.context.user_activity.clone(),
            backend.api.upgrade_latch().clone(),f.context.data_root_capability.clone().unwrap(),backend.api.clone(),f.context.file_index.clone().unwrap());
        assert!(!replacement.same_binding_metadata(&f.persistence));f.context.store.borrow_mut().private_persistence=Some(replacement);
        pump_for(Duration::from_millis(100));
        assert!(f.context.store.borrow().selected_custom_prompts.is_empty(),"old zero-timer modified replacement Store authority");
        assert!(f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap().selected_custom_prompts.is_empty());
    }
    #[test]
    fn core_custom_remove_after_upgrade_preserves_store_and_editor(){
        let(f,app)=fixture(None);f.context.store.borrow_mut().custom_prompts.push("saved body".into());
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        f.persistence.upgrade_latch().trip(RequiredUpgrade{minimum_version:None});app.global::<AppState>().invoke_remove_custom_prompt("saved body".into());
        assert!(f.context.store.borrow().custom_prompts.iter().any(|body|body=="saved body"));
        assert_eq!(app.global::<AppState>().get_custom_prompt_message(),"original status");
        assert!(f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap().custom_prompts.iter().any(|body|body=="saved body"));
    }

    // Stage02 lifecycle tests: use the shared real worker's post-send boundary,
    // not an independent fake executor. Drop releases every hold before Fixture
    // drains/joins and retires its namespace.
    struct HeldCustomWorker{
        arrived:mpsc::Receiver<()>,release:Option<mpsc::Sender<()>>,
    }
    impl HeldCustomWorker{
        fn after_send()->Self{
            let(entered,arrived)=mpsc::channel();let(release,wait)=mpsc::channel();
            set_delivery_preparation_after_send_for_test(move||{
                entered.send(()).unwrap();wait.recv_timeout(Duration::from_secs(6)).expect("custom held worker was not released");
            });Self{arrived,release:Some(release)}
        }
        fn wait(&self){self.arrived.recv_timeout(Duration::from_secs(6)).expect("actual custom worker never sent");}
        fn release(&mut self){if let Some(release)=self.release.take(){let _=release.send(());}}
    }
    impl Drop for HeldCustomWorker{fn drop(&mut self){self.release();}}
    struct CustomJoinedTrip{
        handle:Option<std::thread::JoinHandle<()>>,release:mpsc::Sender<()>,
    }
    impl CustomJoinedTrip{
        fn new(f:&Fixture,hold:&HeldCustomWorker)->Self{
            let latch=f.persistence.upgrade_latch();
            Self{handle:Some(std::thread::spawn(move||latch.trip(RequiredUpgrade{minimum_version:None}))),release:hold.release.as_ref().unwrap().clone()}
        }
        fn join(&mut self){let _=self.release.send(());if let Some(handle)=self.handle.take(){handle.join().unwrap();}}
    }
    impl Drop for CustomJoinedTrip{fn drop(&mut self){
        let _=self.release.send(());if let Some(handle)=self.handle.take(){let result=handle.join();if !std::thread::panicking(){result.unwrap();}}
    }}
    #[test]
    fn core_custom_sent_input_is_not_published_until_actual_worker_exit(){
        let(f,app)=fixture(None);let path=image_file(&f,"sent.png",[19,29,39,255]);let mut hold=HeldCustomWorker::after_send();
        choose(vec![path]);app.global::<AppState>().invoke_choose_custom_prompt_reference();hold.wait();
        pump_for(Duration::from_millis(120));assert!(rows(&app).is_empty(),"sent payload was consumed before worker join");
        hold.release();pump_until(||rows(&app).len()==1);
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();
        assert_eq!(rows(&app).len(),1);
    }
    #[test]
    fn core_custom_clear_cancels_sent_input_before_worker_exit(){
        let(f,app)=fixture(None);let path=image_file(&f,"clear.png",[4,5,6,255]);let mut hold=HeldCustomWorker::after_send();
        choose(vec![path]);app.global::<AppState>().invoke_choose_custom_prompt_reference();hold.wait();
        app.global::<AppState>().invoke_clear_custom_prompt_reference();hold.release();
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();pump_for(Duration::from_millis(120));
        assert!(rows(&app).is_empty());assert!(app.global::<AppState>().get_custom_prompt_reference_path().is_empty());
    }
    #[test]
    fn core_custom_sent_input_after_exact_upgrade_is_joined_without_private_completion(){
        let(f,app)=fixture(None);let path=image_file(&f,"upgrade.png",[7,8,9,255]);let mut hold=HeldCustomWorker::after_send();
        choose(vec![path]);app.global::<AppState>().invoke_choose_custom_prompt_reference();hold.wait();
        let mut trip=CustomJoinedTrip::new(&f,&hold);
        pump_until(||f.persistence.upgrade_latch().is_tripped());
        hold.release();trip.join();drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();
        pump_for(Duration::from_millis(120));assert!(rows(&app).is_empty());
        assert_eq!(app.global::<AppState>().get_custom_prompt_message(),"original status");
        assert!(f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap().custom_prompts.is_empty());
    }
    #[test]
    fn core_custom_sent_input_retirement_does_not_fill_replacement_store(){
        let(f,app)=fixture(None);let path=image_file(&f,"retire.png",[10,11,12,255]);let mut hold=HeldCustomWorker::after_send();
        choose(vec![path]);app.global::<AppState>().invoke_choose_custom_prompt_reference();hold.wait();
        *f.context.active_namespace.lock().unwrap()=None;f.context.store.borrow_mut().private_persistence=None;
        f.context.store.borrow_mut().custom_prompts.push("replacement draft".into());
        cancel_delivery_commit_workers(f.persistence.lease());hold.release();
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();pump_for(Duration::from_millis(120));
        assert!(rows(&app).is_empty());assert_eq!(f.context.store.borrow().custom_prompts,vec!["replacement draft".to_string()]);
        assert_eq!(app.global::<AppState>().get_custom_prompt_message(),"original status");
    }
    #[test]
    fn core_custom_weak_window_cancels_actual_prepared_worker_and_all_handles_join(){
        let(f,app)=fixture(None);let path=image_file(&f,"weak.png",[13,14,15,255]);
        let(arrived,wait)=mpsc::channel();let cancelled=Arc::new(AtomicBool::new(false));let observed=cancelled.clone();
        CUSTOM_INPUT_TEST_PREPARED.with(|hook|*hook.borrow_mut()=Some(Box::new(move|cancel|{
            arrived.send(()).unwrap();let deadline=Instant::now()+Duration::from_secs(4);
            while !cancel.load(Ordering::Acquire)&&Instant::now()<deadline{std::thread::sleep(Duration::from_millis(2));}
            observed.store(cancel.load(Ordering::Acquire),Ordering::Release);
        })));
        choose(vec![path]);app.global::<AppState>().invoke_choose_custom_prompt_reference();wait.recv_timeout(Duration::from_secs(6)).unwrap();
        drop(app);pump_until(||cancelled.load(Ordering::Acquire));
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();
        assert!(cancelled.load(Ordering::Acquire));assert!(f.context.store.borrow().custom_prompts.is_empty());
    }
    #[test]
    fn core_custom_success_payload_followed_by_worker_panic_is_rejected_and_sticky(){
        let(mut f,app)=fixture(None);f.expected_delivery_failure=true;
        let path=image_file(&f,"panic.png",[16,17,18,255]);let(reached,wait)=mpsc::channel();
        set_delivery_preparation_after_send_for_test(move||{reached.send(()).unwrap();panic!("controlled custom post-send failure");});
        choose(vec![path.clone()]);app.global::<AppState>().invoke_choose_custom_prompt_reference();wait.recv_timeout(Duration::from_secs(6)).unwrap();
        pump_for(Duration::from_millis(160));
        assert!(rows(&app).is_empty(),"success survived its worker's failed join");
        assert!(drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).is_err());
        choose(vec![path]);app.global::<AppState>().invoke_choose_custom_prompt_reference();pump_for(Duration::from_millis(100));
        assert!(rows(&app).is_empty());assert!(spawn_delivery_preparation(&f.persistence,|_,_,_|Ok(())).is_err(),"sticky worker failure reopened admission");
    }
    #[test]
    fn core_custom_owned_picker_preserves_order_duplicate_limit_and_saved_preview(){
        let(f,app)=fixture(None);
        let paths=(0..10).map(|index|image_file(&f,&format!("ordered-{index}.png"),[index,20,30,255])).collect::<Vec<_>>();
        let mut selected=vec![paths[0].clone(),paths[0].clone()];selected.extend(paths.iter().skip(1).cloned());
        choose(selected);app.global::<AppState>().invoke_choose_custom_prompt_reference();pump_until(||rows(&app).len()==8);
        let owned=rows(&app).iter().map(|row|row.source_path.to_string()).collect::<Vec<_>>();
        for(index,path)in owned.iter().enumerate(){
            let bytes=f.authority.read_image_source(Path::new(path),100*1024*1024).unwrap();
            assert_eq!(decode_reference_bytes(&bytes).unwrap().to_rgba8().get_pixel(0,0).0,[index as u8,20,30,255]);
        }
        let first=rows(&app)[0].id.clone();app.global::<AppState>().invoke_remove_custom_prompt_reference(first);
        choose(vec![paths[1].clone()]);app.global::<AppState>().invoke_choose_custom_prompt_reference();pump_for(Duration::from_millis(100));assert_eq!(rows(&app).len(),7);
        app.global::<AppState>().invoke_save_custom_prompt("".into(),"Original body".into());
        pump_until(||!app.global::<AppState>().get_custom_prompt_editor_open());
        app.global::<AppState>().invoke_begin_edit_custom_prompt("Original body".into());
        pump_until(||rows(&app).len()==7&&rows(&app).iter().all(|row|row.image.size().width>0));
        assert_eq!(rows(&app).iter().map(|row|row.source_path.to_string()).collect::<Vec<_>>(),owned[1..]);
        let id=rows(&app)[0].id.clone();app.global::<AppState>().invoke_open_custom_prompt_reference(id);
        pump_until(||app.global::<AppState>().get_viewer_open());
        assert_eq!(app.global::<AppState>().get_viewer_source_path(),owned[1]);
        assert_eq!(app.global::<AppState>().get_viewer_width(),80);
    }
    #[test]
    fn core_custom_waiting_picker_does_not_hold_upgrade_drain_permit(){
        let(f,app)=fixture(None);let path=image_file(&f,"dialog.png",[40,50,60,255]);let pending=held_picker();
        app.global::<AppState>().invoke_choose_custom_prompt_reference();
        let latch=f.persistence.upgrade_latch();let(done,wait)=mpsc::channel();
        let worker=std::thread::spawn(move||{latch.trip(RequiredUpgrade{minimum_version:None});let _=done.send(());});
        let drained=wait.recv_timeout(Duration::from_secs(2));
        // Always finish the genuine native seam, then join, even on assertion failure.
        complete(&pending,vec![path]);let joined=worker.join();
        assert!(drained.is_ok(),"waiting native picker retained a counted permit");joined.unwrap();
        pump_for(Duration::from_millis(100));assert!(rows(&app).is_empty());
    }
    #[test]
    fn core_custom_selection_and_remove_are_ordered_in_real_sqlite(){
        let(f,app)=fixture(None);{
            let mut store=f.context.store.borrow_mut();store.custom_prompts=vec!["body one".into(),"body two".into()];
            store.custom_prompt_profiles.insert("body one".into(),CustomPromptProfile{name:"One".into(),..Default::default()});
            store.custom_prompt_profiles.insert("body two".into(),CustomPromptProfile{name:"Two".into(),..Default::default()});
        }
        app.global::<AppState>().set_page("generation".into());
        app.global::<AppState>().invoke_toggle_custom_prompt_selection("body one".into());pump_for(Duration::from_millis(120));
        app.global::<AppState>().invoke_toggle_custom_prompt_selection("body two".into());pump_for(Duration::from_millis(120));
        app.global::<AppState>().invoke_remove_custom_prompt("body one".into());pump_for(Duration::from_millis(120));
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();
        let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(durable.custom_prompts,vec!["body two".to_string()]);
        assert_eq!(durable.selected_custom_prompts.get("character").cloned().unwrap_or_default(),BTreeSet::from(["body two".to_string()]));
        assert_eq!(f.context.store.borrow().custom_prompts,durable.custom_prompts);
    }
    #[test]
    fn core_custom_failed_selection_save_flushes_current_store_without_replaying_toggle_or_later_editor(){
        let(f,app)=fixture(None);
        {let mut store=f.context.store.borrow_mut();store.custom_prompts.push("body".into());store.custom_prompt_profiles.insert("body".into(),CustomPromptProfile{name:"Named".into(),..Default::default()});}
        app.global::<AppState>().set_page("generation".into());f.writer.reject_custom_prompt_inserts_for_test(true);
        app.global::<AppState>().invoke_toggle_custom_prompt_selection("body".into());
        pump_until(||app.global::<AppState>().get_custom_prompt_message().contains("retry"));
        assert!(f.context.store.borrow().selected_custom_prompts["character"].contains("body"));
        assert!(f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap().custom_prompts.is_empty());
        app.global::<AppState>().set_prompt("later editor text".into());f.writer.reject_custom_prompt_inserts_for_test(false);
        app.global::<AppState>().invoke_toggle_custom_prompt_selection("body".into());pump_for(Duration::from_millis(160));
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();pump_for(Duration::from_millis(80));
        let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert!(durable.selected_custom_prompts["character"].contains("body"),"retry toggled already-staged selection a second time");
        assert_eq!(app.global::<AppState>().get_prompt(),"later editor text","save retry replayed an old UI projection");
    }


    fn failed_toggle_then_distinct_action(action:&str){
        let(f,app)=fixture(None);{
            let mut store=f.context.store.borrow_mut();store.custom_prompts=vec!["body A".into(),"body B".into()];
            store.custom_prompt_profiles.insert("body A".into(),CustomPromptProfile{name:"A".into(),..Default::default()});
            store.custom_prompt_profiles.insert("body B".into(),CustomPromptProfile{name:"B".into(),..Default::default()});
        }
        app.global::<AppState>().set_page("generation".into());
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        f.writer.reject_custom_prompt_inserts_for_test(true);
        app.global::<AppState>().invoke_toggle_custom_prompt_selection("body A".into());
        pump_until(||app.global::<AppState>().get_custom_prompt_message().contains("retry"));
        assert!(f.context.store.borrow().selected_custom_prompts["character"].contains("body A"));
        f.writer.reject_custom_prompt_inserts_for_test(false);
        match action{
            "remove"=>app.global::<AppState>().invoke_remove_custom_prompt("body B".into()),
            "clear"=>app.global::<AppState>().invoke_clear_custom_prompt_selections(),
            "category"=>{app.global::<AppState>().set_asset_type("scene".into());app.global::<AppState>().invoke_toggle_custom_prompt_selection("body B".into());},
            _=>panic!("invalid distinct action fixture"),
        }
        pump_for(Duration::from_millis(160));
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();
        let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        match action{
            "remove"=>{
                assert_eq!(durable.custom_prompts,vec!["body A".to_string()],"a failed toggle swallowed later remove");
                assert!(durable.selected_custom_prompts["character"].contains("body A"));
            },
            "clear"=>assert!(durable.selected_custom_prompts.get("character").is_none_or(BTreeSet::is_empty),"a failed toggle swallowed clear"),
            "category"=>{
                assert!(durable.selected_custom_prompts["character"].contains("body A"));
                assert!(durable.selected_custom_prompts.get("scene").is_some_and(|set|set.contains("body B")),"previous category debt swallowed a new category toggle");
            },
            _=>unreachable!(),
        }
        assert_eq!(f.context.store.borrow().selected_custom_prompts,durable.selected_custom_prompts);
    }
    #[test]
    fn core_custom_failed_toggle_does_not_swallow_distinct_remove(){failed_toggle_then_distinct_action("remove");}
    #[test]
    fn core_custom_failed_toggle_does_not_swallow_clear(){failed_toggle_then_distinct_action("clear");}
    #[test]
    fn core_custom_failed_toggle_does_not_swallow_new_category_toggle(){failed_toggle_then_distinct_action("category");}
    #[test]
    fn core_custom_unchanged_selection_typing_does_not_enqueue_store_or_ack_workers(){
        let(f,app)=fixture(None);app.global::<AppState>().set_page("generation".into());
        // This unsaved real Store row exposes any accidental whole-Store write.
        f.context.store.borrow_mut().custom_prompts.push("unsaved sentinel".into());
        let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));let observed=calls.clone();
        set_delivery_preparation_after_send_for_test(move||{observed.fetch_add(1,Ordering::SeqCst);});
        for index in 0..32{
            let text=format!("ordinary keystroke {index}");
            assert_eq!(app.global::<AppState>().invoke_normalize_prompt_editor_text(text.clone().into(),"".into()),text);
        }
        pump_for(Duration::from_millis(160));
        let before=calls.load(Ordering::SeqCst);
        let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        // Consume an unused one-shot fixture hook and join before assertions;
        // this no-op is not a Store write and is not counted as normalization.
        let(_,receiver)=spawn_delivery_preparation(&f.persistence,|_,_,_|Ok(())).unwrap();
        let completed=receiver.recv_timeout(Duration::from_secs(2));
        drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();
        completed.unwrap().unwrap();
        assert_eq!(before,0,"ordinary keystrokes spawned an acknowledgment worker");
        assert!(durable.custom_prompts.is_empty(),"ordinary keystrokes persisted an unrelated Store row");
        assert_eq!(app.global::<AppState>().get_prompt(),"ordinary keystroke 31");
    }
    #[test]
    fn core_custom_typing_does_not_treat_unrelated_save_debt_as_retry(){
        let(f,app)=fixture(None);{
            let mut store=f.context.store.borrow_mut();store.custom_prompts.push("body A".into());
            store.custom_prompt_profiles.insert("body A".into(),CustomPromptProfile{name:"A".into(),..Default::default()});
        }
        app.global::<AppState>().set_page("generation".into());f.writer.reject_custom_prompt_inserts_for_test(true);
        app.global::<AppState>().invoke_toggle_custom_prompt_selection("body A".into());
        pump_until(||app.global::<AppState>().get_custom_prompt_message().contains("retry"));
        f.writer.reject_custom_prompt_inserts_for_test(false);
        let current=app.global::<AppState>().get_prompt().to_string();
        let later=format!("{current} later typing");
        app.global::<AppState>().invoke_normalize_prompt_editor_text(later.clone().into(),"".into());
        pump_for(Duration::from_millis(160));
        let before_retry=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert!(before_retry.custom_prompts.is_empty(),"typing silently flushed unrelated failed-write debt");
        assert_eq!(app.global::<AppState>().get_prompt(),later);
        app.global::<AppState>().invoke_toggle_custom_prompt_selection("body A".into());
        pump_for(Duration::from_millis(160));drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();
        let durable=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert!(durable.selected_custom_prompts["character"].contains("body A"),"typing consumed the exact retry and the real retry toggled twice");
        assert_eq!(app.global::<AppState>().get_prompt(),later);
    }
}
