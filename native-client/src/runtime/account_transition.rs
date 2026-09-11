//! Serialized real-user activation and bounded ordinary user activity.
use super::*;
use std::sync::Condvar;

#[derive(Clone, Default)]
struct AccountTransitionAdmission { state: Arc<Mutex<AccountTransitionAdmissionState>> }
#[derive(Default)]
struct AccountTransitionAdmissionState { sequence: u64, active: Option<u64>, closed: bool }
struct AccountTransitionTicket { admission: AccountTransitionAdmission, id: u64 }
impl AccountTransitionAdmission {
    fn begin(&self) -> Result<AccountTransitionTicket> {
        let mut state = self.state.lock().map_err(|_| anyhow!("account transition poisoned"))?;
        anyhow::ensure!(!state.closed && state.active.is_none(), "账号切换正在进行");
        let Some(id) = state.sequence.checked_add(1) else { state.closed = true; anyhow::bail!("account transition counter exhausted"); };
        state.sequence = id; state.active = Some(id);
        Ok(AccountTransitionTicket { admission: self.clone(), id })
    }
}
impl Drop for AccountTransitionTicket {
    fn drop(&mut self) {
        let mut state = self.admission.state.lock().unwrap_or_else(|poison| {
            let mut state = poison.into_inner(); state.closed = true; state
        });
        if state.active == Some(self.id) { state.active = None; }
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) enum LoginOrigin { Password, Email, Wechat, TeamInviteFresh, TeamInviteReplay }
enum ActivationInput {
    Login { user: LoginUser, tokens: TokenSet, origin: LoginOrigin, suggested_group: Option<String> },
    Resume { owner: String },
    Switch { scope: SessionScope, choice: AccountGroupChoice, rollback: PreviousBillingAuthority },
    PreparedSwitch { ticket: BillingSwitchTicket, choice: AccountGroupChoice },
    Logout { scope: SessionScope, token: Option<String>, all: bool },
}

enum ActivationMailboxPhase {
    Working, UiApplying, Retired, Prepare(Box<PreparedActivation>), Save(Box<PreparedActivation>),
    PreparationFailed(Box<PreparedActivation>,ApiError),
    Saved(Box<PersistedActivation>), Discard(Box<PersistedActivation>), SignedOut, Complete, Failed(ApiError),
}
struct ActivationMailbox {
    phase: Mutex<ActivationMailboxPhase>, changed: Condvar,
    cancelled: std::sync::atomic::AtomicBool,
}
impl ActivationMailbox {
    fn new() -> Self { Self { phase: Mutex::new(ActivationMailboxPhase::Working), changed: Condvar::new(), cancelled: std::sync::atomic::AtomicBool::new(false) } }
    fn put(&self, value: ActivationMailboxPhase) {
        let previous={
            let mut phase=self.phase.lock().unwrap_or_else(|poison|poison.into_inner());
            std::mem::replace(&mut *phase,value)
        };
        self.changed.notify_all();
        // Prepared/Persisted Drop can perform exact credential/writer cleanup.
        // Never run it under the mailbox mutex.
        drop(previous);
    }
    fn fail_on_worker(&self,error:ApiError) {
        let previous={
            let mut phase=self.phase.lock().unwrap_or_else(|poison|poison.into_inner());
            // A short UI operation may temporarily own the prepared payload.
            // Its next put returns ownership before original-worker cleanup.
            while matches!(&*phase,ActivationMailboxPhase::UiApplying) {
                phase=self.changed.wait(phase).unwrap_or_else(|poison|poison.into_inner());
            }
            std::mem::replace(&mut *phase,ActivationMailboxPhase::Working)
        };
        drop(previous);
        self.put(ActivationMailboxPhase::Failed(error));
    }
    fn cancel(&self) { self.cancelled.store(true, Ordering::SeqCst); self.changed.notify_all(); }
    fn wait_for(&self, ready: impl Fn(&ActivationMailboxPhase) -> bool) -> Result<ActivationMailboxPhase, ApiError> {
        let mut phase = self.phase.lock().map_err(transition_error)?;
        loop {
            if self.cancelled.load(Ordering::SeqCst) { return Err(transition_error("账号切换已取消")); }
            if ready(&phase) { return Ok(std::mem::replace(&mut *phase, ActivationMailboxPhase::Working)); }
            phase = self.changed.wait(phase).map_err(transition_error)?;
        }
    }
}
struct ActivationUi {
    mailbox: Arc<ActivationMailbox>, models: Option<PreparedPrivateModels>,
    worker: Option<std::thread::JoinHandle<()>>, login_origin: Option<LoginOrigin>,
}
impl Drop for ActivationUi {
    fn drop(&mut self) { self.mailbox.cancel(); }
}
pub(super) struct AccountTransitionCoordinator {
    core:Arc<TransitionCore>,pending:RefCell<Option<ActivationUi>>,
    retired_workers:RefCell<Vec<ActivationUi>>,worker_failed:Arc<std::sync::atomic::AtomicBool>,
}
#[cfg(test)]
thread_local! {
    static ACTIVATION_WORKER_CHECKPOINT:RefCell<Option<Arc<dyn Fn(&str)+Send+Sync>>>=const {RefCell::new(None)};
}

#[derive(Default)]
pub(super) struct PreparedUiProjection {
    assignments:Vec<Box<dyn FnOnce(&AppState)>>,
}
impl PreparedUiProjection {
    // Values (including model graphs and formatted strings) are evaluated now.
    // The consumed publication closure may only assign that already-owned value.
    pub(super) fn push<T:'static>(&mut self,value:T,set:impl FnOnce(&AppState,T)+'static) {
        self.assignments.push(Box::new(move|state|set(state,value)));
    }
    pub(super) fn append(&mut self,mut other:Self) { self.assignments.append(&mut other.assignments); }
    pub(super) fn publish(self,state:&AppState) { for assignment in self.assignments { assignment(state); } }
}
struct PreparedPrivateModels {
    data:Option<PreparedPrivateStore>,visuals:Option<PreparedActivationVisuals>,visual_persistence:Option<PrivatePersistence>,
    ui:PreparedUiProjection,model_groups:Vec<ModelGroupData>,pagination:CreditLedgerPagination,
    credit_version:Option<String>,credit_epoch:u64,scope:SessionScope,groups:Vec<AccountGroupChoice>,owner:String,
}
impl PreparedPrivateModels {
    fn new(app:&AppWindow,prepared:&mut PreparedActivation,context:&AppContext) -> Result<Self,ApiError> {
        let state=app.global::<AppState>();
        let mut data=prepared.data.clone().map(prepare_private_store);
        if let Some(data)=&mut data {
            let core=&prepared.pending.core;
            data.store.private_persistence=Some(PrivatePersistence::new(core.writer.clone(),prepared.lease.clone(),core.activity.clone(),core.backend.api.upgrade_latch().clone())
                .with_storage(core.root.clone(),core.backend.api.clone(),context.file_index.clone().expect("initialized file index precedes activation")));
        }
        let mut visuals=None;
        let mut visual_persistence=None;
        let preferred_image=data.as_ref().map(|data|data.image_model.clone()).unwrap_or_else(||state.get_image_model().to_string());
        let preferred_prompt=data.as_ref().map(|data|data.reasoning_model.clone()).unwrap_or_else(||state.get_reasoning_model().to_string());
        let preferred_video=data.as_ref().map(|data|data.video_model.clone()).unwrap_or_else(||state.get_video_model().to_string());
        let preferred_pack=if data.is_some() { String::new() } else { state.get_selected_credit_pack_code().to_string() };
        let PreparedBackendProjection { mut ui,model_groups,pagination,credit_version } =
            prepare_activation_backend_projection(&prepared.snapshot,&preferred_image,&preferred_prompt,&preferred_video,&preferred_pack);
        ui.append(prepare_activation_team_projection(&prepared.groups,prepared.billing_ticket.proposed_scope(),&prepared.snapshot.account));
        if let Some(data)=&data {
            let category=prepared.profile.as_ref().map(|profile|resolve_category(&profile.asset_type,"")).unwrap_or_else(||"character".into());
            let prompt=prompt_draft_for_category(&data.store.prompt_drafts,&category);
            ui.push(category.clone().into(),|state,value|state.set_asset_type(value));
            ui.push(prompt.clone().into(),|state,value|state.set_prompt(value));
            ui.push(negative_prompt_draft_for_category(&data.store.prompt_drafts,&category).into(),|state,value|state.set_negative_prompt(value));
            ui.append(prepare_startup_projection(&data.store,&category,&prompt));
            let mut prepared_visuals=prepare_activation_visuals(&data.store,&category,state.get_language().as_str(),
                state.get_asset_gallery_layout().as_str(),state.get_generation_gallery_layout().as_str());
            ui.append(prepared_visuals.take_ui());
            visual_persistence=data.store.private_persistence.clone();
            visuals=Some(prepared_visuals);
            ui.push("generation".into(),|state,value|state.set_page(value));
        }
        if let Some(profile)=&prepared.profile {
            ui.push(profile.accepted_user_terms_version.clone().into(),|state,value|state.set_accepted_user_terms_version(value));
            ui.push(profile.accepted_privacy_version.clone().into(),|state,value|state.set_accepted_privacy_version(value));
        } else if data.is_some() {
            ui.push("".into(),|state,value|state.set_accepted_user_terms_version(value));
            ui.push("".into(),|state,value|state.set_accepted_privacy_version(value));
        }
        ui.push(false,|state,value|state.set_account_group_switching(value));
        ui.push(true,|state,value|state.set_logged_in(value));
        ui.push(false,|state,value|state.set_offline_mode(value));
        ui.push("online".into(),|state,value|state.set_session_state(value));
        ui.push(true,|state,value|state.set_ever_authenticated(value));
        ui.push(true,|state,value|state.set_offline_available(value));
        ui.push(false,|state,value|state.set_auth_open(value));
        ui.push(false,|state,value|state.set_auth_busy(value));
        ui.push("".into(),|state,value|state.set_auth_password(value));
        ui.push("".into(),|state,value|state.set_auth_code(value));
        ui.push("".into(),|state,value|state.set_auth_error(value));
        let credit_epoch=if data.is_some() { 1 } else { context.store.borrow().credit_sync_epoch.checked_add(1).ok_or_else(||transition_error("credit publication counter exhausted"))? };
        let mut profile=prepared.profile.clone().unwrap_or_else(||if data.is_some() { UserProfileData::default() } else { user_profile_data(app) });
        profile.logged_in=false; profile.backend_auth_version=1; profile.ever_authenticated=true;
        profile.nickname=prepared.snapshot.account.user.nickname.clone().unwrap_or_default();
        profile.email_mask=prepared.snapshot.account.user.email_masked.clone();
        if data.is_some() { profile.asset_type=resolve_category(&profile.asset_type,""); }
        prepared.profile=Some(profile);
        Ok(Self { data,visuals,visual_persistence,ui,model_groups,pagination,credit_version,credit_epoch,
            scope:prepared.billing_ticket.proposed_scope().request.session.clone(),
            owner:prepared.lease.namespace.user_public_id().to_owned(),groups:prepared.groups.clone() })
    }
    fn publish(self,app:&AppWindow,context:&AppContext) -> Option<ActivationVisualEffects> {
        let state=app.global::<AppState>();
        if let Some(data)=self.data {
            *context.store.borrow_mut()=data.store;
        }
        {
            let mut store=context.store.borrow_mut();
            store.model_groups=self.model_groups;
            store.credit_ledger_pagination=self.pagination;
            store.credit_account_version=self.credit_version;
            store.credit_sync_epoch=self.credit_epoch;
        }
        *context.account_snapshot_scope.lock().unwrap_or_else(|poison|poison.into_inner())=Some(self.scope);
        *context.current_user_id.lock().unwrap_or_else(|poison|poison.into_inner())=Some(self.owner);
        context.team_groups.replace(self.groups);
        self.ui.publish(&state);
        self.visuals.zip(self.visual_persistence).map(|(visuals,persistence)|visuals.publish(persistence))
    }
}

impl AccountTransitionCoordinator {
    pub(super) fn initialize(context: &mut AppContext, root_path: PathBuf) -> Result<()> {
        context.file_index = Some(FileIndex::initialize(root_path.join("storage-index.sqlite3"))?);
        context.backend.as_ref().ok_or_else(|| anyhow!("backend unavailable"))?.api
            .bind_user_work(UserWorkAdmission::new(context.active_namespace.clone(), context.user_activity.clone()))?;
        context.billing_context = Arc::new(BillingContextManager::with_upgrade_latch(
            context.backend.as_ref().ok_or_else(|| anyhow!("backend unavailable"))?.api.upgrade_latch().clone(),
        ));
        let core = Arc::new(TransitionCore {
            backend: context.backend.clone().ok_or_else(|| anyhow!("backend unavailable"))?,
            writer: client_state_writer()?.clone(), root: context.data_root_capability.clone().ok_or_else(|| anyhow!("retained root unavailable"))?, root_path,
            admission: AccountTransitionAdmission::default(), namespace_operations: context.namespace_operations.clone(),
            activity: context.user_activity.clone(), billing: context.billing_context.clone(), active_namespace: context.active_namespace.clone(),
            current_user_id: context.current_user_id.clone(), cleanup_failure: Arc::new(Mutex::new(None)),
        });
        context.account_transition = Some(Rc::new(Self { core, pending: RefCell::new(None),
            retired_workers:RefCell::new(Vec::new()),worker_failed:Arc::new(false.into()) }));
        Ok(())
    }
    pub(super) fn activate_authenticated(self: &Rc<Self>, app: &AppWindow, context: AppContext, response: LoginResponse, origin: LoginOrigin, suggested_group: Option<String>) {
        self.activate_identity(app, context, response.user, response.tokens, origin, suggested_group);
    }
    pub(super) fn activate_identity(self: &Rc<Self>, app: &AppWindow, context: AppContext, user: LoginUser, tokens: TokenSet, origin: LoginOrigin, suggested_group: Option<String>) {
        self.start(app, context, ActivationInput::Login { user, tokens, origin, suggested_group });
    }
    pub(super) fn resume_persisted_session(self: &Rc<Self>, app: &AppWindow, context: AppContext) {
        let Some(owner) = resumable_persisted_owner(self.core.backend.api.session(), &self.core.cleanup_failure) else { return; };
        self.start(app, context, ActivationInput::Resume { owner });
    }
    pub(super) fn switch_billing(self: &Rc<Self>, app: &AppWindow, context: AppContext, choice: AccountGroupChoice, rollback: PreviousBillingAuthority) {
        let Some(scope) = context.current_account_session_scope() else { return; };
        self.start(app, context, ActivationInput::Switch { scope, choice, rollback });
    }
    pub(super) fn logout(self: &Rc<Self>, app: &AppWindow, context: AppContext, scope: SessionScope, all: bool) {
        let token = self.core.backend.api.session().access().filter(|access| access.auth_epoch == scope.auth_epoch).map(|access| access.access_token);
        self.start(app, context, ActivationInput::Logout { scope, token, all });
    }
    fn start(self: &Rc<Self>, app: &AppWindow, context: AppContext, input: ActivationInput) {
        self.reap_finished_workers();
        let login_origin = match &input {
            ActivationInput::Login { origin, .. } => Some(*origin),
            _ => None,
        };
        let explicit_login = matches!(&input, ActivationInput::Login { .. });
        if self.pending.borrow().is_some() {
            if explicit_login {
                let state = app.global::<AppState>();
                state.set_auth_busy(false);
                state.set_session_state("signed_out".into());
                state.set_auth_error("账号数据正在切换，请稍后重试".into());
            }
            return;
        }
        let ticket = match self.core.admission.begin() {
            Ok(ticket) => ticket,
            Err(error) => {
                let state = app.global::<AppState>();
                if explicit_login {
                    state.set_auth_busy(false);
                    state.set_session_state("signed_out".into());
                }
                state.set_auth_error(error.to_string().into());
                return;
            }
        };
        let input = match input {
            ActivationInput::Switch { scope, choice, rollback } => {
                let proposed = self.core.backend.api.upgrade_latch().apply_if_open(|| self.core.billing.begin_switch(&scope, &self.core.backend.api.device().id, &choice.group_id, rollback));
                match proposed {
                    Ok(Ok(billing_ticket)) => {
                        clear_billing_snapshot_state(app, &context);
                        let groups = context.team_groups.borrow().clone();
                        clear_team_context(app, &context);
                        context.team_groups.replace(groups);
                        render_team_context(app, &context);
                        app.global::<AppState>().set_account_group_switching(true);
                        app.global::<AppState>().set_account_group_error("".into());
                        ActivationInput::PreparedSwitch { ticket: billing_ticket, choice }
                    }
                    Ok(Err(error)) => { app.global::<AppState>().set_auth_error(error.user_message().into()); return; }
                    Err(required) => { app.global::<AppState>().set_auth_error(required.as_error().user_message().into()); return; }
                }
            }
            input => input,
        };
        let mailbox = Arc::new(ActivationMailbox::new());
        // The consumer, cancellation owner and pending-model slot precede the worker.
        *self.pending.borrow_mut() = Some(ActivationUi { mailbox: mailbox.clone(), models: None, worker: None, login_origin });
        let core = self.core.clone();
        let worker_mailbox = mailbox.clone();
        let worker_failed=self.worker_failed.clone();
        #[cfg(test)]
        let checkpoint=ACTIVATION_WORKER_CHECKPOINT.with(|hook|hook.borrow().clone());
        let worker = std::thread::Builder::new().name("account-activation".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                if let Some(hook)=&checkpoint { hook("working"); }
                let prepared = core.prepare_activation(input, &ticket, || {
                    worker_mailbox.put(ActivationMailboxPhase::Retired);
                    worker_mailbox.wait_for(|phase| matches!(phase, ActivationMailboxPhase::Working))?;
                    Ok(())
                })?;
                let Some(prepared) = prepared else { worker_mailbox.put(ActivationMailboxPhase::SignedOut); return Ok(()); };
                worker_mailbox.put(ActivationMailboxPhase::Prepare(Box::new(prepared)));
                #[cfg(test)]
                if let Some(hook)=&checkpoint { hook("prepare"); }
                let prepared=match worker_mailbox.wait_for(|phase|matches!(phase,ActivationMailboxPhase::Save(_)|ActivationMailboxPhase::PreparationFailed(..)))? {
                    ActivationMailboxPhase::Save(prepared)=>prepared,
                    ActivationMailboxPhase::PreparationFailed(prepared,error)=>{ drop(prepared); return Err(error); },
                    _=>unreachable!(),
                };
                let persisted = prepared.persist()?;
                worker_mailbox.put(ActivationMailboxPhase::Saved(Box::new(persisted)));
                #[cfg(test)]
                if let Some(hook)=&checkpoint { hook("saved"); }
                match worker_mailbox.wait_for(|phase| matches!(phase, ActivationMailboxPhase::Complete | ActivationMailboxPhase::Discard(_)))? {
                    ActivationMailboxPhase::Discard(persisted) => {
                        drop(persisted);
                        return Err(core.backend.api.upgrade_latch().snapshot().map(|required| required.as_error()).unwrap_or_else(|| transition_error("activation cancelled")));
                    }
                    ActivationMailboxPhase::Complete => {},
                    _ => unreachable!(),
                }
                Ok::<_, ApiError>(())
            }));
            match result {
                Ok(Ok(()))=>{},
                Ok(Err(error))=>worker_mailbox.fail_on_worker(error),
                Err(payload)=>{
                    worker_failed.store(true,Ordering::SeqCst);
                    core.admission.state.lock().unwrap_or_else(|poison|poison.into_inner()).closed=true;
                    // The payload is never surfaced in ordinary UI. Candidate
                    // cleanup remains on this actual worker and preserves debt.
                    drop(payload);
                    worker_mailbox.fail_on_worker(transition_error("账号切换线程异常，切换未完成"));
                }
            }
            drop(ticket);
        });
        match worker {
            Ok(worker) => {
                self.pending.borrow_mut().as_mut().unwrap().worker = Some(worker);
                app.global::<AppState>().set_auth_busy(true);
                Self::poll(self.clone(), app.as_weak(), context);
            }
            Err(error) => { self.pending.borrow_mut().take(); app.global::<AppState>().set_auth_error(transition_error(error).user_message().into()); }
        }
    }
    fn poll(coordinator: Rc<Self>, app: Weak<AppWindow>, context: AppContext) {
        slint::Timer::single_shot(Duration::from_millis(40), move || {
            let Some(window) = app.upgrade() else {
                coordinator.cancel_pending_receiver();
                if coordinator.reap_finished_workers() {
                    // Retain the coordinator and real handles even though the
                    // UI receiver is gone. app.run shutdown also owns this drain.
                    Self::poll(coordinator,app,context);
                }
                return;
            };
            let app=window;
            if coordinator.advance(&app, &context) { Self::poll(coordinator, app.as_weak(), context); }
        });
    }
    fn record_worker_failure(&self) {
        self.worker_failed.store(true,Ordering::SeqCst);
        self.core.admission.state.lock().unwrap_or_else(|poison|poison.into_inner()).closed=true;
    }
    fn cancel_pending_receiver(&self) {
        if let Some(pending)=self.pending.borrow_mut().take() {
            pending.mailbox.cancel();
            self.retired_workers.borrow_mut().push(pending);
        }
    }
    fn reap_finished_workers(&self)->bool {
        let ready={
            let mut workers=self.retired_workers.borrow_mut();let mut ready=Vec::new();let mut index=0;
            while index<workers.len(){
                if workers[index].worker.as_ref().is_none_or(|worker|worker.is_finished()) {
                    ready.push(workers.swap_remove(index));
                }else{index+=1;}
            }
            ready
        };
        for mut worker in ready {
            if let Some(handle)=worker.worker.take(){if handle.join().is_err(){self.record_worker_failure();}}
        }
        !self.retired_workers.borrow().is_empty()
    }
    fn advance(&self, app: &AppWindow, context: &AppContext) -> bool {
        let retired_pending=self.reap_finished_workers();
        let mut slot = self.pending.borrow_mut();
        let Some(pending) = slot.as_mut() else { return retired_pending; };
        let phase = {
            let mut phase = pending.mailbox.phase.lock().unwrap_or_else(|poison| poison.into_inner());
            match &*phase {
                ActivationMailboxPhase::Retired | ActivationMailboxPhase::Prepare(_) | ActivationMailboxPhase::Saved(_) | ActivationMailboxPhase::Failed(_) | ActivationMailboxPhase::SignedOut => std::mem::replace(&mut *phase, ActivationMailboxPhase::UiApplying),
                _ => return true,
            }
        };
        match phase {
            ActivationMailboxPhase::Retired => {
                clear_retired_private_state(app, context);
                pending.mailbox.put(ActivationMailboxPhase::Working);
            }
            ActivationMailboxPhase::Prepare(mut prepared) => {
                match PreparedPrivateModels::new(app,&mut prepared,context) {
                    Ok(models)=>{ pending.models=Some(models); pending.mailbox.put(ActivationMailboxPhase::Save(prepared)); },
                    Err(error)=>pending.mailbox.put(ActivationMailboxPhase::PreparationFailed(prepared,error)),
                }
            }
            ActivationMailboxPhase::Saved(persisted) => {
                if persisted.prepared.namespace.is_some() {
                    // A queued external selection predates this private publication.
                    // Discard only its in-memory hints, never the selected files.
                    drop(crate::platform::take_external_image_drops());
                }
                let latch = self.core.backend.api.upgrade_latch().clone();
                let mut publication = Some((persisted, pending.models.take().expect("prepared UI precedes durable save")));
                let published = latch.apply_if_open(|| {
                    let (persisted, models) = publication.take().unwrap();
                    let PersistedActivation { prepared, _commit } = *persisted;
                    let PreparedActivation { mut pending, namespace, activity, lease, billing_ticket, staged, .. } = prepared;
                    self.core.billing.publish_persisted(billing_ticket, staged);
                    let changes_user = namespace.is_some();
                    if let Some(namespace) = namespace { namespace.publish(); }
                    *self.core.active_namespace.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(lease);
                    let visual_effects=models.publish(app, context);
                    if let Some(activity) = activity { activity.publish(); } pending.committed = true;
                    (_commit, changes_user,visual_effects)
                });
                match published {
                    Ok((commit, changes_user,visual_effects)) => {
                        pending.mailbox.put(ActivationMailboxPhase::Complete);
                        drop(commit);
                        // No I/O or dispatch occurred between the save and publication above.
                        self.retired_workers.borrow_mut().push(slot.take().unwrap());drop(slot);
                        if let Some(effects)=visual_effects { start_activation_visual_effects(app,context.clone(),effects); }
                        // Original-payer settlement is independent of the currently
                        // selected group's UI. This also restarts A after A→B→A.
                        recover_pending_orders(app, context.clone());
                        if changes_user {
                            recover_pending_generations(app, context.clone()); recover_pending_prompt_tasks(app, context.clone());
                            recover_prompt_optimization(app, context.clone());
                        }
                        if app.global::<AppState>().get_team_tab().as_str() == "pending" {
                            app.global::<AppState>().invoke_load_pending_team_invitations("".into());
                        }
                        return self.reap_finished_workers();
                    }
                    Err(required) => {
                        let (persisted, _) = publication.take().unwrap();
                        pending.mailbox.put(ActivationMailboxPhase::Discard(persisted));
                        app.global::<AppState>().set_session_state("update_required".into());
                        app.global::<AppState>().set_auth_error("当前客户端版本过旧，必须更新后继续使用".into());
                        show_required_update_prompt(app, required.minimum_version.as_deref().unwrap_or_default());
                        return true;
                    }
                }
            }
            ActivationMailboxPhase::SignedOut => {
                self.retired_workers.borrow_mut().push(slot.take().unwrap());drop(slot);
                let state = app.global::<AppState>(); state.set_auth_busy(false); state.set_auth_open(true); state.set_profile_open(false);
                state.set_session_state("signed_out".into());
                return self.reap_finished_workers();
            }
            ActivationMailboxPhase::Failed(error) => {
                let failed = slot.take().unwrap();
                let login_origin = failed.login_origin;
                self.retired_workers.borrow_mut().push(failed);drop(slot);
                let state = app.global::<AppState>(); state.set_auth_busy(false); state.set_account_group_switching(false);
                state.set_account_group_error(error.user_message().into());
                render_team_context(app, context);
                if context.active_namespace.lock().unwrap_or_else(|poison| poison.into_inner()).is_none() { state.set_auth_open(true); }
                if let Some(required) = RequiredUpgrade::from_error(&error) {
                    state.set_session_state("update_required".into());
                    show_required_update_prompt(app, required.minimum_version.as_deref().unwrap_or_default());
                } else if context.active_namespace.lock().unwrap_or_else(|poison| poison.into_inner()).is_none() { state.set_logged_in(false); state.set_session_state("signed_out".into()); }
                let cleanup = self.core.cleanup_failure.lock().unwrap_or_else(|poison| poison.into_inner()).clone();
                let message = match cleanup {
                    Some(cleanup) => format!("{}；{}", error.user_message(), cleanup).into(),
                    None => error.user_message().into(),
                };
                present_activation_failure(&state, login_origin, message);
                return self.reap_finished_workers();
            }
            _ => {},
        }
        true
    }
    pub(super) fn shutdown(&self) -> Result<()> {
        self.core.admission.state.lock().unwrap_or_else(|poison|poison.into_inner()).closed=true;
        self.cancel_pending_receiver();
        let workers=std::mem::take(&mut *self.retired_workers.borrow_mut());
        for worker in &workers {worker.mailbox.cancel();}
        // Cancel every worker before joining any, and join all before reporting.
        for mut worker in workers {
            if let Some(handle)=worker.worker.take(){if handle.join().is_err(){self.record_worker_failure();}}
        }
        anyhow::ensure!(!self.worker_failed.load(Ordering::SeqCst),"账号切换线程异常，切换未完成");
        Ok(())
    }
}

fn present_activation_failure(state: &AppState, origin: Option<LoginOrigin>, message: SharedString) {
    let message = if origin.is_some() {
        format!("登录验证已通过，但客户端未能完成登录：{message}").into()
    } else {
        message
    };
    state.set_auth_error(message.clone());
    if matches!(origin, Some(LoginOrigin::Email)) {
        state.set_auth_code("".into());
        state.set_auth_error(format!("{message}；请重新获取验证码后重试").into());
    }
    if matches!(origin, Some(LoginOrigin::Wechat)) {
        state.set_auth_wechat_status(message);
    }
}

fn clear_retired_private_state(app: &AppWindow, context: &AppContext) {
    let retired_lease = context.store.borrow().private_persistence.as_ref().map(|binding| binding.lease().clone());
    if let Some(lease) = retired_lease {
        cancel_native_file_drag_for_retirement(&lease);
        cancel_reference_workers_for_retirement(&lease);
        cancel_enhancement_workers_for_retirement(&lease);
        cancel_cutout_workers_for_retirement(&lease);
        cancel_canvas_workers_for_lease(&lease);
        close_video_player_for_retirement(&lease);
        notification_callbacks::cancel_notification_workers_for_retirement(&lease);
        payment_callbacks::cancel_payment_workers_for_retirement(&lease);
        cancel_delivery_commit_workers(&lease);
        cancel_prompt_workers_for_retirement(&lease);
        cancel_team_workers_for_retirement(&lease);
        cancel_toolbox_workers_for_lease(&lease);
    }
    drop(crate::platform::take_external_image_drops());
    clear_team_context(app, context);
    clear_account_snapshot_state(app, context); clear_payment_account_state(app, context);
    clear_notification_account_state(app, context); clear_prompt_task_account_state(app);
    clear_prompt_optimization_account_state(app, context); clear_generation_account_state(app, context, None);
    *context.store.borrow_mut() = Store::default();
    *context.canvas_history.borrow_mut() = CanvasController::default();
    clear_retired_private_projection(&app.global::<AppState>());
    let state = app.global::<AppState>(); state.set_logged_in(false); state.set_session_state("signed_out".into());
    state.set_nickname("".into()); state.set_email_mask("".into()); state.set_prompt("".into()); state.set_negative_prompt("".into()); state.set_canvas_workflow_prompt("".into());
}
/// Captured at model preparation, never reconstructed from mutable UI identity.
#[derive(Clone)]
pub(super) struct PrivatePersistence {
    binding: Arc<()>,
    writer: ClientStateWriter, lease: NamespaceLease, activity: UserActivityGate, upgrade: UpgradeLatch,
    storage: Option<(Arc<DataRootCapability>, ApiClient, FileIndex)>,
}
impl PrivatePersistence {
    #[cfg(test)]
    pub(super) fn for_test_with_storage(writer: ClientStateWriter, lease: NamespaceLease, activity: UserActivityGate, upgrade: UpgradeLatch,
        root: Arc<DataRootCapability>, client: ApiClient, index: FileIndex) -> Self {
        Self::new(writer, lease, activity, upgrade).with_storage(root, client, index)
    }
    #[cfg(test)]
    pub(super) fn for_test(writer: ClientStateWriter, lease: NamespaceLease, activity: UserActivityGate, upgrade: UpgradeLatch) -> Self {
        Self::new(writer, lease, activity, upgrade)
    }
    fn new(writer: ClientStateWriter, lease: NamespaceLease, activity: UserActivityGate, upgrade: UpgradeLatch) -> Self {
        Self { binding: Arc::new(()), writer, lease, activity, upgrade, storage: None }
    }
    fn with_storage(mut self, root: Arc<DataRootCapability>, client: ApiClient, index: FileIndex) -> Self {
        self.storage = Some((root, client, index)); self
    }
    pub(super) fn begin_activity(&self) -> Result<UserActivityPermit> { self.activity.begin_recovery_unit(&self.lease) }
    pub(super) fn lease(&self) -> &NamespaceLease { &self.lease }
    pub(super) fn upgrade_latch(&self) -> UpgradeLatch { self.upgrade.clone() }
    pub(super) fn owns_path(&self, path: &Path) -> bool { path.starts_with(self.lease.namespace.root()) }
    pub(super) fn is_current(&self) -> bool {
        !self.upgrade.is_tripped() && self.activity.begin_recovery_unit(&self.lease).is_ok()
    }
    pub(super) fn same_binding(&self, other: &Self) -> bool { self.lease == other.lease && self.is_current() }
    /// Pure identity comparison for an already admitted short completion.
    pub(super) fn same_binding_metadata(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.binding, &other.binding) && self.lease == other.lease
    }
    pub(super) fn begin_effect(&self) -> Result<(UserActivityPermit, api::OrdinaryBlockingEffectPermit)> {
        let activity = self.activity.begin_recovery_unit(&self.lease)?;
        let effect = self.upgrade.begin_ordinary_blocking_effect().map_err(|required| anyhow!(required.as_error().user_message()))?;
        Ok((activity, effect))
    }
    pub(super) fn storage_authority(&self) -> Result<Arc<NamespaceStorageAuthority>> {
        let _activity = self.activity.begin_recovery_unit(&self.lease)?;
        anyhow::ensure!(!self.upgrade.is_tripped(), "ordinary work closed");
        let (root, client, index) = self.storage.as_ref().ok_or_else(|| anyhow!("captured private storage unavailable"))?;
        Ok(Arc::new(NamespaceStorageAuthority::open_active(root.clone(), &self.lease, client.clone(), index.clone())?))
    }
    pub(super) fn prepare_ordered_save(&self) -> Result<PreparedPrivateStoreWrite> {
        let durable = self.upgrade.begin_ordinary_durable_commit().map_err(|required| anyhow!(required.as_error().user_message()))?;
        let activity = self.activity.begin_recovery_unit(&self.lease)?;
        Ok(PreparedPrivateStoreWrite { writer: self.writer.clone(), lease: self.lease.clone(), admission: (activity, durable) })
    }
    pub(super) fn save_store(&self, data: LocalStoreData) -> Result<()> {
        let _commit = self.upgrade.begin_ordinary_durable_commit().map_err(|required| anyhow!(required.as_error().user_message()))?;
        let _unit = self.activity.begin_recovery_unit(&self.lease)?;
        self.writer.persist_client_state_checked_for_namespace(&self.lease, data)?;
        Ok(())
    }
    pub(super) fn save_profile(&self, data: UserProfileData) -> Result<()> {
        let _commit = self.upgrade.begin_ordinary_durable_commit().map_err(|required| anyhow!(required.as_error().user_message()))?;
        let _unit = self.activity.begin_recovery_unit(&self.lease)?;
        self.writer.persist_client_user_profile_checked_for_namespace(&self.lease, data)?;
        Ok(())
    }
    pub(super) fn read_retained_redemption(&self, session: &SessionScope, key: &str) -> Result<PendingCreditRedemption> {
        anyhow::ensure!(self.lease.auth_epoch == session.auth_epoch && self.lease.namespace.user_public_id() == session.owner_user_id, "redemption namespace/session mismatch");
        let _activity = self.activity.begin_recovery_unit(&self.lease)?;
        anyhow::ensure!(!self.upgrade.is_tripped(), "ordinary work is closed");
        let record = self.writer.read_retained_redemption_checked(&self.lease, key)?
            .filter(|record| !record.code.is_empty() && !record.billing_account_group_id.is_empty())
            .ok_or_else(|| anyhow!("exact retained redemption missing"))?;
        Uuid::parse_str(&record.billing_account_group_id)?;
        Ok(record)
    }
}
/// Prepare outside apply_user_completion; retain its Option outside the closure.
/// Return the entire enqueue result out of that closure before dropping an error.
pub(super) struct PreparedPrivateStoreWrite {
    writer: ClientStateWriter, lease: NamespaceLease, admission: client_state::StoreWriteAdmission,
}
impl PreparedPrivateStoreWrite {
    pub(super) fn lease(&self) -> &NamespaceLease { &self.lease }
    pub(super) fn enqueue_profile(self, data: UserProfileData)
        -> std::result::Result<mpsc::Receiver<client_state::WriteResult>, client_state::PreparedStoreEnqueueError> {
        self.writer.enqueue_client_user_profile_checked_for_namespace(&self.lease,data,self.admission)
    }
    pub(super) fn enqueue(self, data: LocalStoreData)
        -> std::result::Result<mpsc::Receiver<client_state::WriteResult>, client_state::PreparedStoreEnqueueError> {
        self.writer.enqueue_client_state_checked_for_namespace(&self.lease, data, self.admission)
    }
}
struct RetiredSessionCleanup {
    session: Arc<SessionManager>, scope: SessionScope,
    debt: Arc<Mutex<Option<CleanupDebt>>>, armed: bool,
}
impl RetiredSessionCleanup {
    fn new(session: Arc<SessionManager>, scope: SessionScope, debt: Arc<Mutex<Option<CleanupDebt>>>) -> Self {
        Self { session, scope, debt, armed: true }
    }
    fn clear(&mut self) -> Result<(), ApiError> {
        if !self.armed { return Ok(()); }
        self.armed = false;
        let result = self.session.clear_scope(&self.scope).and_then(|()| {
            // A stale-scope no-op is not a receipt for deletion of retained credentials.
            if self.session.persisted_owner_user_id().is_none() { Ok(()) }
            else { Err(transition_error(INCOMPLETE_CREDENTIAL_CLEANUP)) }
        });
        update_cleanup_debt(&self.debt, |debt| debt.credentials = result.is_err());
        result
    }
}
impl Drop for RetiredSessionCleanup { fn drop(&mut self) { let _ = self.clear(); } }
struct RetiredLogoutAttempt {
    api: ApiClient, token: Option<String>, all: bool, armed: bool,
    debt: Arc<Mutex<Option<CleanupDebt>>>,
}
impl RetiredLogoutAttempt {
    fn attempt(&mut self) -> Result<(), ApiError> {
        if !self.armed { return Ok(()); }
        self.armed = false;
        let Some(token) = self.token.take() else { return Ok(()); };
        let result = AuthApi::new(self.api.clone()).logout_with_fixed_token(self.all, &token);
        if result.is_err() { update_cleanup_debt(&self.debt, |debt| debt.remote_logout = true); }
        result
    }
}
impl Drop for RetiredLogoutAttempt { fn drop(&mut self) { let _ = self.attempt(); } }
#[derive(Clone, Debug, Default)]
struct CleanupDebt { credentials: bool, private_ui: bool, candidate_writer: bool, remote_logout: bool }
impl CleanupDebt {
    fn is_empty(&self) -> bool { !self.credentials && !self.private_ui && !self.candidate_writer && !self.remote_logout }
}
impl std::fmt::Display for CleanupDebt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut messages = Vec::new();
        if self.credentials { messages.push(INCOMPLETE_CREDENTIAL_CLEANUP); }
        if self.private_ui { messages.push("退出界面清理未确认"); }
        if self.candidate_writer { messages.push("候选账号存储清理未确认，当前登录保持关闭"); }
        if self.remote_logout { messages.push("远程退出未确认"); }
        formatter.write_str(&messages.join("；"))
    }
}
fn update_cleanup_debt(debt: &Mutex<Option<CleanupDebt>>, update: impl FnOnce(&mut CleanupDebt)) {
    let mut slot = debt.lock().unwrap_or_else(|poison| {
        let mut slot = poison.into_inner();
        slot.get_or_insert_with(CleanupDebt::default).candidate_writer = true;
        slot
    });
    update(slot.get_or_insert_with(CleanupDebt::default));
    if slot.as_ref().is_some_and(CleanupDebt::is_empty) { *slot = None; }
}
const INCOMPLETE_CREDENTIAL_CLEANUP: &str = "凭据清理及退出尚未完成；重启可能恢复保留的会话";
fn resumable_persisted_owner(session: &SessionManager, debt: &Mutex<Option<CleanupDebt>>) -> Option<String> {
    // Even a remote-only warning requires an explicit new login, never silent resume.
    if debt.lock().ok()?.is_some() { return None; }
    session.persisted_owner_user_id()
}
struct TransitionCore {
    backend: Arc<BackendRuntime>, writer: ClientStateWriter,
    root: Arc<DataRootCapability>, root_path: PathBuf,
    admission: AccountTransitionAdmission, namespace_operations: NamespaceOperationGate,
    activity: UserActivityGate, billing: Arc<BillingContextManager>,
    active_namespace: Arc<Mutex<Option<NamespaceLease>>>,
    current_user_id: Arc<Mutex<Option<String>>>,
    cleanup_failure: Arc<Mutex<Option<CleanupDebt>>>,
}
pub(super) fn transition_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::LocalState { message: format!("账号切换未完成：{error}") }
}
struct PendingActivation {
    core: Arc<TransitionCore>, scope: SessionScope, writer_lease: Option<NamespaceLease>, committed: bool, changes_user: bool,
}
impl Drop for PendingActivation {
    fn drop(&mut self) {
        if self.committed || !self.changes_user { return; }
        self.core.billing.invalidate_for_auth_change();
        if let Some(lease) = &self.writer_lease {
            if self.core.writer.deactivate(lease).is_err() {
                update_cleanup_debt(&self.core.cleanup_failure, |debt| debt.candidate_writer = true);
            }
        }
        let cleared = self.core.backend.api.session().clear_scope(&self.scope);
        let acknowledged = cleared.is_ok()
            && self.core.backend.api.session().persisted_owner_user_id().is_none();
        update_cleanup_debt(&self.core.cleanup_failure, |debt| debt.credentials = !acknowledged);
    }
}
struct PreparedActivation {
    pending: PendingActivation,
    namespace: Option<PreparedNamespacePublication>, activity: Option<PreparedUserActivity>,
    lease: NamespaceLease, billing_ticket: BillingSwitchTicket, staged: StagedBillingConfirmation,
    data: Option<LocalStoreData>, profile: Option<UserProfileData>, snapshot: BackendSnapshot,
    groups: Vec<AccountGroupChoice>,
}
struct PersistedActivation { prepared: PreparedActivation, _commit: OrdinaryDurableCommitPermit }
impl TransitionCore {
    fn prepare_activation(
        self: &Arc<Self>, input: ActivationInput, ticket: &AccountTransitionTicket,
        retired_ui: impl FnOnce() -> Result<(), ApiError>,
    ) -> Result<Option<PreparedActivation>, ApiError> {
        let input = match input {
            ActivationInput::PreparedSwitch { ticket, choice } => return self.prepare_billing_switch(ticket, choice).map(Some),
            input => input,
        };
        if let Some(required) = self.backend.api.upgrade_latch().snapshot() { return Err(required.as_error()); }
        let explicit_login = matches!(&input, ActivationInput::Login { .. });
        if let Some(debt) = self.cleanup_failure.lock().map_err(transition_error)?.as_ref() {
            if !explicit_login || debt.candidate_writer { return Err(transition_error(debt)); }
        }
        let mut namespace_transition = self.namespace_operations.begin_transition().map_err(transition_error)?;
        let old = self.active_namespace.lock().map_err(transition_error)?.clone();
        // Prepare all cleanup ownership before the irreversible writer retirement.
        let mut retired_cleanup = old.as_ref().map(|old| {
            let scope = SessionScope { owner_user_id: old.namespace.user_public_id().into(), auth_epoch: old.auth_epoch };
            let mut cleanup = RetiredSessionCleanup::new(self.backend.api.session().clone(), scope, self.cleanup_failure.clone());
            cleanup.armed = false;
            cleanup
        });
        let mut remote_logout = match &input {
            ActivationInput::Logout { token, all, .. } => Some(RetiredLogoutAttempt {
                api: self.backend.api.clone(), token: token.clone(), all: *all, armed: false, debt: self.cleanup_failure.clone(),
            }),
            _ => None,
        };
        if let ActivationInput::Logout { scope, .. } = &input {
            if old.as_ref().map(|lease| lease.auth_epoch == scope.auth_epoch && lease.namespace.user_public_id() == scope.owner_user_id) != Some(true) {
                return Err(ApiError::AuthenticationRequired);
            }
        }
        if let Some(old) = old.as_ref() {
            let quiesced = self.activity.begin_quiesce(old).map_err(transition_error)?;
            let flushed = self.writer.flush_for_retirement(old).map_err(transition_error)?;
            namespace_transition.retire_flushed(flushed).map_err(transition_error)?;
            retired_cleanup.as_mut().expect("old retirement owns cleanup").armed = true;
            if let Some(remote) = &mut remote_logout { remote.armed = true; }
            quiesced.retire();
            *self.active_namespace.lock().unwrap_or_else(|poison| poison.into_inner()) = None;
            self.billing.invalidate_for_auth_change();
            *self.current_user_id.lock().unwrap_or_else(|poison| poison.into_inner()) = None;
        }
        // Evaluate both independent outcomes before returning a local error. The owned
        // attempt is armed only after real retirement and still uses normal API gates.
        let ui_result = retired_ui();
        update_cleanup_debt(&self.cleanup_failure, |debt| debt.private_ui = ui_result.is_err());
        let cleanup_result = match &mut retired_cleanup {
            Some(cleanup) => cleanup.clear(),
            None if explicit_login => {
                // clear() retries only the manager's retained authorized record. It never
                // loads/adopts a replacement; successful deletion clears no writer/UI debt.
                let result = self.backend.api.session().clear();
                update_cleanup_debt(&self.cleanup_failure, |debt| debt.credentials = result.is_err());
                result
            }
            None => Ok(()),
        };
        let remote_result = match &mut remote_logout { Some(remote) => remote.attempt(), None => Ok(()) };
        if let Err(error) = &remote_result {
            if RequiredUpgrade::from_error(error).is_some() { return Err(error.clone()); }
        }
        if ui_result.is_err() || cleanup_result.is_err() || remote_result.is_err() {
            let mut messages = Vec::new();
            if ui_result.is_err() { messages.push("退出界面清理未确认"); }
            if cleanup_result.is_err() { messages.push(INCOMPLETE_CREDENTIAL_CLEANUP); }
            if remote_result.is_err() { messages.push("远程退出未确认"); }
            return Err(transition_error(messages.join("；")));
        }
        let (scope, suggestion) = match input {
            ActivationInput::Logout { .. } => return Ok(None),
            ActivationInput::Login { user, tokens, origin, suggested_group } => {
                let _origin = origin;
                let scope = self.backend.api.session().install_tokens_for_user(&tokens, &user.id)?;
                (scope, suggested_group)
            }
            ActivationInput::Resume { owner } => (self.backend.api.refresh_persisted_owner(&owner)?, None),
            ActivationInput::Switch { .. } | ActivationInput::PreparedSwitch { .. } => unreachable!("switches use the non-retiring branch"),
        };
        let mut pending = PendingActivation { core: self.clone(), scope: scope.clone(), writer_lease: None, committed: false, changes_user: true };
        self.billing.bind_authenticated_session(scope.clone())?;
        let listed = TeamApi::new(self.backend.api.clone()).list_groups(&scope)?;
        let saved = self.writer.load_selected_group(&scope.owner_user_id, &self.backend.api.device().id).map_err(transition_error)?;
        let choice = BillingContextManager::choose_candidate(&listed.items, saved.as_deref(), suggestion.as_deref())?.clone();
        let billing_ticket = self.billing.begin_switch(&scope, &self.backend.api.device().id, &choice.group_id, PreviousBillingAuthority::StillValid)?;
        let snapshot = AccountApi::new(self.backend.api.clone()).snapshot_billing(billing_ticket.proposed_scope())?;
        let staged = self.billing.stage_confirmation(&billing_ticket, choice, snapshot.account.clone())?;
        let lease = NamespaceLease {
            namespace: UserNamespace::new(&self.root_path, &scope.owner_user_id).map_err(transition_error)?,
            auth_epoch: scope.auth_epoch, namespace_epoch: ticket.id,
        };
        let authority = NamespaceStorageAuthority::open_prepublication(self.root.clone(), &lease).map_err(transition_error)?;
        let phase = namespace_transition.begin_prepublication_recovery(&lease).map_err(transition_error)?;
        phase.verify_no_unsupported_imports(&authority).map_err(transition_error)?;
        let recovered = phase.finish().map_err(transition_error)?;
        let data = self.writer.load_client_state_for_namespace(&lease).map_err(transition_error)?.unwrap_or_default();
        let profile = self.writer.load_client_user_profile_for_namespace(&lease).map_err(transition_error)?;
        let namespace = namespace_transition.prepare_publication(&lease, recovered).map_err(transition_error)?;
        let activity = self.activity.prepare_activation(lease.clone()).map_err(transition_error)?;
        self.writer.activate(lease.clone()).map_err(transition_error)?;
        pending.writer_lease = Some(lease.clone());
        Ok(Some(PreparedActivation { pending, namespace: Some(namespace), activity: Some(activity), lease, billing_ticket, staged, data: Some(data), profile, snapshot, groups: listed.items }))
    }
    fn prepare_billing_switch(self: &Arc<Self>, ticket: BillingSwitchTicket, choice: AccountGroupChoice) -> Result<PreparedActivation, ApiError> {
        let scope = ticket.proposed_scope().request.session.clone();
        let lease = self.active_namespace.lock().map_err(transition_error)?.clone().filter(|lease| lease.auth_epoch == scope.auth_epoch && lease.namespace.user_public_id() == scope.owner_user_id).ok_or(ApiError::AuthenticationRequired)?;
        let snapshot = AccountApi::new(self.backend.api.clone()).snapshot_billing(ticket.proposed_scope())?;
        let staged = self.billing.stage_confirmation(&ticket, choice, snapshot.account.clone())?;
        let groups = TeamApi::new(self.backend.api.clone()).list_groups(&scope)?.items;
        let pending = PendingActivation { core: self.clone(), scope, writer_lease: None, committed: false, changes_user: false };
        Ok(PreparedActivation { pending, namespace: None, activity: None, lease, billing_ticket: ticket, staged, data: None, profile: None, snapshot, groups })
    }
}
impl PreparedActivation {
    fn persist(mut self) -> Result<PersistedActivation, ApiError> {
        let core = self.pending.core.clone();
        let permit = core.backend.api.upgrade_latch().begin_ordinary_durable_commit().map_err(|required| required.as_error())?;
        if let Some(profile)=self.profile.clone() {
            core.writer.persist_client_user_profile_checked_for_namespace(&self.lease,profile).map_err(transition_error)?;
        }
        let result = core.writer.save_selected_group(
            &self.lease.namespace.user_public_id(), self.billing_ticket.device_installation_id(),
            &self.billing_ticket.proposed_scope().request.account_group_id,
        ).map_err(transition_error);
        if let Err(error) = result {
            drop(self);
            drop(permit);
            return Err(error);
        }
        // Acknowledged disk selection is irreversible by ordinary ticket Drop.
        self.billing_ticket.suppress_rollback();
        Ok(PersistedActivation { prepared: self, _commit: permit })
    }
}

#[derive(Clone, Default)]
pub(super) struct UserActivityGate { inner: Arc<ActivityInner> }
/// Runtime binding installed once by the real activation coordinator. A session token alone
/// never admits a private worker; the exact published namespace and activity gate must agree.
#[derive(Clone)]
pub(super) struct UserWorkAdmission {
    active: Arc<Mutex<Option<NamespaceLease>>>, activity: UserActivityGate,
}
impl UserWorkAdmission {
    pub(super) fn new(active: Arc<Mutex<Option<NamespaceLease>>>, activity: UserActivityGate) -> Self { Self { active, activity } }
    pub(super) fn begin(&self, scope: &SessionScope) -> Result<UserActivityPermit> {
        let active = self.active.lock().map_err(|_| anyhow!("active namespace poisoned"))?;
        let lease = active.as_ref().filter(|lease| lease.auth_epoch == scope.auth_epoch
            && lease.namespace.user_public_id() == scope.owner_user_id)
            .ok_or_else(|| anyhow!("captured user namespace is inactive"))?;
        self.activity.begin_recovery_unit(lease)
    }
    pub(super) fn is_current(&self, scope: &SessionScope) -> bool { self.begin(scope).is_ok() }
}
#[derive(Default)]
struct ActivityInner { state: Mutex<ActivityState>, changed: Condvar }
#[derive(Default)]
struct ActivityState { lease: Option<NamespaceLease>, count: usize, quiescing: bool, closed: bool }
pub(super) struct UserActivityPermit { inner: Arc<ActivityInner>, lease: NamespaceLease }
pub(super) struct QuiescedUserActivity { inner: Arc<ActivityInner>, lease: NamespaceLease, retired: bool }
struct PreparedUserActivity { inner: Arc<ActivityInner>, lease: NamespaceLease }
impl PreparedUserActivity {
    fn publish(self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|poison| {
            let mut state = poison.into_inner(); state.closed = true; state
        });
        if !state.closed { state.lease = Some(self.lease); }
    }
}
impl UserActivityGate {
    fn prepare_activation(&self, lease: NamespaceLease) -> Result<PreparedUserActivity> {
        let state = self.inner.state.lock().map_err(|_| anyhow!("activity gate poisoned"))?;
        anyhow::ensure!(!state.closed && state.lease.is_none() && state.count == 0 && !state.quiescing, "activity cannot be prepared");
        Ok(PreparedUserActivity { inner: self.inner.clone(), lease })
    }
    pub(super) fn activate(&self, lease: NamespaceLease) -> Result<()> {
        let mut state = self.inner.state.lock().map_err(|_| anyhow!("activity gate poisoned"))?;
        anyhow::ensure!(!state.closed && state.lease.is_none() && state.count == 0 && !state.quiescing, "activity already bound");
        state.lease = Some(lease);
        Ok(())
    }
    pub(super) fn begin_recovery_unit(&self, lease: &NamespaceLease) -> Result<UserActivityPermit> {
        let mut state = self.inner.state.lock().map_err(|_| anyhow!("activity gate poisoned"))?;
        anyhow::ensure!(!state.closed && !state.quiescing && state.lease.as_ref() == Some(lease), "user activity is closed or stale");
        let Some(count) = state.count.checked_add(1) else { state.closed = true; return Err(anyhow!("activity counter exhausted")); };
        state.count = count;
        Ok(UserActivityPermit { inner: self.inner.clone(), lease: lease.clone() })
    }
    // Blocking drain is used only by the activation worker, never the UI thread.
    pub(super) fn begin_quiesce(&self, lease: &NamespaceLease) -> Result<QuiescedUserActivity> {
        let mut state = self.inner.state.lock().map_err(|_| anyhow!("activity gate poisoned"))?;
        anyhow::ensure!(!state.closed && !state.quiescing && state.lease.as_ref() == Some(lease), "cannot quiesce stale activity");
        state.quiescing = true;
        while state.count != 0 {
            state = self.inner.changed.wait(state).map_err(|_| anyhow!("activity drain poisoned"))?;
        }
        Ok(QuiescedUserActivity { inner: self.inner.clone(), lease: lease.clone(), retired: false })
    }
}
impl UserActivityPermit {
    pub(super) fn is_quiescing(&self) -> bool {
        self.inner.state.lock().map(|state| state.closed || state.quiescing || state.lease.as_ref() != Some(&self.lease)).unwrap_or(true)
    }
}
impl Drop for UserActivityPermit {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|poison| poison.into_inner());
        if state.lease.as_ref() == Some(&self.lease) {
            state.count = state.count.saturating_sub(1);
            self.inner.changed.notify_all();
        }
    }
}
impl QuiescedUserActivity {
    pub(super) fn retire(mut self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|poison| poison.into_inner());
        if state.lease.as_ref() == Some(&self.lease) {
            state.lease = None;
            state.quiescing = false;
        }
        self.retired = true;
    }
}
impl Drop for QuiescedUserActivity {
    fn drop(&mut self) {
        if self.retired { return; }
        let mut state = self.inner.state.lock().unwrap_or_else(|poison| poison.into_inner());
        if !state.closed && state.lease.as_ref() == Some(&self.lease) {
            state.quiescing = false;
            self.inner.changed.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_activation_failure_replaces_the_wechat_success_placeholder() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();

        present_activation_failure(
            &state,
            Some(LoginOrigin::Wechat),
            "服务响应异常，请稍后重试".into(),
        );

        assert_eq!(state.get_auth_error(), state.get_auth_wechat_status());
        assert!(state.get_auth_error().starts_with("登录验证已通过，但客户端未能完成登录："));
        assert!(!state.get_auth_wechat_status().contains("登录成功"));
        state.set_auth_code("123456".into());
        present_activation_failure(&state, Some(LoginOrigin::Email), "无法保存登录状态".into());
        assert!(state.get_auth_code().is_empty());
        assert!(state.get_auth_error().contains("请重新获取验证码"));
    }

    // Real coordinator entry points, HTTP snapshots, writer acknowledgements,
    // timers and private publication; no fabricated Saved mailbox.
    mod actual_matrix {
        use super::*;
        use std::io::Write;
        use std::sync::atomic::AtomicBool;
        const USER_A: &str = "11111111-1111-4111-8111-111111111111";
        const GROUP_A: &str = "55555555-5555-4555-8555-555555555555";
        const GROUP_B: &str = "44444444-4444-4444-8444-444444444444";

        fn choice(id: &str, name: &str, version: &str) -> serde_json::Value {
            serde_json::json!({"group_id":id,"name":name,"group_status":"active","role":"owner",
                "member_id":null,"relationship_status":null,"readable_context":true,"selectable":true,
                "group_version":version,"membership_version":null,"capabilities":["bill","manage_group"],"quota":null})
        }
        fn snapshot(user: &str, selected: serde_json::Value) -> serde_json::Value {
            serde_json::json!({"user":{"id":user,"email_masked":"matrix***@example.com","nickname":"Confirmed profile",
                "status":"active","registered_at":"2026-09-07T00:00:00Z"},"read_only":false,
                "capabilities":["bill","manage_group"],"billing_group":selected,"membership":null,
                "credits":null,"quota":null,"entitlement":{}})
        }
        struct Http {
            url: String, stop: Arc<AtomicBool>, requests: Arc<Mutex<Vec<String>>>,
            worker: Option<std::thread::JoinHandle<()>>,
        }
        impl Http {
            fn new(user: &'static str, selected_group: &'static str, name: &'static str) -> Self {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                listener.set_nonblocking(true).unwrap();
                let url = format!("http://{}/", listener.local_addr().unwrap());
                let stop = Arc::new(AtomicBool::new(false)); let cancelled = stop.clone();
                let requests = Arc::new(Mutex::new(Vec::new())); let observed = requests.clone();
                let selected = Arc::new(Mutex::new(choice(selected_group, name, "1")));
                let worker = std::thread::spawn(move || {
                    let mut handles = Vec::new();
                    while !cancelled.load(Ordering::Acquire) {
                        match listener.accept() {
                            Ok((mut stream, _)) => {
                                let observed = observed.clone(); let selected = selected.clone();
                                handles.push(std::thread::spawn(move || {
                                    // Darwin inherits O_NONBLOCK on accepted sockets. Empty
                                    // speculative connections must not consume an API slot.
                                    stream.set_nonblocking(false).unwrap();
                                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                                    match stream.peek(&mut [0u8; 1]) {
                                        Ok(0) => return,
                                        Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return,
                                        Ok(_) => {}, Err(error) => panic!("matrix request peek: {error}"),
                                    }
                                    let request = backend_generation::billing_capture_test_support::read_request(&mut stream);
                                    let line = request.lines().next().unwrap();
                                    let mut parts = line.split_whitespace();
                                    let method = parts.next().unwrap(); let path = parts.next().unwrap();
                                    let header = |name: &str| request.lines().find_map(|line| line.split_once(':')
                                        .filter(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.trim()));
                                    assert_eq!(header("x-token"), Some(if user == USER_A { "fixture-access" } else { "new-B-access" }));
                                    assert_eq!(header("x-account-group-id"), matches!(path, "/v1/account" | "/v1/models").then_some(selected_group));
                                    let (status, data, error) = if method == "PATCH" {
                                        assert_eq!(path, format!("/v1/account-groups/{selected_group}"));
                                        assert!(Uuid::parse_str(header("idempotency-key").expect("mutation key")).is_ok());
                                        let body: serde_json::Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                                        assert_eq!(body, serde_json::json!({"name":"Renamed team","expected_version":"1"}));
                                        *selected.lock().unwrap() = choice(selected_group, "Renamed team", "2");
                                        ("200 OK", selected.lock().unwrap().clone(), serde_json::Value::Null)
                                    } else {
                                        assert_eq!(method, "GET");
                                        match path {
                                            "/v1/account-groups" => ("200 OK", serde_json::json!({"items":[selected.lock().unwrap().clone()],"pending_invitation_count":0}), serde_json::Value::Null),
                                            "/v1/account" => ("200 OK", snapshot(user, selected.lock().unwrap().clone()), serde_json::Value::Null),
                                            "/v1/models" | "/v1/account/sessions" => ("200 OK", serde_json::json!({"items":[]}), serde_json::Value::Null),
                                            "/v1/account/invitation" => ("503 Service Unavailable", serde_json::Value::Null,
                                                serde_json::json!({"code":"service_unavailable","message":"optional fixture invitation unavailable","details":null})),
                                            _ => panic!("unexpected matrix route: {path}"),
                                        }
                                    };
                                    observed.lock().unwrap().push(line.to_owned());
                                    let body = serde_json::json!({"request_id":"matrix-local-only","data":data,"error":error,"meta":null}).to_string();
                                    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                                }));
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
                            Err(error) => panic!("matrix accept: {error}"),
                        }
                    }
                    let mut failed = false;
                    for handle in handles { failed |= handle.join().is_err(); }
                    assert!(!failed, "matrix HTTP handler failed");
                });
                Self { url, stop, requests, worker: Some(worker) }
            }
            fn finish(&mut self) {
                self.stop.store(true, Ordering::Release);
                if let Some(worker) = self.worker.take() { worker.join().unwrap(); }
            }
        }
        impl Drop for Http {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::Release);
                if let Some(worker) = self.worker.take() {
                    let joined = worker.join();
                    if !std::thread::panicking() { joined.unwrap(); }
                }
            }
        }
        struct Fixture {
            context: AppContext, coordinator: Rc<AccountTransitionCoordinator>,
            original_lease: NamespaceLease, original_scope: SessionScope,
            // Root/writer fixture is dropped only after the explicit worker drain.
            _storage: client_state::tests::Fixture,
        }
        impl Fixture {
            fn new(url: &str) -> (Self, AppWindow) {
                i_slint_backend_testing::init_no_event_loop();
                let (storage, core, scope, lease) = active_transition_fixture_at(false, url);
                let index = FileIndex::initialize(core.root_path.join("matrix-index.sqlite3")).unwrap();
                let coordinator = Rc::new(AccountTransitionCoordinator { core: core.clone(), pending: RefCell::new(None),
                    retired_workers: RefCell::new(Vec::new()), worker_failed: Arc::new(false.into()) });
                let context = AppContext { backend: Some(core.backend.clone()), billing_context: core.billing.clone(),
                    namespace_operations: core.namespace_operations.clone(), user_activity: core.activity.clone(),
                    active_namespace: core.active_namespace.clone(), current_user_id: core.current_user_id.clone(),
                    data_root_capability: Some(core.root.clone()), file_index: Some(index.clone()),
                    account_snapshot_scope: Arc::new(Mutex::new(Some(scope.clone()))),
                    account_transition: Some(coordinator.clone()), ..Default::default() };
                let persistence = PrivatePersistence::new(core.writer.clone(), lease.clone(), core.activity.clone(),
                    core.backend.api.upgrade_latch().clone()).with_storage(core.root.clone(), core.backend.api.clone(), index);
                context.store.borrow_mut().private_persistence = Some(persistence.clone());
                context.store.borrow_mut().custom_prompts = vec!["A private draft".into()];
                context.billing_context.bind_authenticated_session(scope.clone()).unwrap();
                let account: AccountSnapshot = serde_json::from_value(snapshot(USER_A, choice(GROUP_A, "Original team", "1"))).unwrap();
                let ticket = core.billing.begin_switch(&scope, &core.backend.api.device().id, GROUP_A, PreviousBillingAuthority::StillValid).unwrap();
                let staged = core.billing.stage_confirmation(&ticket, account.billing_group.clone(), account).unwrap();
                core.writer.save_selected_group(USER_A, &core.backend.api.device().id, GROUP_A).unwrap();
                core.billing.publish_persisted(ticket, staged);
                context.team_groups.replace(vec![serde_json::from_value(choice(GROUP_A, "Original team", "1")).unwrap(),
                    serde_json::from_value(choice(GROUP_B, "Second team", "1")).unwrap()]);
                let app = AppWindow::new().unwrap(); let state = app.global::<AppState>();
                state.set_session_state("online".into()); state.set_logged_in(true);
                state.set_profile_open(true); state.set_account_center_section("accounts-teams".into()); state.set_team_tab("members".into());
                wire_team_callbacks(&app, context.clone()); render_team_context(&app, &context);
                persistence.save_store(local_store_data(&app, &context.store.borrow())).unwrap();
                persistence.save_profile(UserProfileData { nickname: "A retained profile".into(), ..Default::default() }).unwrap();
                (Self { context, coordinator, original_lease: lease, original_scope: scope, _storage: storage }, app)
            }
            fn pump(&self, app: &AppWindow, mut ready: impl FnMut() -> bool) {
                let deadline = Instant::now() + Duration::from_secs(8);
                loop {
                    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
                    slint::platform::update_timers_and_animations();
                    if ready() && self.coordinator.pending.borrow().is_none() && self.coordinator.retired_workers.borrow().is_empty() { break; }
                    assert!(Instant::now() < deadline, "actual matrix transition did not publish: auth={}, team={}",
                        app.global::<AppState>().get_auth_error(), app.global::<AppState>().get_account_group_error());
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            fn assert_private_a_unchanged(&self) {
                assert_eq!(self.context.store.borrow().custom_prompts, vec!["A private draft"]);
                assert_eq!(self.context.active_namespace.lock().unwrap().as_ref(), Some(&self.original_lease));
                assert_eq!(self.coordinator.core.writer.load_client_state_for_namespace(&self.original_lease).unwrap().unwrap().custom_prompts,
                    vec!["A private draft"]);
                assert_eq!(self.context.apply_user_completion(&self.original_lease, || "same user allowed").unwrap(), "same user allowed");
            }
            fn assert_durable_selection(&self, user: &str, group: &str) {
                let core = &self.coordinator.core;
                assert_eq!(core.writer.load_selected_group(user, &core.backend.api.device().id).unwrap().as_deref(), Some(group));
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let mut failures = Vec::new();
                macro_rules! collect { ($result:expr) => { if let Err(error) = $result { failures.push(error.to_string()); } }; }
                // Cancel first, then release ordinary-family activity before
                // joining a coordinator that may be waiting in quiesce.
                self.coordinator.cancel_pending_receiver();
                let mut owned_leases = vec![self.original_lease.clone()];
                if let Some(current) = self.context.active_namespace.lock().unwrap_or_else(|error| error.into_inner()).clone() {
                    if !owned_leases.contains(&current) { owned_leases.push(current); }
                }
                collect!(shutdown_team_workers()); collect!(shutdown_prompt_workers());
                dispose_pending_native_file_drag_for_shutdown();
                collect!(shutdown_reference_workers());
                collect!(shutdown_enhancement_workers());
                collect!(shutdown_cutout_workers());
                collect!(payment_callbacks::shutdown_payment_workers());
                collect!(notification_callbacks::shutdown_notification_workers());
                for lease in &owned_leases {
                    collect!(drain_video_player_workers_for_lease_for_test(lease));
                    collect!(toolbox_callbacks::drain_toolbox_workers_for_lease_for_test(lease));
                    collect!(drain_canvas_workers_for_lease_for_test(lease));
                }
                collect!(drain_activation_preview_workers_for_shutdown()); collect!(drain_delivery_commit_workers_for_shutdown());
                collect!(self.coordinator.shutdown());
                let current = self.context.active_namespace.lock().unwrap_or_else(|error| error.into_inner()).take();
                if let Some(lease) = current {
                    match self.context.user_activity.begin_quiesce(&lease) {
                        Ok(guard) => guard.retire(), Err(error) => failures.push(error.to_string()),
                    }
                }
                // Release callback-owned HTTP clients before Windows runs TLS
                // destructors under the loader lock. Dropping Slint's timer list
                // there would join reqwest's runtime while its exit needs that lock.
                let deadline = Instant::now() + Duration::from_secs(8);
                while let Some(delay) = slint::platform::duration_until_next_timer_update() {
                    i_slint_backend_testing::mock_elapsed_time(delay + Duration::from_millis(1));
                    slint::platform::update_timers_and_animations();
                    if Instant::now() >= deadline {
                        failures.push("matrix callbacks did not drain before thread exit".into());
                        break;
                    }
                }
                if !std::thread::panicking() { assert!(failures.is_empty(), "matrix drain failures: {failures:?}"); }
            }
        }

        // Catches a login that publishes B using A's Store/lease, skips durable
        // B selection/profile, or leaves old-A ordinary completions admitted.
        #[test]
        fn core_actual_coordinator_user_a_to_b_keeps_a_private_and_publishes_durable_b() {
            let transport = Http::new(CLEANUP_B, GROUP_B, "Second team");
            let (fixture, app) = Fixture::new(&transport.url); let mut http = transport;
            let ActivationInput::Login { user, tokens, origin, suggested_group } = cleanup_login_input() else { unreachable!() };
            fixture.coordinator.activate_identity(&app, fixture.context.clone(), user, tokens, origin, suggested_group);
            fixture.pump(&app, || fixture.context.current_account_session_scope().is_some_and(|scope| scope.owner_user_id == CLEANUP_B)
                && app.global::<AppState>().get_selected_account_group_id() == GROUP_B);
            http.finish();
            assert_eq!(http.requests.lock().unwrap().len(), 5);
            let lease_b = fixture.context.active_namespace.lock().unwrap().clone().unwrap();
            assert_ne!(lease_b, fixture.original_lease); assert_eq!(lease_b.namespace.user_public_id(), CLEANUP_B);
            assert!(fixture.context.store.borrow().custom_prompts.is_empty());
            let writer = &fixture.coordinator.core.writer;
            assert_eq!(writer.load_client_state_for_namespace(&fixture.original_lease).unwrap().unwrap().custom_prompts, vec!["A private draft"]);
            assert_eq!(writer.load_client_user_profile_for_namespace(&fixture.original_lease).unwrap().unwrap().nickname, "A retained profile");
            assert_eq!(writer.load_client_user_profile_for_namespace(&lease_b).unwrap().unwrap().nickname, "Confirmed profile");
            assert!(fixture.context.apply_user_completion(&fixture.original_lease, || panic!("old A applied to B")).is_err());
            assert_eq!(fixture.context.apply_user_completion(&lease_b, || "B current").unwrap(), "B current");
            fixture.assert_durable_selection(USER_A, GROUP_A); fixture.assert_durable_selection(CLEANUP_B, GROUP_B);
            assert!(!fixture.coordinator.core.backend.api.session().is_scope_current(&fixture.original_scope));
        }
        // Catches account-center switching that loses private Store data,
        // changes user namespace, or publishes without the SQLite selection ack.
        #[test]
        fn core_actual_coordinator_group_switch_preserves_private_store_and_retires_old_billing() {
            let transport = Http::new(USER_A, GROUP_B, "Second team");
            let (fixture, app) = Fixture::new(&transport.url); let mut http = transport;
            let old = fixture.context.billing_context.confirmed_scope().unwrap();
            app.global::<AppState>().invoke_switch_account_group(GROUP_B.into());
            fixture.pump(&app, || app.global::<AppState>().get_selected_account_group_id() == GROUP_B);
            http.finish(); assert_eq!(http.requests.lock().unwrap().len(), 5);
            fixture.assert_private_a_unchanged(); fixture.assert_durable_selection(USER_A, GROUP_B);
            assert!(!fixture.context.billing_context.is_current(&old));
            assert_eq!(fixture.context.billing_context.confirmed_scope().unwrap().request.session, fixture.original_scope);
            assert_eq!(fixture.context.billing_context.confirmed_snapshot().unwrap().billing_group.name, "Second team");
        }
        // Catches the old rename path which changes only visible list metadata
        // and leaves the confirmed snapshot/version and coordinator ack stale.
        #[test]
        fn core_actual_coordinator_selected_team_rename_reconfirms_metadata_and_saves() {
            let transport = Http::new(USER_A, GROUP_A, "Original team");
            let (fixture, app) = Fixture::new(&transport.url); let mut http = transport;
            let old = fixture.context.billing_context.confirmed_scope().unwrap();
            app.global::<AppState>().set_team_name_input("Renamed team".into());
            app.global::<AppState>().invoke_rename_team();
            fixture.pump(&app, || fixture.context.billing_context.confirmed_snapshot().is_some_and(|snapshot| snapshot.billing_group.group_version == "2"));
            http.finish(); assert_eq!(http.requests.lock().unwrap().len(), 7);
            fixture.assert_private_a_unchanged(); fixture.assert_durable_selection(USER_A, GROUP_A);
            assert!(!fixture.context.billing_context.is_current(&old));
            assert_eq!(fixture.context.billing_context.confirmed_snapshot().unwrap().billing_group.name, "Renamed team");
            assert_eq!(app.global::<AppState>().get_team_name_input(), "Renamed team");
            assert_eq!(fixture.coordinator.core.writer.load_client_user_profile_for_namespace(&fixture.original_lease).unwrap().unwrap().nickname, "Confirmed profile");
        }
        // Catches a same-ID early return which prevents fresh authority and
        // profile confirmation while looking like a successful selection.
        #[test]
        fn core_actual_coordinator_reselect_same_group_reconfirms_without_losing_private_data() {
            let transport = Http::new(USER_A, GROUP_A, "Synchronized team");
            let (fixture, app) = Fixture::new(&transport.url); let mut http = transport;
            let old = fixture.context.billing_context.confirmed_scope().unwrap();
            app.global::<AppState>().invoke_switch_account_group(GROUP_A.into());
            fixture.pump(&app, || fixture.context.billing_context.confirmed_snapshot().is_some_and(|snapshot| snapshot.billing_group.name == "Synchronized team"));
            http.finish(); assert_eq!(http.requests.lock().unwrap().len(), 5);
            fixture.assert_private_a_unchanged(); fixture.assert_durable_selection(USER_A, GROUP_A);
            assert!(!fixture.context.billing_context.is_current(&old));
            assert_eq!(app.global::<AppState>().get_team_name_input(), "Synchronized team");
            assert_eq!(fixture.coordinator.core.writer.load_client_user_profile_for_namespace(&fixture.original_lease).unwrap().unwrap().nickname, "Confirmed profile");
        }
    }

    struct ActivationCheckpointReset;
    impl Drop for ActivationCheckpointReset {
        fn drop(&mut self){ACTIVATION_WORKER_CHECKPOINT.with(|hook|hook.borrow_mut().take());}
    }
    struct ActivationTestDrain {
        coordinator:Rc<AccountTransitionCoordinator>,release:Option<mpsc::Sender<()>>,
    }
    impl Drop for ActivationTestDrain {
        fn drop(&mut self){
            if let Some(release)=self.release.take(){let _=release.send(());}
            let _=self.coordinator.shutdown();
        }
    }
    fn assert_actual_activation_worker_panic(target:&'static str) {
        i_slint_backend_testing::init_no_event_loop();
        let (listener,url)=backend_generation::billing_capture_test_support::listener();
        let (_fixture,core,_,_)=active_transition_fixture_at(false,&url);
        let app=AppWindow::new().unwrap();
        let context=AppContext {
            backend:Some(core.backend.clone()),billing_context:core.billing.clone(),
            namespace_operations:core.namespace_operations.clone(),user_activity:core.activity.clone(),
            active_namespace:core.active_namespace.clone(),current_user_id:core.current_user_id.clone(),
            data_root_capability:Some(core.root.clone()),
            file_index:Some(FileIndex::initialize(core.root_path.join("panic-index.sqlite3")).unwrap()),
            ..Default::default()
        };
        let coordinator=Rc::new(AccountTransitionCoordinator{core:core.clone(),pending:RefCell::new(None),
            retired_workers:RefCell::new(Vec::new()),worker_failed:Arc::new(false.into())});
        let transport=cleanup_login_transport(listener);
        let (entered_tx,entered_rx)=mpsc::channel();
        let (release_tx,release_rx)=mpsc::channel();let release_rx=Mutex::new(release_rx);
        ACTIVATION_WORKER_CHECKPOINT.with(|hook|*hook.borrow_mut()=Some(Arc::new(move|phase|{
            if phase==target {
                entered_tx.send(()).unwrap();let _=release_rx.lock().unwrap().recv();
                panic!("fixture private panic payload must not reach UI");
            }
        })));
        let _hook=ActivationCheckpointReset;
        let mut drain=ActivationTestDrain{coordinator:coordinator.clone(),release:Some(release_tx)};
        coordinator.start(&app,context.clone(),cleanup_login_input());
        let deadline=Instant::now()+Duration::from_secs(5);
        loop {
            if entered_rx.try_recv().is_ok(){break;}
            assert!(Instant::now()<deadline,"actual activation checkpoint was not reached: {target}");
            let advance=coordinator.pending.borrow().as_ref().is_some_and(|pending|{
                let phase=pending.mailbox.phase.lock().unwrap();
                matches!(&*phase,ActivationMailboxPhase::Retired)
                    || (target=="saved" && matches!(&*phase,ActivationMailboxPhase::Prepare(_)))
            });
            if advance {coordinator.advance(&app,&context);}
            std::thread::sleep(Duration::from_millis(2));
        }
        drain.release.take().unwrap().send(()).unwrap();
        let deadline=Instant::now()+Duration::from_secs(3);
        while coordinator.pending.borrow().as_ref().and_then(|pending|pending.worker.as_ref()).is_some_and(|worker|!worker.is_finished()) {
            assert!(Instant::now()<deadline,"actual panic worker failed to exit");
            std::thread::sleep(Duration::from_millis(2));
        }
        // Actual worker has exited: a nonterminal mailbox must not leave UI busy.
        let busy=coordinator.advance(&app,&context);
        let shutdown=coordinator.shutdown();
        transport.finish();
        assert!(!busy,"finished panic worker must be consumed, not polled forever");
        assert!(!app.global::<AppState>().get_auth_busy());
        assert!(!app.global::<AppState>().get_auth_error().contains("fixture private panic"));
        assert!(shutdown.is_err(),"panic failure must remain sticky even after UI reaping");
        assert!(coordinator.shutdown().is_err(),"empty shutdown must not erase a prior panic");
        assert!(core.admission.begin().is_err(),"failed coordinator cannot admit new work");
        if target!="working" {
            assert!(core.active_namespace.lock().unwrap().is_none());
            assert!(core.backend.api.session().persisted_owner_user_id().is_none());
        }
    }
    #[test]
    fn core_activation_actual_working_panic_closes_admission_and_shutdown_reports_failure(){
        assert_actual_activation_worker_panic("working");
    }
    #[test]
    fn core_activation_actual_lost_window_retains_original_worker_until_joined_shutdown(){
        i_slint_backend_testing::init_no_event_loop();
        let (_fixture,core,_,_)=active_transition_fixture(false);
        let app=AppWindow::new().unwrap();
        let context=AppContext {
            backend:Some(core.backend.clone()),billing_context:core.billing.clone(),
            namespace_operations:core.namespace_operations.clone(),user_activity:core.activity.clone(),
            active_namespace:core.active_namespace.clone(),current_user_id:core.current_user_id.clone(),
            data_root_capability:Some(core.root.clone()),
            file_index:Some(FileIndex::initialize(core.root_path.join("lost-window-index.sqlite3")).unwrap()),
            ..Default::default()
        };
        let coordinator=Rc::new(AccountTransitionCoordinator{core:core.clone(),pending:RefCell::new(None),
            retired_workers:RefCell::new(Vec::new()),worker_failed:Arc::new(false.into())});
        let (entered_tx,entered_rx)=mpsc::channel();let (release_tx,release_rx)=mpsc::channel();
        let release_rx=Mutex::new(release_rx);
        ACTIVATION_WORKER_CHECKPOINT.with(|hook|*hook.borrow_mut()=Some(Arc::new(move|phase|{
            if phase=="working" {entered_tx.send(()).unwrap();let _=release_rx.lock().unwrap().recv();}
        })));
        let _hook=ActivationCheckpointReset;
        let mut drain=ActivationTestDrain{coordinator:coordinator.clone(),release:Some(release_tx)};
        coordinator.start(&app,context.clone(),cleanup_login_input());
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let original=coordinator.pending.borrow().as_ref().unwrap().worker.as_ref().unwrap().thread().id();
        drop(app);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
        slint::platform::update_timers_and_animations();
        assert!(coordinator.pending.borrow().is_none());
        {
            let workers=coordinator.retired_workers.borrow();assert_eq!(workers.len(),1);
            let handle=workers[0].worker.as_ref().unwrap();
            assert_eq!(handle.thread().id(),original);assert!(!handle.is_finished());
            assert!(workers[0].mailbox.cancelled.load(Ordering::SeqCst));
        }
        drain.release.take().unwrap().send(()).unwrap();
        coordinator.shutdown().unwrap();
        assert!(coordinator.retired_workers.borrow().is_empty());
        assert!(core.active_namespace.lock().unwrap().is_none());
        assert!(core.backend.api.session().persisted_owner_user_id().is_none());
        assert!(core.admission.begin().is_err());
    }
    #[test]
    fn core_activation_actual_prepared_panic_cleans_candidate_before_safe_failed_ui(){
        assert_actual_activation_worker_panic("prepare");
    }
    #[test]
    fn core_activation_actual_saved_panic_cleans_unpublished_candidate_before_safe_failed_ui(){
        assert_actual_activation_worker_panic("saved");
    }

    #[test]
    fn core_activation_publication_uses_complete_pre_save_models_for_fresh_store_and_billing_switch() {
        i_slint_backend_testing::init_no_event_loop();
        for (fresh_store, failure) in [(true,None),(false,None),(true,Some("profile")),(true,Some("selection"))] {
            let (_fixture,core,scope,lease)=active_transition_fixture(false);
            let app=AppWindow::new().unwrap();
            let context=AppContext {
                backend:Some(core.backend.clone()),billing_context:core.billing.clone(),
                namespace_operations:core.namespace_operations.clone(),user_activity:core.activity.clone(),
                active_namespace:core.active_namespace.clone(),current_user_id:core.current_user_id.clone(),
                data_root_capability:Some(core.root.clone()),
                file_index:Some(FileIndex::initialize(core.root_path.join("projection-index.sqlite3")).unwrap()),
                ..Default::default()
            };
            core.billing.bind_authenticated_session(scope.clone()).unwrap();
            let account:AccountSnapshot=serde_json::from_value(serde_json::json!({
                "user":{"id":scope.owner_user_id,"email_masked":"before@example.com","nickname":"Prepared owner","status":"active","registered_at":"2026-09-07T00:00:00Z"},
                "read_only":false,"capabilities":["bill","read_group_finance","manage_group","purchase","redeem"],
                "billing_group":{"group_id":CLEANUP_GROUP,"name":"Prepared team","group_status":"active","role":"owner",
                    "member_id":null,"relationship_status":null,"readable_context":true,"selectable":true,"group_version":"1",
                    "membership_version":null,"capabilities":["bill","read_group_finance","manage_group","purchase","redeem"],"quota":null},
                "membership":null,"credits":{"available":"91","reserved":"2","lifetime_granted":"100","lifetime_spent":"9","version":"7"},
                "quota":null,"entitlement":{}
            })).unwrap();
            let choice=account.billing_group.clone();
            let ticket=core.billing.begin_switch(&scope,&core.backend.api.device().id,CLEANUP_GROUP,PreviousBillingAuthority::StillValid).unwrap();
            let staged=core.billing.stage_confirmation(&ticket,choice.clone(),account.clone()).unwrap();
            let snapshot=BackendSnapshot {
                account, models:Some(vec![serde_json::from_value(serde_json::json!({"code":"before-image","version":1,"purpose":"image_generation",
                    "name":"Prepared image","capabilities":{},"prices":[]})).unwrap()]),
                packs:Some(vec![serde_json::from_value(serde_json::json!({"code":"before-pack","name":"Prepared pack","price_cents":"10000","credits":"100"})).unwrap()]),
                plans:Some(vec![]),ledger:Some(vec![]),ledger_next_cursor:Some("prepared-cursor".into()),
                orders:None,owner_billing:None,invitation:None,
                sessions:vec![AccountSessionDto { id:"before-session".into(),device_name:"Prepared device".into(),platform:"macos".into(),
                    app_version:"fixture".into(),last_seen_at:"2026-09-07T00:00:00Z".into(),is_current:true }]
            };
            let mut prepared=PreparedActivation {
                pending:PendingActivation { core:core.clone(),scope,writer_lease:None,committed:false,changes_user:false },
                namespace:None,activity:None,lease:lease.clone(),billing_ticket:ticket,staged,
                data:fresh_store.then(LocalStoreData::default),profile:None,snapshot,groups:vec![choice]
            };
            let authority=NamespaceStorageAuthority::open(core.root.clone(),&lease).unwrap();
            let mut png=std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(3,5,image::Rgba([15,40,90,255])).write_to(&mut png,image::ImageFormat::Png).unwrap();
            std::thread::scope(|threads| threads.spawn(|| {
                let key=ManagedFileKey::new(ManagedUserArea::Input,"projection.png").unwrap();
                let mut file=authority.create_new_regular(&key).unwrap();
                authority.write_new_regular_from(&mut file,&mut png.into_inner().as_slice()).unwrap();
                authority.sync_regular(&mut file).unwrap();
            }).join().unwrap());
            let source=lease.namespace.path(ManagedUserArea::Input).join("projection.png").to_string_lossy().into_owned();
            let row=|id:&str,category:&str,conversation:&str| serde_json::json!({
                "id":id,"conversation_id":conversation,"title":id,"category":category,"kind":"game",
                "time":"2026-09-07 12:00","prompt":"retained private prompt","ratio":"1:1","quality":"1K",
                "model":"retained-model","source_path":source,"reference_paths":[],"width":3,"height":5
            });
            let original:LocalStoreData=serde_json::from_value(serde_json::json!({
                "assets":[row("one","character","conversation-one"),row("two","scene","conversation-two")],
                "generations":[row("one","character","conversation-one"),row("two","scene","conversation-two")],
                "references":{"character":[{"id":"reference-one","source_path":source}],"scene":[],"ui":[],"effect":[]}
            })).unwrap();
            core.writer.persist_client_state_checked_for_namespace(&lease,original).unwrap();
            if fresh_store { prepared.data=core.writer.load_client_state_for_namespace(&lease).unwrap(); }
            let models=PreparedPrivateModels::new(&app,&mut prepared,&context).unwrap();
            assert!(!app.global::<AppState>().get_logged_in());
            assert_eq!(app.global::<AppState>().get_catalog_models().row_count(),0);
            assert!(core.writer.load_selected_group(lease.namespace.user_public_id(),&core.backend.api.device().id).unwrap().is_none());
            // A later snapshot cannot be used to finish only half of a saved publication.
            prepared.snapshot.models.as_mut().unwrap()[0].name="late mutated model".into();
            prepared.snapshot.packs.as_mut().unwrap()[0].name="late mutated pack".into();
            prepared.snapshot.sessions[0].device_name="late mutated device".into();
            prepared.groups[0].name="late mutated team".into();
            if let Some(failure)=failure {
                let connection=rusqlite::Connection::open(core.root_path.join("fixture.sqlite3")).unwrap();
                if failure=="profile" {
                    connection.execute_batch("CREATE TRIGGER refuse_profile BEFORE INSERT ON user_settings WHEN NEW.key='user_profile' BEGIN SELECT RAISE(ABORT,'fixture profile refusal'); END;").unwrap();
                } else {
                    connection.execute_batch("CREATE TRIGGER refuse_selection BEFORE INSERT ON billing_context_preferences BEGIN SELECT RAISE(ABORT,'fixture selected refusal'); END;").unwrap();
                }
                assert!(prepared.persist().is_err());
                assert!(core.writer.load_selected_group(lease.namespace.user_public_id(),&core.backend.api.device().id).unwrap().is_none());
                assert!(!app.global::<AppState>().get_logged_in());
                let profile=core.writer.load_client_user_profile_for_namespace(&lease).unwrap();
                assert_eq!(profile.is_some(),failure=="selection");
                assert_eq!(core.active_namespace.lock().unwrap().as_ref(),Some(&lease));
                continue;
            }
            let persisted=prepared.persist().unwrap();
            let profile=core.writer.load_client_user_profile_for_namespace(&lease).unwrap().expect("profile must acknowledge before selected publication");
            assert_eq!(profile.nickname,"Prepared owner");
            assert!(!profile.logged_in);
            let mailbox=Arc::new(ActivationMailbox::new());
            mailbox.put(ActivationMailboxPhase::Saved(Box::new(persisted)));
            let coordinator=AccountTransitionCoordinator { core:core.clone(),
                pending:RefCell::new(Some(ActivationUi { mailbox,models:Some(models),worker:None,login_origin:None })),
                retired_workers:RefCell::new(Vec::new()),worker_failed:Arc::new(false.into()) };
            assert!(!coordinator.advance(&app,&context));
            let state=app.global::<AppState>();
            assert!(state.get_logged_in());
            assert_eq!(state.get_catalog_models().row_data(0).unwrap().name,"Prepared image");
            assert_eq!(state.get_credit_packs().row_data(0).unwrap().name,"Prepared pack");
            assert_eq!(state.get_account_sessions().row_data(0).unwrap().device_name,"Prepared device");
            assert_eq!(state.get_account_groups().row_data(0).unwrap().name,"Prepared team");
            assert_eq!(state.get_credit_balance(),"91");
            if fresh_store {
                assert_eq!(state.get_conversations().row_count(),2);
                assert_eq!(state.get_current_conversation_id(),"conversation-one");
                assert_eq!(state.get_generation_visible_limit(),1);
                assert_eq!(state.get_generation_layout_items().row_count(),1);
                assert!(state.get_generation_layout_height()>0.0);
                assert_eq!(state.get_asset_all_count(),2);
                assert_eq!(state.get_references().row_count(),1);
                let deadline=Instant::now()+Duration::from_secs(4);
                while state.get_references().row_data(0).unwrap().image.size().width==0 && Instant::now()<deadline {
                    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
                    slint::platform::update_timers_and_animations();
                    std::thread::sleep(Duration::from_millis(2));
                }
                assert!(state.get_references().row_data(0).unwrap().image.size().width>0);
                drain_activation_preview_workers_for_shutdown().unwrap();
            }
            assert_eq!(state.get_selected_account_group_id().as_str(),CLEANUP_GROUP);
            assert!(context.account_snapshot_scope.lock().unwrap().as_ref().is_some_and(|scope|scope.owner_user_id==lease.namespace.user_public_id()));
        }
    }

    fn core_video_output_snapshot(lease: &NamespaceLease) -> serde_json::Value {
        let task = "77777777-7777-4777-8777-777777777777";
        let file = "88888888-8888-4888-8888-888888888888";
        let path = lease.namespace.path(ManagedUserArea::Videos).join(format!("{file}.mp4"));
        serde_json::json!({"video_outputs":{format!("{task}:{file}"): {
            "source_asset_id":"",
            "client_request_id":"original-video-key","server_task_id":task,"file_id":file,
            "billing_account_group_id":"22222222-2222-4222-8222-222222222222",
            "sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size_bytes":24,
            "model":"","resolution":"","duration_secs":0,"source_path":path.to_string_lossy(),"title":"Saved video","created_at":"2026-09-08T00:00:00Z"
        }}})
    }
    #[test]
    fn core_video_output_metadata_round_trips_through_real_ordered_writer_and_private_store() {
        let (_fixture, core, _scope, lease) = active_transition_fixture(false);
        let persistence = PrivatePersistence::for_test(core.writer.clone(), lease.clone(), core.activity.clone(), core.backend.api.upgrade_latch().clone());
        let expected = core_video_output_snapshot(&lease);
        let data: LocalStoreData = serde_json::from_value(expected.clone()).unwrap();
        let receiver = persistence.prepare_ordered_save().unwrap().enqueue(data).unwrap();
        receiver.recv().unwrap().unwrap();
        let restored = core.writer.load_client_state_for_namespace(&lease).unwrap().unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap()["video_outputs"], expected["video_outputs"]);
        let prepared = prepare_private_store(restored);
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        assert_eq!(serde_json::to_value(local_store_data(&app, &prepared.store)).unwrap()["video_outputs"], expected["video_outputs"]);
        let other = NamespaceLease { namespace: UserNamespace::new(&core.root_path, CLEANUP_B).unwrap(), auth_epoch: lease.auth_epoch + 1, namespace_epoch: lease.namespace_epoch + 1 };
        assert!(core.writer.load_client_state_for_namespace(&other).unwrap().is_none());
    }
    #[test]
    fn core_video_saved_output_source_ids_round_trip_without_cross_image_or_legacy_adoption(){
        let (_fixture,core,_,lease)=active_transition_fixture(false);
        let persistence=PrivatePersistence::for_test(core.writer.clone(),lease.clone(),core.activity.clone(),core.backend.api.upgrade_latch().clone());
        let mut expected=core_video_output_snapshot(&lease);
        let first=expected["video_outputs"].as_object().unwrap().keys().next().unwrap().clone();
        expected["video_outputs"][&first]["source_asset_id"]=serde_json::json!("source-A");
        let mut second=expected["video_outputs"][&first].clone();
        second["source_asset_id"]=serde_json::json!("source-B");
        second["client_request_id"]=serde_json::json!("second-key");
        second["file_id"]=serde_json::json!("99999999-9999-4999-8999-999999999999");
        second["source_path"]=serde_json::json!(lease.namespace.path(ManagedUserArea::Videos).join("99999999-9999-4999-8999-999999999999.mp4").to_string_lossy());
        let second_key="77777777-7777-4777-8777-777777777777:99999999-9999-4999-8999-999999999999";
        expected["video_outputs"][second_key]=second;
        persistence.save_store(serde_json::from_value(expected.clone()).unwrap()).unwrap();
        let saved=core.writer.load_client_state_for_namespace(&lease).unwrap().unwrap();
        let actual=serde_json::to_value(saved).unwrap();
        assert_eq!(actual["video_outputs"][&first]["source_asset_id"],"source-A");
        assert_eq!(actual["video_outputs"][second_key]["source_asset_id"],"source-B");
        let mut changed=actual.clone();changed["video_outputs"][&first]["source_asset_id"]=serde_json::json!("source-B");
        assert!(persistence.save_store(serde_json::from_value(changed).unwrap()).is_err());
        assert_eq!(serde_json::to_value(core.writer.load_client_state_for_namespace(&lease).unwrap().unwrap()).unwrap(),actual);
        let mut missing_source=core_video_output_snapshot(&lease);
        missing_source["video_outputs"][&first].as_object_mut().unwrap().remove("source_asset_id");
        let legacy:LocalStoreData=serde_json::from_value(missing_source).unwrap();
        let legacy=serde_json::to_value(legacy).unwrap();
        assert_eq!(legacy["video_outputs"][&first]["source_asset_id"],"");
    }
    #[test]
    fn core_video_output_identity_cannot_be_replaced_or_erased_by_an_old_sqlite_snapshot() {
        let (_fixture, core, _scope, lease) = active_transition_fixture(false);
        let persistence = PrivatePersistence::for_test(core.writer.clone(), lease.clone(), core.activity.clone(), core.backend.api.upgrade_latch().clone());
        let expected = core_video_output_snapshot(&lease);
        persistence.save_store(serde_json::from_value(expected.clone()).unwrap()).unwrap();
        let key = expected["video_outputs"].as_object().unwrap().keys().next().unwrap().clone();
        for field in ["client_request_id", "server_task_id", "file_id", "billing_account_group_id", "sha256", "source_path"] {
            let mut changed = expected.clone();
            changed["video_outputs"][&key][field] = serde_json::json!("different-original-identity");
            assert!(persistence.save_store(serde_json::from_value(changed).unwrap()).is_err(), "{field}");
            assert_eq!(serde_json::to_value(core.writer.load_client_state_for_namespace(&lease).unwrap().unwrap()).unwrap()["video_outputs"], expected["video_outputs"]);
        }
        assert!(persistence.save_store(LocalStoreData::default()).is_err(), "old snapshot cannot erase retained videos");
        assert_eq!(serde_json::to_value(core.writer.load_client_state_for_namespace(&lease).unwrap().unwrap()).unwrap()["video_outputs"], expected["video_outputs"]);
    }
    #[test]
    fn core_video_output_missing_legacy_field_defaults_empty_without_adopting_global_files() {
        let data: LocalStoreData = serde_json::from_str("{}").unwrap();
        assert_eq!(serde_json::to_value(data).unwrap()["video_outputs"], serde_json::json!({}));
    }
fn select_finance_fixture_group(context: &AppContext, session: &SessionScope, payer: &str, owner: bool) {
        let caps = if owner { vec!["bill", "redeem", "read_group_finance"] } else { vec!["bill"] };
        let snapshot: AccountSnapshot = serde_json::from_value(serde_json::json!({
            "user":{"id":session.owner_user_id,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
            "read_only":false,"capabilities":caps,"membership":null,"entitlement":{},"credits":null,"quota":null,
            "billing_group":{"group_id":payer,"name":"fixture","group_status":"active","role":if owner {"owner"}else{"member"},
                "member_id":if owner {None}else{Some("44444444-4444-4444-8444-444444444444")},
                "relationship_status":if owner {None}else{Some("active")},"readable_context":true,"selectable":true,
                "group_version":"1","membership_version":if owner {None}else{Some("1")},"capabilities":caps,"quota":null}
        })).unwrap();
        let ticket = context.billing_context.begin_switch(session, "fixture-device", payer, PreviousBillingAuthority::StillValid).unwrap();
        let staged = context.billing_context.stage_confirmation(&ticket, snapshot.billing_group.clone(), snapshot).unwrap();
        context.billing_context.publish_persisted(ticket, staged);
    }
    #[test]
    fn core_actual_redemption_replays_sqlite_payer_after_team_switch_and_preserves_denial() {
        use std::io::Write;
        use backend_generation::billing_capture_test_support::{listener, read_request};
        let (listener, url) = listener();
        let (_fixture, core, session, lease) = active_transition_fixture_at(false, &url);
        let context = AppContext {
            data_root_capability: Some(core.root.clone()), file_index: Some(FileIndex::initialize(core.root_path.join("finance-index.sqlite3")).unwrap()),
            backend: Some(core.backend.clone()), active_namespace: core.active_namespace.clone(), namespace_operations: core.namespace_operations.clone(),
            user_activity: core.activity.clone(), billing_context: core.billing.clone(), current_user_id: core.current_user_id.clone(),
            account_snapshot_scope: Arc::new(Mutex::new(Some(session.clone()))), ..Default::default()
        };
        context.store.borrow_mut().private_persistence = Some(PrivatePersistence::new(core.writer.clone(), lease.clone(), core.activity.clone(), core.backend.api.upgrade_latch().clone()));
        context.billing_context.bind_authenticated_session(session.clone()).unwrap();
        let payer_a = "22222222-2222-4222-8222-222222222222";
        let payer_b = "33333333-3333-4333-8333-333333333333";
        select_finance_fixture_group(&context, &session, payer_a, true);
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        app.global::<AppState>().set_credit_balance("42".into());
        let first = credit_callbacks::prepare_credit_redemption(&app, &context, "ORIGINAL-CODE").unwrap();
        let original = core.writer.load_client_state_for_namespace(&lease).unwrap().unwrap().pending_credit_redemptions_by_owner.get(&session.owner_user_id).unwrap().clone();
        let expected = serde_json::to_value(&original).unwrap();
        let expected_key = original.client_request_id.clone();
        let transport = JoinedFixtureTransport(Some(std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            for phase in 0..3 {
                let deadline = Instant::now() + Duration::from_secs(8);
                let mut stream = loop {
                    match listener.accept() { Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                        Err(error) => panic!("controlled redemption request missing: {error}") }
                };
                let request = read_request(&mut stream);
                assert!(request.starts_with("POST /v1/credits/redemptions "));
                let header = |name: &str| request.lines().find_map(|line| line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.trim().to_owned()));
                assert_eq!(header("x-account-group-id").as_deref(), Some(payer_a));
                assert_eq!(header("idempotency-key").as_deref(), Some(expected_key.as_str()));
                assert_eq!(serde_json::from_str::<serde_json::Value>(request.split("\r\n\r\n").nth(1).unwrap()).unwrap(), serde_json::json!({"code":"ORIGINAL-CODE","client_request_id":expected_key}));
                let (status, data, error) = match phase {
                    0 => ("503 Service Unavailable", serde_json::Value::Null, serde_json::json!({"code":"service_unavailable","message":"ambiguous","details":null})),
                    1 => ("403 Forbidden", serde_json::Value::Null, serde_json::json!({"code":"account_group_not_selectable","message":"original admission denied","details":null})),
                    _ => ("200 OK", serde_json::json!({"redemption_id":"fixture-redemption","credits_granted":"999","redeemed_at":"2026-09-07T00:00:00Z","credit_expires_at":null,"account":{"available":"999","reserved":"0","lifetime_granted":"999","lifetime_spent":"0","version":"99"}}), serde_json::Value::Null),
                };
                let body = serde_json::json!({"request_id":"controlled-redemption","data":data,"error":error,"meta":null}).to_string();
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        })));
        let (receipt, outcome) = first.run(&core.backend);
        credit_callbacks::complete_credit_redemption(&app, &context, &receipt, outcome);
        select_finance_fixture_group(&context, &session, payer_b, false);
        for success in [false, true] {
            let retry = credit_callbacks::prepare_credit_redemption(&app, &context, "ORIGINAL-CODE").unwrap();
            let (receipt, outcome) = retry.run(&core.backend);
            credit_callbacks::complete_credit_redemption(&app, &context, &receipt, outcome);
            let saved = core.writer.load_client_state_for_namespace(&lease).unwrap().unwrap();
            if success { assert!(saved.pending_credit_redemptions_by_owner.is_empty()); }
            else { assert_eq!(serde_json::to_value(saved.pending_credit_redemptions_by_owner.get(&session.owner_user_id).unwrap()).unwrap(), expected); }
            assert_eq!(app.global::<AppState>().get_credit_balance().as_str(), "42");
            assert_eq!(context.billing_context.confirmed_scope().unwrap().request.account_group_id, payer_b);
        }
        transport.finish();
    }
    struct JoinedFixtureTransport(Option<std::thread::JoinHandle<()>>);
    #[test]
    fn core_actual_redemption_success_with_failed_sqlite_ack_retains_identical_intent() {
        assert_redemption_completion_preserves_intent(false);
    }
    #[test]
    fn core_actual_redemption_exact_426_keeps_sqlite_intent_and_private_balance() {
        assert_redemption_completion_preserves_intent(true);
    }
    fn assert_redemption_completion_preserves_intent(upgrade: bool) {
        use std::io::Write;
        use backend_generation::billing_capture_test_support::{listener, read_request};
        let (listener, url) = listener();
        let (_fixture, core, session, lease) = active_transition_fixture_at(false, &url);
        let context = AppContext {
            data_root_capability: Some(core.root.clone()), file_index: Some(FileIndex::initialize(core.root_path.join("finance-index.sqlite3")).unwrap()),
            backend: Some(core.backend.clone()), active_namespace: core.active_namespace.clone(), namespace_operations: core.namespace_operations.clone(),
            user_activity: core.activity.clone(), billing_context: core.billing.clone(), current_user_id: core.current_user_id.clone(),
            account_snapshot_scope: Arc::new(Mutex::new(Some(session.clone()))), ..Default::default()
        };
        context.store.borrow_mut().private_persistence = Some(PrivatePersistence::new(core.writer.clone(), lease.clone(), core.activity.clone(), core.backend.api.upgrade_latch().clone()));
        context.billing_context.bind_authenticated_session(session.clone()).unwrap();
        select_finance_fixture_group(&context, &session, "22222222-2222-4222-8222-222222222222", true);
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        app.global::<AppState>().set_credit_balance("42".into());
        let prepared = credit_callbacks::prepare_credit_redemption(&app, &context, "RETAIN-ME").unwrap();
        let original = context.store.borrow().pending_credit_redemptions_by_owner.get(&session.owner_user_id).unwrap().clone();
        let expected = serde_json::to_value(&original).unwrap();
        if !upgrade {
            let connection = rusqlite::Connection::open(core.root_path.join("fixture.sqlite3")).unwrap();
            connection.execute_batch("CREATE TRIGGER refuse_redemption_settlement BEFORE UPDATE ON user_settings BEGIN SELECT RAISE(ABORT, 'fixture local acknowledgement failed'); END;").unwrap();
        }
        let transport = JoinedFixtureTransport(Some(std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut stream = loop {
                match listener.accept() { Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                    Err(error) => panic!("redemption transport missing: {error}") }
            };
            let request = read_request(&mut stream);
            assert!(request.starts_with("POST /v1/credits/redemptions "));
            assert_eq!(serde_json::from_str::<serde_json::Value>(request.split("\r\n\r\n").nth(1).unwrap()).unwrap(), serde_json::json!({"code":"RETAIN-ME","client_request_id":original.client_request_id}));
            let (status, data, error) = if upgrade {
                ("426 Upgrade Required", serde_json::Value::Null, serde_json::json!({"code":"client_upgrade_required","message":"untrusted","details":{"minimum_version":"99.0.0"}}))
            } else {
                ("200 OK", serde_json::json!({"redemption_id":"fixture-redemption","credits_granted":"999","redeemed_at":"2026-09-07T00:00:00Z","credit_expires_at":null,"account":{"available":"999","reserved":"0","lifetime_granted":"999","lifetime_spent":"0","version":"99"}}), serde_json::Value::Null)
            };
            let body = serde_json::json!({"request_id":"controlled","data":data,"error":error,"meta":null}).to_string();
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        })));
        let (receipt, outcome) = prepared.run(&core.backend);
        transport.finish();
        credit_callbacks::complete_credit_redemption(&app, &context, &receipt, outcome);
        let retained = core.writer.read_retained_redemption_checked(&lease, &expected["client_request_id"].as_str().unwrap()).unwrap().unwrap();
        assert_eq!(serde_json::to_value(retained).unwrap(), expected);
        assert_eq!(serde_json::to_value(context.store.borrow().pending_credit_redemptions_by_owner.get(&session.owner_user_id).unwrap()).unwrap(), expected);
        assert_eq!(app.global::<AppState>().get_credit_balance().as_str(), "42");
        assert!(!app.global::<AppState>().get_credit_redemption_success());
        assert_eq!(core.backend.api.upgrade_latch().is_tripped(), upgrade);
    }
    impl JoinedFixtureTransport {
        fn finish(mut self) { self.0.take().unwrap().join().unwrap(); }
    }
    impl Drop for JoinedFixtureTransport {
        fn drop(&mut self) {
            if let Some(worker) = self.0.take() { let _ = worker.join(); }
        }
    }



    #[test]
    fn core_cleanup_observed_foreign_record_denies_repeated_clear_and_direct_install() {
        let storage = Arc::new(DeleteFaultStore { inner: api::test_support::MemoryRefreshTokenStore::default(), fail: false.into() });
        let (_fixture, core, _, _) = active_transition_fixture_with_store(storage.clone(), "http://127.0.0.1:9");
        let replacement = PersistedRefreshSession { owner_user_id: "66666666-6666-4666-8666-666666666666".into(), refresh_token: "unknown-replacement".into() };
        storage.save(&replacement).unwrap();
        assert!(core.backend.api.session().clear().is_err());
        assert!(core.backend.api.session().clear().is_err(), "observed foreign record is not an empty store");
        let ActivationInput::Login { user, tokens, .. } = cleanup_login_input() else { unreachable!() };
        assert!(core.backend.api.session().install_tokens_for_user(&tokens, &user.id).is_err());
        let retained = storage.load().unwrap().unwrap();
        assert_eq!(retained.owner_user_id, replacement.owner_user_id);
        assert_eq!(retained.refresh_token, replacement.refresh_token);
        assert!(core.backend.api.session().access().is_none());
    }
    #[test]
    fn core_ordered_prepared_save_survives_completion_refusal_until_released_outside_latch() {
        let (_fixture, core, _, lease) = active_transition_fixture(false);
        let persistence = PrivatePersistence::new(core.writer.clone(), lease.clone(), core.activity.clone(), core.backend.api.upgrade_latch().clone());
        let mut prepared = Some(persistence.prepare_ordered_save().unwrap());
        let latch = core.backend.api.upgrade_latch().clone();
        let transfer = latch.begin_ordinary_transfer().unwrap();
        let tripping = latch.clone();
        let worker = std::thread::spawn(move || tripping.trip_from_ordinary_transfer(transfer, RequiredUpgrade { minimum_version: None }, || ()));
        // trip publishes its closed state before waiting for the held durable admission.
        while latch.snapshot().is_none() { std::thread::yield_now(); }
        let result = latch.apply_if_open(|| prepared.take().unwrap().enqueue(LocalStoreData::default()));
        assert!(result.is_err());
        assert!(prepared.is_some(), "a refused completion must retain its prepared guards outside the mutex");
        drop(prepared);
        worker.join().unwrap();
        assert!(core.writer.load_client_state_for_namespace(&lease).unwrap().is_none());
    }


    struct StartupReadFaultStore {
        inner: api::test_support::MemoryRefreshTokenStore,
        fail: std::sync::atomic::AtomicBool,
        loads: std::sync::atomic::AtomicUsize,
        writes: std::sync::atomic::AtomicUsize,
    }
    impl RefreshTokenStore for StartupReadFaultStore {
        fn load(&self) -> std::result::Result<Option<PersistedRefreshSession>, ApiError> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) { Err(ApiError::LocalState { message: "fixture unreadable retained credentials".into() }) }
            else { self.inner.load() }
        }
        fn save(&self, record: &PersistedRefreshSession) -> std::result::Result<(), ApiError> {
            self.writes.fetch_add(1, Ordering::SeqCst); self.inner.save(record)
        }
        fn clear(&self) -> std::result::Result<(), ApiError> {
            self.writes.fetch_add(1, Ordering::SeqCst); self.inner.clear()
        }
        fn clear_if_current(&self, expected: &PersistedRefreshSession) -> std::result::Result<StoreMutation, ApiError> {
            self.writes.fetch_add(1, Ordering::SeqCst); self.inner.clear_if_current(expected)
        }
        fn replace_if_current(&self, expected: &PersistedRefreshSession, replacement: &PersistedRefreshSession) -> std::result::Result<StoreMutation, ApiError> {
            self.writes.fetch_add(1, Ordering::SeqCst); self.inner.replace_if_current(expected, replacement)
        }
    }
    #[test]
    fn core_cleanup_startup_read_failure_never_becomes_an_empty_slot_or_resume_authority() {
        let storage = Arc::new(StartupReadFaultStore { inner: api::test_support::MemoryRefreshTokenStore::default(),
            fail: true.into(), loads: 0.into(), writes: 0.into() });
        storage.inner.save(&PersistedRefreshSession { owner_user_id: CLEANUP_B.into(), refresh_token: "unreadable-existing-record".into() }).unwrap();
        let session = SessionManager::new(storage.clone());
        // A later-readable file does not retroactively authorize this process to adopt it.
        storage.fail.store(false, Ordering::SeqCst);
        let ActivationInput::Login { user, tokens, .. } = cleanup_login_input() else { unreachable!() };
        for _ in 0..2 {
            assert!(session.clear().is_err(), "unknown startup state must not acknowledge empty cleanup");
            assert!(session.install_tokens_for_user(&tokens, &user.id).is_err());
            assert!(session.refresh_persisted_owner(CLEANUP_B, |_| panic!("unknown credentials must not reach remote refresh")).is_err());
        }
        assert_eq!(storage.loads.load(Ordering::SeqCst), 1, "no automatic reload/adoption");
        assert_eq!(storage.writes.load(Ordering::SeqCst), 0);
        assert!(session.access().is_none());
        assert!(resumable_persisted_owner(&session, &Mutex::new(None)).is_none());
        assert_eq!(storage.inner.load().unwrap().unwrap().refresh_token, "unreadable-existing-record");
        let empty = SessionManager::new(Arc::new(api::test_support::MemoryRefreshTokenStore::default()));
        empty.clear().unwrap();
        assert_eq!(empty.install_tokens_for_user(&tokens, &user.id).unwrap().owner_user_id, CLEANUP_B);
    }
    #[test]
    fn core_cleanup_already_invalidated_scope_is_not_a_disk_cleanup_receipt() {
        let storage = Arc::new(DeleteFaultStore { inner: api::test_support::MemoryRefreshTokenStore::default(), fail: true.into() });
        let session = Arc::new(SessionManager::new(storage.clone()));
        let ActivationInput::Login { user, tokens, .. } = cleanup_login_input() else { unreachable!() };
        let scope = session.install_tokens_for_user(&tokens, &user.id).unwrap();
        assert!(session.clear_scope(&scope).is_err());
        let debt = Arc::new(Mutex::new(None));
        drop(RetiredSessionCleanup::new(session.clone(), scope, debt.clone()));
        assert!(debt.lock().unwrap().as_ref().is_some_and(|debt| debt.credentials));
        assert!(resumable_persisted_owner(&session, &debt).is_none());
        assert_eq!(storage.load().unwrap().unwrap().owner_user_id, CLEANUP_B);
    }
    #[test]
    fn core_cleanup_pending_activation_drop_keeps_unacknowledged_disk_deletion() {
        let (_fixture, core, scope, _) = active_transition_fixture(true);
        assert!(core.backend.api.session().clear_scope(&scope).is_err());
        drop(PendingActivation { core: core.clone(), scope, writer_lease: None, committed: false, changes_user: true });
        assert!(core.cleanup_failure.lock().unwrap().as_ref().is_some_and(|debt| debt.credentials));
        assert!(resumable_persisted_owner(core.backend.api.session(), &core.cleanup_failure).is_none());
        assert!(core.backend.api.session().persisted_owner_user_id().is_some());
    }
    const CLEANUP_B: &str = "33333333-3333-4333-8333-333333333333";
    const CLEANUP_GROUP: &str = "44444444-4444-4444-8444-444444444444";
    fn cleanup_login_input() -> ActivationInput {
        ActivationInput::Login { user: LoginUser { id: CLEANUP_B.into(), email_masked: "b***@example.com".into(), nickname: Some("B".into()), status: "active".into() },
            tokens: TokenSet { access_token: "new-B-access".into(), access_expires_in_seconds: 1800, refresh_token: "new-B-refresh".into(),
                refresh_expires_at: "2099-01-01T00:00:00Z".into(), token_type: "X-Token".into() },
            origin: LoginOrigin::Password, suggested_group: Some(CLEANUP_GROUP.into()) }
    }
    fn cleanup_login_transport(listener: std::net::TcpListener) -> JoinedFixtureTransport {
        use backend_generation::billing_capture_test_support::read_request;
        use std::io::Write;
        JoinedFixtureTransport(Some(std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let choice = serde_json::json!({"group_id":CLEANUP_GROUP,"name":"B team","group_status":"active","role":"member",
                "member_id":"55555555-5555-4555-8555-555555555555","relationship_status":"active","readable_context":true,"selectable":true,
                "group_version":"1","membership_version":"1","capabilities":["bill"],"quota":null});
            let mut seen = BTreeSet::new();
            for _ in 0..5 {
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut stream = loop { match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                    Err(error) => panic!("activation transport: {error}"),
                }};
                let request = read_request(&mut stream);
                let path = request.lines().next().unwrap().split_whitespace().nth(1).unwrap().to_owned();
                assert!(seen.insert(path.clone()), "unexpected repeated activation request");
                assert!(request.lines().any(|line| line.eq_ignore_ascii_case("x-token: new-B-access")));
                let selected = request.lines().find_map(|line| line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case("x-account-group-id")).map(|(_, value)| value.trim()));
                assert_eq!(selected, matches!(path.as_str(), "/v1/account" | "/v1/models").then_some(CLEANUP_GROUP));
                let (status, data, error) = match path.as_str() {
                    "/v1/account-groups" => ("200 OK", serde_json::json!({"items":[choice],"pending_invitation_count":0}), serde_json::Value::Null),
                    "/v1/account" => ("200 OK", serde_json::json!({"user":{"id":CLEANUP_B,"email_masked":"b***@example.com","nickname":"B","status":"active","registered_at":"2026-09-07T00:00:00Z"},
                        "read_only":false,"capabilities":["bill"],"billing_group":choice,"membership":null,"credits":null,"quota":null,"entitlement":{}}), serde_json::Value::Null),
                    "/v1/account/sessions" | "/v1/models" => ("200 OK", serde_json::json!({"items":[]}), serde_json::Value::Null),
                    "/v1/account/invitation" => ("503 Service Unavailable", serde_json::Value::Null, serde_json::json!({"code":"service_unavailable","message":"optional invitation unavailable","details":null})),
                    _ => panic!("unexpected activation route {path}"),
                };
                let body = serde_json::json!({"request_id":"activation-cleanup","data":data,"error":error,"meta":null}).to_string();
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            assert_eq!(seen.len(), 5);
        })))
    }
    #[test]
    fn core_cleanup_explicit_login_retries_exact_delete_before_preparing_and_persisting_b() {
        use backend_generation::billing_capture_test_support::listener;
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let (listener, url) = listener();
        let storage = Arc::new(DeleteFaultStore { inner: api::test_support::MemoryRefreshTokenStore::default(), fail: true.into() });
        let (_fixture, core, scope, _) = active_transition_fixture_with_store(storage.clone(), &url);
        let context = AppContext { file_index: Some(FileIndex::initialize(core.root_path.join("cleanup-index.sqlite3")).unwrap()), ..Default::default() };
        let ticket = core.admission.begin().unwrap();
        assert!(core.prepare_activation(ActivationInput::Logout { scope, token: None, all: false }, &ticket,
            || { clear_retired_private_state(&app, &context); Ok(()) }).is_err());
        drop(ticket);
        assert!(resumable_persisted_owner(core.backend.api.session(), &core.cleanup_failure).is_none());
        storage.fail.store(false, Ordering::SeqCst);
        let transport = cleanup_login_transport(listener);
        let ticket = core.admission.begin().unwrap();
        app.global::<AppState>().set_colorize_result_path("retained A projection".into());
        let mut prepared = core.prepare_activation(cleanup_login_input(), &ticket,
            || { clear_retired_private_state(&app, &context); Ok(()) }).expect("explicit login must retry exact cleanup").unwrap();
        assert!(app.global::<AppState>().get_colorize_result_path().is_empty());
        assert_eq!(prepared.pending.scope.owner_user_id, CLEANUP_B);
        assert_eq!(prepared.lease.namespace.user_public_id(), CLEANUP_B);
        let _models = PreparedPrivateModels::new(&app, &mut prepared, &context).unwrap();
        let persisted = prepared.persist().unwrap();
        assert_eq!(core.writer.load_selected_group(CLEANUP_B, &core.backend.api.device().id).unwrap().as_deref(), Some(CLEANUP_GROUP));
        assert_eq!(storage.load().unwrap().unwrap().owner_user_id, CLEANUP_B);
        assert!(core.cleanup_failure.lock().unwrap().is_none());
        drop(persisted); // Candidate cleanup occurs before fixture roots are dropped.
        transport.finish();
    }
    #[test]
    fn core_cleanup_failed_or_replaced_record_never_allows_repeated_explicit_login() {
        use backend_generation::billing_capture_test_support::listener;
        for replace in [false, true] {
            let (listener, url) = listener();
            let storage = Arc::new(DeleteFaultStore { inner: api::test_support::MemoryRefreshTokenStore::default(), fail: true.into() });
            let (_fixture, core, scope, _) = active_transition_fixture_with_store(storage.clone(), &url);
            let ticket = core.admission.begin().unwrap();
            assert!(core.prepare_activation(ActivationInput::Logout { scope, token: None, all: false }, &ticket, || Ok(())).is_err());
            drop(ticket);
            if replace {
                storage.fail.store(false, Ordering::SeqCst);
                storage.save(&PersistedRefreshSession { owner_user_id: "66666666-6666-4666-8666-666666666666".into(), refresh_token: "replacement-not-authorized".into() }).unwrap();
            }
            let before = storage.load().unwrap().unwrap();
            for _ in 0..2 {
                let ticket = core.admission.begin().unwrap();
                assert!(core.prepare_activation(cleanup_login_input(), &ticket, || Ok(())).is_err());
                assert!(core.backend.api.session().access().is_none());
                assert!(core.active_namespace.lock().unwrap().is_none());
                let retained = storage.load().unwrap().unwrap();
                assert_eq!(retained.owner_user_id, before.owner_user_id);
                assert_eq!(retained.refresh_token, before.refresh_token);
                assert!(resumable_persisted_owner(core.backend.api.session(), &core.cleanup_failure).is_none());
            }
            listener.set_nonblocking(true).unwrap();
            assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        }
    }
    #[test]
    fn core_cleanup_remote_warning_allows_only_explicit_login_without_retrying_logout() {
        use backend_generation::billing_capture_test_support::listener;
        let (remote_listener, remote_url) = listener();
        let (_fixture, core, scope, _) = active_transition_fixture_at(false, &remote_url);
        let remote = JoinedFixtureTransport(Some(std::thread::spawn(move || {
            use std::io::Write;
            remote_listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut stream = loop { match remote_listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => panic!("remote logout: {error}"),
            }};
            let request = backend_generation::billing_capture_test_support::read_request(&mut stream);
            assert!(request.starts_with("POST /v1/auth/logout "));
            let body = r#"{"request_id":"logout","data":null,"error":{"code":"service_unavailable","message":"remote unavailable","details":null},"meta":null}"#;
            write!(stream, "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            drop(stream);
            // The same retained listener subsequently handles only explicit B account reads.
            cleanup_login_transport(remote_listener).finish();
        })));
        let ticket = core.admission.begin().unwrap();
        assert!(core.prepare_activation(ActivationInput::Logout { scope, token: Some("fixed-A".into()), all: false }, &ticket, || Ok(())).is_err());
        drop(ticket);
        assert!(core.backend.api.session().persisted_owner_user_id().is_none());
        let ticket = core.admission.begin().unwrap();
        assert!(core.prepare_activation(ActivationInput::Resume { owner: CLEANUP_B.into() }, &ticket, || panic!("automatic resume must stay closed")).is_err());
        drop(ticket);
        let ticket = core.admission.begin().unwrap();
        let prepared = core.prepare_activation(cleanup_login_input(), &ticket, || Ok(())).expect("remote warning is not local cleanup debt").unwrap();
        assert_eq!(prepared.pending.scope.owner_user_id, CLEANUP_B);
        assert!(core.cleanup_failure.lock().unwrap().is_some(), "remote warning must remain disclosed");
        drop(prepared);
        remote.finish();
    }
    #[test]
    fn core_cleanup_candidate_writer_failure_is_not_erased_by_successful_credential_clear() {
        let (_fixture, core, scope, lease) = active_transition_fixture(false);
        let mut wrong = lease.clone(); wrong.namespace_epoch += 1;
        drop(PendingActivation { core: core.clone(), scope, writer_lease: Some(wrong), committed: false, changes_user: true });
        assert!(core.backend.api.session().clear().is_ok());
        let ticket = core.admission.begin().unwrap();
        assert!(core.prepare_activation(cleanup_login_input(), &ticket, || panic!("writer debt must block UI transition")).is_err());
        assert!(core.cleanup_failure.lock().unwrap().is_some());
        assert!(core.backend.api.session().access().is_none());
    }

    #[test]
    fn core_actual_logout_attempt_survives_local_cleanup_and_ui_ack_failures() {
        use backend_generation::billing_capture_test_support::{listener, read_request};
        use std::io::Write;
        i_slint_backend_testing::init_no_event_loop();
        for (fail_delete, fail_ack, fail_remote) in [(true, false, false), (true, true, true), (false, true, false)] {
            let (listener, url) = listener();
            let (_fixture, core, scope, _) = active_transition_fixture_at(fail_delete, &url);
            let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let captured = seen.clone();
            let transport = JoinedFixtureTransport(Some(std::thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                        Err(error) => panic!("fixture accept: {error}"),
                    }
                };
                let request = read_request(&mut stream);
                assert!(request.starts_with("POST /v1/auth/logout_all "));
                assert!(request.lines().any(|line| line.eq_ignore_ascii_case("x-token: captured-retired-token")));
                assert!(!request.to_ascii_lowercase().contains("x-account-group-id:"));
                captured.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let (status, data, error) = if fail_remote {
                    ("503 Service Unavailable", serde_json::Value::Null, serde_json::json!({"code":"service_unavailable","message":"remote unavailable","details":null}))
                } else { ("200 OK", serde_json::json!({"logged_out_all":true}), serde_json::Value::Null) };
                let body = serde_json::json!({"request_id":"logout-fixture","data":data,"error":error,"meta":null}).to_string();
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            })));
            let ticket = core.admission.begin().unwrap();
            let error = core.prepare_activation(ActivationInput::Logout { scope, token: Some("captured-retired-token".into()), all: true }, &ticket,
                || if fail_ack { Err(transition_error("fixture retired UI acknowledgement cancelled")) } else { Ok(()) }).err().expect("logout must not claim success");
            transport.finish();
            assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1, "local failure skipped captured remote logout");
            assert!(core.backend.api.session().access().is_none());
            assert!(core.active_namespace.lock().unwrap().is_none());
            assert!(resumable_persisted_owner(core.backend.api.session(), &core.cleanup_failure).is_none());
            // ApiError intentionally sanitizes arbitrary LocalState detail. Assert the
            // actual coordinator projection, which appends its owned cleanup disclosure.
            let app = AppWindow::new().unwrap();
            let mailbox = Arc::new(ActivationMailbox::new());
            mailbox.put(ActivationMailboxPhase::Failed(error));
            let coordinator = AccountTransitionCoordinator { core: core.clone(), pending: RefCell::new(Some(ActivationUi { mailbox, models: None, worker: None, login_origin: None })),
                retired_workers:RefCell::new(Vec::new()),worker_failed:Arc::new(false.into()) };
            assert!(!coordinator.advance(&app, &AppContext::default()));
            let message = app.global::<AppState>().get_auth_error();
            if fail_delete {
                assert!(message.contains("重启可能恢复保留的会话"));
                assert!(!message.contains("重启后重试"));
            }
            if fail_remote { assert!(message.contains("远程退出未确认")); }
        }
    }

    #[test]
    fn core_actual_logout_does_not_contact_remote_before_successful_retirement() {
        use backend_generation::billing_capture_test_support::listener;
        let (listener, url) = listener();
        let (_fixture, core, scope, lease) = active_transition_fixture_at(false, &url);
        let connection = rusqlite::Connection::open(core.root_path.join("fixture.sqlite3")).unwrap();
        connection.execute_batch("CREATE TRIGGER refuse_logout_fixture BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT, 'fixture undurable state'); END;").unwrap();
        assert!(core.writer.persist_client_state_checked_for_namespace(&lease, LocalStoreData::default()).is_err());
        let ticket = core.admission.begin().unwrap();
        assert!(core.prepare_activation(ActivationInput::Logout { scope: scope.clone(), token: Some("must-not-send".into()), all: false }, &ticket,
            || panic!("failed flush must not retire UI")).is_err());
        listener.set_nonblocking(true).unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        assert!(core.backend.api.session().is_scope_current(&scope));
        assert_eq!(core.active_namespace.lock().unwrap().as_ref(), Some(&lease));
        assert!(core.cleanup_failure.lock().unwrap().is_none());
    }
    fn active_transition_fixture(fail_delete: bool) -> (client_state::tests::Fixture, Arc<TransitionCore>, SessionScope, NamespaceLease) {
        active_transition_fixture_at(fail_delete, "http://127.0.0.1:9")
    }
    fn active_transition_fixture_at(fail_delete: bool, url: &str) -> (client_state::tests::Fixture, Arc<TransitionCore>, SessionScope, NamespaceLease) {
        active_transition_fixture_with_store(Arc::new(DeleteFaultStore { inner: api::test_support::MemoryRefreshTokenStore::default(), fail: fail_delete.into() }), url)
    }
    fn active_transition_fixture_with_store(storage: Arc<DeleteFaultStore>, url: &str) -> (client_state::tests::Fixture, Arc<TransitionCore>, SessionScope, NamespaceLease) {
        let fixture = client_state::tests::Fixture::new(false, false);
        let session = Arc::new(SessionManager::new(storage));
        let scope = session.install_tokens_for_user(&TokenSet {
            access_token: "fixture-access".into(), access_expires_in_seconds: 1800,
            refresh_token: "fixture-refresh".into(), refresh_expires_at: "2099-01-01T00:00:00Z".into(), token_type: "X-Token".into(),
        }, "11111111-1111-4111-8111-111111111111").unwrap();
        let lease = fixture.lease(&scope.owner_user_id, scope.auth_epoch, 1);
        let root = fixture.data_root_capability_arc();
        let root_path = lease.namespace.root().parent().unwrap().parent().unwrap().to_path_buf();
        let namespace_operations = NamespaceOperationGate::default();
        let transition = namespace_operations.begin_transition().unwrap();
        let authority = NamespaceStorageAuthority::open_prepublication(root.clone(), &lease).unwrap();
        let recovery = transition.begin_prepublication_recovery(&lease).unwrap();
        recovery.verify_no_unsupported_imports(&authority).unwrap();
        let recovered = recovery.finish().unwrap();
        transition.prepare_publication(&lease, recovered).unwrap().publish();
        fixture.activate(lease.clone()).unwrap();
        let activity = UserActivityGate::default();
        activity.prepare_activation(lease.clone()).unwrap().publish();
        let active_namespace = Arc::new(Mutex::new(Some(lease.clone())));
        let client = ApiClient::new(ApiClientConfig {
            base_url: reqwest::Url::parse(url).unwrap(), app_version: "fixture".into(), timeout: Duration::from_secs(1),
        }, DeviceIdentity { id: "22222222-2222-4222-8222-222222222222".into(), name: "fixture".into(), platform: "macos".into() }, session).unwrap();
        client.bind_user_work(UserWorkAdmission::new(active_namespace.clone(), activity.clone())).unwrap();
        let core = Arc::new(TransitionCore {
            backend: Arc::new(BackendRuntime { api: client }), writer: (*fixture).clone(), root, root_path,
            admission: AccountTransitionAdmission::default(), namespace_operations, activity,
            billing: Arc::new(BillingContextManager::default()), active_namespace,
            current_user_id: Arc::new(Mutex::new(Some(scope.owner_user_id.clone()))), cleanup_failure: Arc::new(Mutex::new(None)),
        });
        (fixture, core, scope, lease)
    }
    #[test]
    fn core_actual_coordinator_retired_ack_cancellation_clears_exact_credentials_and_records_debt() {
        for fail in [false, true] {
            let (_fixture, core, scope, lease) = active_transition_fixture(fail);
            let ticket = core.admission.begin().unwrap();
            let result = core.prepare_activation(ActivationInput::Logout { scope: scope.clone(), token: None, all: false }, &ticket,
                || Err(transition_error("fixture shutdown during retired acknowledgement")));
            assert!(result.is_err());
            assert!(core.active_namespace.lock().unwrap().is_none());
            assert!(core.namespace_operations.active_lease().is_none());
            assert!(core.activity.begin_recovery_unit(&lease).is_err());
            assert!(core.backend.api.session().access().is_none());
            let debt = core.cleanup_failure.lock().unwrap().clone().expect("unacknowledged private UI remains closed");
            assert_eq!(debt.credentials, fail);
            assert!(debt.private_ui);
            assert_eq!(core.backend.api.session().persisted_owner_user_id().is_some(), fail);
            assert!(resumable_persisted_owner(core.backend.api.session(), &core.cleanup_failure).is_none());
            assert!(core.writer.persist_client_state_checked_for_namespace(&lease, LocalStoreData::default()).is_err());
        }
    }
    #[test]
    fn core_actual_logout_clears_private_slint_projection_after_real_writer_retirement() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let (_fixture, core, scope, lease) = active_transition_fixture(false);
        let context = AppContext::default();
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_compression_images(ModelRc::new(VecModel::from(vec![CompressionImageItem { id: "A".into(), name: "private A".into(), source_path: "/fixture/A.png".into(), result_path: "/fixture/A-result.png".into(), ..Default::default() }])));
        state.set_colorize_result_path("/fixture/A-result.png".into());
        let ticket = core.admission.begin().unwrap();
        assert!(core.prepare_activation(ActivationInput::Logout { scope, token: None, all: false }, &ticket,
            || { clear_retired_private_state(&app, &context); Ok(()) }).unwrap().is_none());
        assert!(!state.get_logged_in());
        assert_eq!(state.get_compression_images().row_count(), 0);
        assert!(state.get_colorize_result_path().is_empty());
        assert!(core.backend.api.session().persisted_owner_user_id().is_none());
        assert!(core.writer.persist_client_state_checked_for_namespace(&lease, LocalStoreData::default()).is_err());
    }

    #[test]
    fn core_reference_import_commits_only_captured_namespace_and_rejects_retired_completion() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let external = tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let source = external.path().join("source.png");
        image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 20, 30, 255])).save(&source).unwrap();
        let (_fixture, core, scope, lease) = active_transition_fixture(false);
        let index = FileIndex::initialize(core.root_path.join("reference-index.sqlite3")).unwrap();
        let persistence = PrivatePersistence::new(core.writer.clone(), lease.clone(), core.activity.clone(), core.backend.api.upgrade_latch().clone())
            .with_storage(core.root.clone(), core.backend.api.clone(), index);
        let mut store = Store::default(); store.private_persistence = Some(persistence.clone());
        let store = Rc::new(RefCell::new(store));
        assert!(add_reference_from_captured_path(&app, &store, &source, &persistence, "text2img", false));
        let owned = references_for_context(&store.borrow(), "text2img", false).first().unwrap().source_path.clone();
        assert!(Path::new(&owned).starts_with(lease.namespace.path(ManagedUserArea::ReferencesLibrary)));
        let loaded = core.writer.load_client_state_for_namespace(&lease).unwrap().unwrap();
        assert!(serde_json::to_string(&loaded).unwrap().contains(&owned));
        let restored = prepare_private_store(loaded);
        assert_eq!(references_for_context(&restored.store, "text2img", false)[0].source_path, owned);
        let other = NamespaceLease { namespace: UserNamespace::new(&core.root_path, "33333333-3333-4333-8333-333333333333").unwrap(), auth_epoch: lease.auth_epoch + 1, namespace_epoch: lease.namespace_epoch + 1 };
        assert!(core.writer.load_client_state_for_namespace(&other).unwrap().is_none());
        let ticket = core.admission.begin().unwrap();
        core.prepare_activation(ActivationInput::Logout { scope, token: None, all: false }, &ticket, || Ok(())).unwrap();
        *store.borrow_mut() = Store::default();
        let before = references_for_context(&store.borrow(), "text2img", false).len();
        assert!(!add_reference_from_captured_path(&app, &store, &source, &persistence, "text2img", false));
        assert_eq!(references_for_context(&store.borrow(), "text2img", false).len(), before);
        assert!(source.is_file() && Path::new(&owned).is_file(), "retirement preserves original and owned bytes");
    }
    #[test]
    fn core_ordered_save_prepare_denies_exact_upgrade_without_writer_mutation() {
        let (_fixture, core, _, lease) = active_transition_fixture(false);
        let persistence = PrivatePersistence::new(core.writer.clone(), lease.clone(), core.activity.clone(), core.backend.api.upgrade_latch().clone());
        let transfer = core.backend.api.upgrade_latch().begin_ordinary_transfer().unwrap();
        core.backend.api.upgrade_latch().trip_from_ordinary_transfer(transfer, RequiredUpgrade { minimum_version: None }, || ());
        assert!(persistence.prepare_ordered_save().is_err());
        assert!(core.writer.load_client_state_for_namespace(&lease).unwrap().is_none());
    }
    #[test]
    fn core_reference_settings_missing_legacy_key_loads_empty_without_owner_reassignment() {
        let (_fixture, core, _, lease) = active_transition_fixture(false);
        core.writer.persist_client_state_checked_for_namespace(&lease, LocalStoreData::default()).unwrap();
        let connection = rusqlite::Connection::open(core.root_path.join("fixture.sqlite3")).unwrap();
        connection.execute("DELETE FROM user_settings WHERE user_public_id = ?1 AND key = 'references'", [lease.namespace.user_public_id()]).unwrap();
        let restored = prepare_private_store(core.writer.load_client_state_for_namespace(&lease).unwrap().unwrap());
        for category in ["character", "scene", "ui", "effect"] {
            assert!(references_for_context(&restored.store, category, false).is_empty());
        }
        let count: i64 = connection.query_row("SELECT COUNT(*) FROM user_settings WHERE key = 'references'", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "legacy loading must not synthesize or assign a reference row");
    }
    #[test]
    fn core_reference_upload_rejects_foreign_namespace_before_any_network() {
        use backend_generation::billing_capture_test_support::listener;
        let (listener, url) = listener();
        let (_fixture, core, scope, lease) = active_transition_fixture_at(false, &url);
        let other = UserNamespace::new(&core.root_path, "33333333-3333-4333-8333-333333333333").unwrap();
        fs::create_dir_all(other.output_dir()).unwrap();
        let path = other.output_dir().join("other.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])).save(&path).unwrap();
        let index = FileIndex::initialize(core.root_path.join("upload-index.sqlite3")).unwrap();
        let authority = NamespaceStorageAuthority::open_active(core.root.clone(), &lease, core.backend.api.clone(), index).unwrap();
        assert!(GenerationApi::new(core.backend.api.clone()).upload_reference_for_namespace(&path, &authority, &scope, false).is_err());
        listener.set_nonblocking(true).unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        assert!(path.is_file());
    }
    #[test]
    fn core_reference_upload_uses_copied_bytes_and_exact_provider_upgrade_closes_completion() {
        use backend_generation::billing_capture_test_support::{listener, read_request_bytes};
        use std::io::Write;
        for upgrade in [false, true] {
            let (listener, url) = listener();
            let (_fixture, core, scope, lease) = active_transition_fixture_at(false, &url);
            let source = lease.namespace.path(ManagedUserArea::ReferencesLibrary).join("source.png");
            image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 255])).save(&source).unwrap();
            let original = fs::read(&source).unwrap();
            let index = FileIndex::initialize(core.root_path.join("upload-index.sqlite3")).unwrap();
            let authority = NamespaceStorageAuthority::open_active(core.root.clone(), &lease, core.backend.api.clone(), index).unwrap();
            let provider = reqwest::Url::parse(&url).unwrap().join("fixture-upload").unwrap().to_string();
            let transport = JoinedFixtureTransport(Some(std::thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                for phase in 0..if upgrade { 2 } else { 3 } {
                    let deadline = Instant::now() + Duration::from_secs(4);
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                            Err(error) => panic!("upload fixture: {error}"),
                        }
                    };
                    let request = read_request_bytes(&mut stream);
                    let header_end = request.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
                    let headers = String::from_utf8_lossy(&request[..header_end]).to_lowercase();
                    assert!(!headers.contains("x-account-group-id:"));
                    let (status, data, error) = match phase {
                        0 => {
                            assert!(headers.starts_with("post /v1/uploads/references "));
                            let body: serde_json::Value = serde_json::from_slice(&request[header_end + 4..]).unwrap();
                            assert_eq!(body["mime_type"], "image/png");
                            assert!(body["size_bytes"].as_u64().unwrap() > 0);
                            ("200 OK", serde_json::json!({"file":{"id":"fixture-input"},"upload":{"method":"POST","url":provider,"fields":{},"file_field":"file"}}), serde_json::Value::Null)
                        },
                        1 if upgrade => ("426 Upgrade Required", serde_json::Value::Null,
                            serde_json::json!({"code":"client_upgrade_required","message":"untrusted","details":{"minimum_version":"99.0.0"}})),
                        1 => {
                            assert!(headers.starts_with("post /fixture-upload "));
                            assert!(!headers.contains("x-token:"));
                            ("200 OK", serde_json::Value::Null, serde_json::Value::Null)
                        },
                        _ => {
                            assert!(headers.starts_with("post /v1/uploads/references/fixture-input/complete "));
                            ("200 OK", serde_json::json!({}), serde_json::Value::Null)
                        },
                    };
                    let body = serde_json::json!({"request_id":"fixture","data":data,"error":error,"meta":null}).to_string();
                    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
                if upgrade {
                    std::thread::sleep(Duration::from_millis(40));
                    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
                }
            })));
            let result = GenerationApi::new(core.backend.api.clone()).upload_reference_for_namespace(&source, &authority, &scope, false);
            transport.finish();
            if upgrade { assert!(result.unwrap_err().is_client_update_required()); }
            else { assert_eq!(result.unwrap(), "fixture-input"); }
            assert_eq!(core.backend.api.upgrade_latch().is_tripped(), upgrade);
            assert_eq!(fs::read(&source).unwrap(), original);
        }
    }
    struct DeleteFaultStore { inner: api::test_support::MemoryRefreshTokenStore, fail: std::sync::atomic::AtomicBool }
    impl RefreshTokenStore for DeleteFaultStore {
        fn load(&self) -> std::result::Result<Option<PersistedRefreshSession>, ApiError> { self.inner.load() }
        fn save(&self, value: &PersistedRefreshSession) -> std::result::Result<(), ApiError> { self.inner.save(value) }
        fn clear(&self) -> std::result::Result<(), ApiError> { self.inner.clear() }
        fn replace_if_current(&self, expected: &PersistedRefreshSession, value: &PersistedRefreshSession) -> std::result::Result<StoreMutation, ApiError> { self.inner.replace_if_current(expected, value) }
        fn clear_if_current(&self, expected: &PersistedRefreshSession) -> std::result::Result<StoreMutation, ApiError> {
            if self.fail.load(Ordering::SeqCst) { return Err(ApiError::LocalState { message: "fixture exact-delete failure".into() }); }
            self.inner.clear_if_current(expected)
        }
    }
    #[test]
    fn core_retired_session_cleanup_runs_on_cancellation_and_records_delete_debt() {
        for fail in [false, true] {
            let storage = Arc::new(DeleteFaultStore { inner: api::test_support::MemoryRefreshTokenStore::default(), fail: fail.into() });
            let session = Arc::new(SessionManager::new(storage));
            let scope = session.install_tokens_for_user(&TokenSet {
                access_token: "fixture-access".into(), access_expires_in_seconds: 1800,
                refresh_token: "fixture-refresh".into(), refresh_expires_at: "2099-01-01T00:00:00Z".into(), token_type: "X-Token".into(),
            }, "11111111-1111-4111-8111-111111111111").unwrap();
            let debt = Arc::new(Mutex::new(None));
            { let _cancelled = RetiredSessionCleanup::new(session.clone(), scope.clone(), debt.clone()); }
            assert!(session.access().is_none());
            assert_eq!(debt.lock().unwrap().is_some(), fail);
            assert_eq!(resumable_persisted_owner(&session, &debt).is_some(), false);
            assert_eq!(session.persisted_owner_user_id().is_some(), fail);
        }
    }
    #[test]
    fn core_retirement_clears_toolbox_rows_and_private_scalar_values_not_just_previews() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let context = AppContext::default();
        let state = app.global::<AppState>();
        let rows = ModelRc::new(VecModel::from(vec![CompressionImageItem {
            id: "account-a-row".into(), name: "A private image".into(),
            source_path: "/fixture/account-a/source.png".into(), result_path: "/fixture/account-a/result.png".into(),
            ..CompressionImageItem::default()
        }]));
        state.set_compression_images(rows.clone()); state.set_conversion_images(rows.clone());
        state.set_watermark_source_path("/fixture/account-a/private.png".into());
        state.set_colorize_result_path("/fixture/account-a/result.png".into());
        state.set_custom_prompt_input("A private prompt".into());
        clear_retired_private_projection(&state);
        assert_eq!(state.get_compression_images().row_count(), 0);
        state.set_compression_images(rows.clone()); state.set_conversion_images(rows);
        state.set_watermark_source_path("/fixture/account-a/private.png".into());
        state.set_colorize_result_path("/fixture/account-a/result.png".into());
        state.set_custom_prompt_input("A private prompt".into());
        clear_retired_private_state(&app, &context);
        assert_eq!(state.get_compression_images().row_count(), 0);
        assert_eq!(state.get_conversion_images().row_count(), 0);
        assert!(state.get_watermark_source_path().is_empty());
        assert!(state.get_colorize_result_path().is_empty());
        assert!(state.get_custom_prompt_input().is_empty());
    }
    #[test]
    fn core_user_work_admission_binds_exact_namespace_and_drains_before_retirement() {
        let fixture = client_state::tests::Fixture::new(false, false);
        let lease = fixture.lease("11111111-1111-4111-8111-111111111111", 4, 7);
        let active = Arc::new(Mutex::new(None));
        let activity = UserActivityGate::default();
        let admission = UserWorkAdmission::new(active.clone(), activity.clone());
        let scope = SessionScope { owner_user_id: lease.namespace.user_public_id().into(), auth_epoch: 4 };
        assert!(admission.begin(&scope).is_err());
        activity.prepare_activation(lease.clone()).unwrap().publish();
        *active.lock().unwrap() = Some(lease.clone());
        assert!(admission.begin(&SessionScope { auth_epoch: 3, ..scope.clone() }).is_err());
        assert!(admission.begin(&SessionScope { owner_user_id: "22222222-2222-4222-8222-222222222222".into(), ..scope.clone() }).is_err());
        let unit = admission.begin(&scope).unwrap();
        std::thread::scope(|workers| {
            let (sent, received) = mpsc::channel();
            let activity = &activity; let lease = &lease;
            let worker = workers.spawn(move || { let retired = activity.begin_quiesce(lease).unwrap(); sent.send(()).unwrap(); retired });
            while !unit.is_quiescing() { std::thread::yield_now(); }
            assert!(!admission.is_current(&scope));
            assert!(admission.begin(&scope).is_err());
            assert!(received.try_recv().is_err());
            drop(unit);
            received.recv().unwrap();
            worker.join().unwrap().retire();
        });
        assert!(admission.begin(&scope).is_err());
    }
    #[test]
    fn core_account_transition_ticket_is_owned_and_stale_release_cannot_reopen() {
        let admission = AccountTransitionAdmission::default();
        let first = admission.begin().unwrap();
        assert!(admission.begin().is_err());
        drop(first);
        let second = admission.begin().unwrap();
        assert!(admission.begin().is_err());
        drop(second);
        assert!(admission.begin().is_ok());
    }

    #[test]
    fn core_private_persistence_requires_live_binding_and_retirement_stops_clones() {
        let fixture = client_state::tests::Fixture::new(false, false);
        let lease = fixture.lease("11111111-1111-4111-8111-111111111111", 1, 1);
        let activity = UserActivityGate::default();
        let latch = UpgradeLatch::default();
        let authority = PrivatePersistence::new((*fixture).clone(), lease.clone(), activity.clone(), latch.clone());
        let clone = authority.clone();
        assert!(authority.save_store(LocalStoreData::default()).is_err());
        fixture.activate(lease.clone()).unwrap();
        activity.prepare_activation(lease.clone()).unwrap().publish();
        authority.save_store(LocalStoreData::default()).unwrap();
        assert!(fixture.load_client_state_for_namespace(&lease).unwrap().is_some());
        let quiesced = activity.begin_quiesce(&lease).unwrap();
        fixture.flush_for_retirement(&lease).unwrap().retire_flushed();
        quiesced.retire();
        assert!(clone.save_store(LocalStoreData::default()).is_err());
        assert!(fixture.load_client_state_for_namespace(&lease).unwrap().is_some());
        latch.trip(RequiredUpgrade { minimum_version: None });
        assert!(authority.save_store(LocalStoreData::default()).is_err());
    }
    #[test]
    fn core_activity_quiesce_drains_real_permits_and_reopens_exact_lease_on_abort() {
        let root = tempfile::tempdir().unwrap();
        let lease = NamespaceLease {
            namespace: UserNamespace::new(root.path(), "11111111-1111-4111-8111-111111111111").unwrap(),
            auth_epoch: 4, namespace_epoch: 7,
        };
        let activity = UserActivityGate::default();
        activity.activate(lease.clone()).unwrap();
        let permit = activity.begin_recovery_unit(&lease).unwrap();
        std::thread::scope(|threads| {
            let (send, receive) = mpsc::channel();
            let activity = &activity;
            let lease = &lease;
            let worker = threads.spawn(move || {
                let quiesced = activity.begin_quiesce(&lease).unwrap();
                send.send(()).unwrap();
                quiesced
            });
            while !permit.is_quiescing() { std::thread::yield_now(); }
            assert!(activity.begin_recovery_unit(&lease).is_err());
            assert!(receive.try_recv().is_err());
            drop(permit);
            receive.recv().unwrap();
            drop(worker.join().unwrap());
        });
        assert!(activity.begin_recovery_unit(&lease).is_ok());
        let quiesced = activity.begin_quiesce(&lease).unwrap();
        quiesced.retire();
        assert!(activity.begin_recovery_unit(&lease).is_err());
    }
}
