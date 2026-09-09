const IMAGE_GENERATION_WAIT_SECS: u64 = 900;
const IMAGE_POLL_INTERVAL_MS: u64 = 2000;
const MAX_REFERENCE_IMAGES: usize = 8;
const IMAGE_DRAG_MIME: &str = "application/x-artforge-image-path";
const URI_LIST_MIME: &str = "text/uri-list";
const TEXT_PLAIN_MIME: &str = "text/plain";

#[derive(Clone, Default, Serialize, Deserialize)]
struct ModelOptionData {
    code: String,
    name: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct ModelGroupData {
    #[serde(default)]
    kind: String,
    name: String,
    models: Vec<ModelOptionData>,
    #[serde(default)]
    used_models: Vec<String>,
    selected_model: String,
}

fn default_canvas_node_kind() -> String {
    "text".to_string()
}

fn default_canvas_node_width() -> f32 {
    280.0
}

fn default_canvas_node_height() -> f32 {
    176.0
}

fn default_canvas_font_size() -> f32 {
    12.0
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct CanvasNoteData {
    id: String,
    #[serde(default = "default_canvas_node_kind")]
    kind: String,
    content: String,
    x: f32,
    y: f32,
    #[serde(default = "default_canvas_node_width")]
    width: f32,
    #[serde(default = "default_canvas_node_height")]
    height: f32,
    #[serde(default)]
    parent_group_id: String,
    #[serde(default)]
    z_index: i32,
    #[serde(default)]
    image_path: String,
    #[serde(default = "default_canvas_font_size")]
    font_size: f32,
    #[serde(skip)]
    selected: bool,
}

impl Default for CanvasNoteData {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: default_canvas_node_kind(),
            content: String::new(),
            x: 0.0,
            y: 0.0,
            width: default_canvas_node_width(),
            height: default_canvas_node_height(),
            parent_group_id: String::new(),
            z_index: 0,
            image_path: String::new(),
            font_size: default_canvas_font_size(),
            selected: false,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct CanvasLinkData {
    id: String,
    source_id: String,
    target_id: String,
    #[serde(default)]
    flow_reversed: bool,
}

fn normalize_canvas_groups(notes: &mut [CanvasNoteData]) {
    let group_ids = notes
        .iter()
        .filter(|note| note.kind == "group")
        .map(|note| note.id.clone())
        .collect::<BTreeSet<_>>();

    for note in notes.iter_mut() {
        if note.parent_group_id == note.id || !group_ids.contains(&note.parent_group_id) {
            note.parent_group_id.clear();
        }
    }

    let parents = notes
        .iter()
        .map(|note| (note.id.clone(), note.parent_group_id.clone()))
        .collect::<BTreeMap<_, _>>();
    for note in notes.iter_mut() {
        let mut current = note.parent_group_id.as_str();
        let mut visited = BTreeSet::from([note.id.as_str()]);
        while !current.is_empty() {
            if !visited.insert(current) {
                note.parent_group_id.clear();
                break;
            }
            current = parents.get(current).map(String::as_str).unwrap_or_default();
        }
    }
}

#[derive(Clone)]
struct AssetData {
    id: String,
    conversation_id: String,
    title: String,
    category: String,
    kind: String,
    time: String,
    prompt: String,
    ratio: String,
    quality: String,
    model: String,
    origin: String,
    width: i32,
    height: i32,
    source_path: String,
    reference_paths: Vec<String>,
    cutout_done: bool,
    remove_black_done: bool,
    upscale_done: bool,
    is_new: bool,
    delivery_recoverable: bool,
    delivery_downloading: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct NotificationData {
    id: String,
    title: String,
    model: String,
    time: String,
    reason: String,
    success: bool,
    read: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct ReferenceData {
    id: String,
    source_path: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct ReferenceGroups {
    character: Vec<ReferenceData>,
    scene: Vec<ReferenceData>,
    ui: Vec<ReferenceData>,
    effect: Vec<ReferenceData>,
}

#[derive(Clone)]
struct QuoteContext {
    title: String,
    prompt: String,
    ratio: String,
    quality: String,
    width: i32,
    height: i32,
}

#[derive(Clone)]
struct PromptControls {
    category: String,
    creation: String,
    style: String,
    view: String,
    weather: String,
    time: String,
    light: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptLanguage {
    Chinese,
    English,
}

enum GenerationOutcome {
    Accepted {
        task_id: String,
    },
    Progress {
        percent: i32,
    },
    NamespaceImageSuccess {
        prepared: Box<PreparedNamespaceDelivery>,
        time: String,
    },
    ImageSuccess {
        local_path: String,
        display_prompt: String,
        time: String,
        upscale_done: bool,
        delivery: Option<DeliveryConfirmation>,
    },
    ImageFailure {
        reason: String,
        time: String,
        delivery: Option<DeliveryConfirmation>,
    },
    Finished,
    CreditInsufficient {
        message: ApiError,
    },
    Failure {
        reason: String,
        time: String,
    },
}

enum WatermarkOutcome {
    Accepted {
        task_id: String,
    },
    Progress {
        percent: i32,
    },
    Success {
        bytes: Vec<u8>,
        delivery: DeliveryConfirmation,
    },
    Recovered {
        local_path: String,
        delivery: Option<DeliveryConfirmation>,
    },
    CreditInsufficient {
        message: String,
    },
    Failure {
        reason: String,
    },
}

enum ImageColorizationOutcome {
    Accepted {
        task_id: String,
    },
    Progress {
        percent: i32,
    },
    Success {
        bytes: Vec<u8>,
        delivery: DeliveryConfirmation,
    },
    Recovered {
        local_path: String,
        delivery: Option<DeliveryConfirmation>,
    },
    CreditInsufficient {
        message: String,
    },
    Failure {
        reason: String,
    },
}

enum ImageEnhancementOutcome {
    Accepted {
        task_id: String,
    },
    Progress {
        percent: i32,
    },
    Success {
        bytes: Vec<u8>,
        delivery: DeliveryConfirmation,
    },
    Recovered {
        local_path: String,
        delivery: Option<DeliveryConfirmation>,
    },
    CreditInsufficient {
        message: String,
    },
    Failure {
        reason: String,
    },
}

#[derive(Clone)]
struct DeliveryConfirmation {
    client_request_id: String,
    item_index: usize,
    task_id: String,
    file_id: String,
    sha256: String,
    size_bytes: u64,
    failed_asset_id: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DeliveryDownloadKey {
    owner_user_id: String,
    auth_epoch: u64,
    client_request_id: String,
    file_id: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DeliveryDownloadReservation {
    key: DeliveryDownloadKey,
    reservation_id: u64,
}

#[derive(Clone)]
struct ActiveGeneration {
    registered_cancel_owner:Option<PrivatePersistence>,
    task_id: String,
    client_request_id: Option<String>,
    server_task_id: Option<String>,
    category: String,
    conversation_id: String,
    prompt: String,
    credit_cost: i32,
    total_count: i32,
    loading_count: i32,
    completed_count: i32,
    success_count: i32,
    failed_count: i32,
    last_failure_reason: Option<String>,
    progress: i32,
    eta: i32,
    latest_success_id: Option<String>,
    session_scope: SessionScope,
    destination: GenerationDestination,
    delivery_download_reservations: Vec<DeliveryDownloadReservation>,
}

impl Default for ActiveGeneration {
    fn default() -> Self {
        Self {
            registered_cancel_owner:None,
            task_id: String::new(),
            client_request_id: None,
            server_task_id: None,
            category: String::new(),
            conversation_id: String::new(),
            prompt: String::new(),
            credit_cost: 0,
            total_count: 0,
            loading_count: 0,
            completed_count: 0,
            success_count: 0,
            failed_count: 0,
            last_failure_reason: None,
            progress: 0,
            eta: 0,
            latest_success_id: None,
            session_scope: SessionScope {
                owner_user_id: String::new(),
                auth_epoch: 0,
            },
            destination: GenerationDestination::Gallery,
            delivery_download_reservations: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum GenerationDestination {
    #[default]
    Gallery,
    Canvas {
        source_node_id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExistingGenerationPolicy {
    StopExisting,
    KeepExisting,
}

const DEFAULT_CANVAS_WORKSPACE_ID: &str = "infinite-canvas";

fn normalize_canvas_workspace_id(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        DEFAULT_CANVAS_WORKSPACE_ID.to_string()
    } else {
        value.to_string()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct CanvasWorkspaceData {
    #[serde(default)]
    notes: Vec<CanvasNoteData>,
    #[serde(default)]
    links: Vec<CanvasLinkData>,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    references: Vec<ReferenceData>,
}

/// A namespace-owned video delivery. Its key and immutable server identity are
/// persisted in the same transaction as the private Store; no image decoder is involved.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(super) struct SavedVideoOutput {
    #[serde(default)]
    pub(super) source_asset_id: String,
    pub(super) client_request_id: String,
    pub(super) server_task_id: String,
    pub(super) file_id: String,
    pub(super) billing_account_group_id: String,
    pub(super) sha256: String,
    pub(super) size_bytes: u64,
    pub(super) source_path: String,
    pub(super) title: String,
    pub(super) created_at: String,
}
impl SavedVideoOutput {
    pub(super) fn key(&self) -> String { format!("{}:{}",self.server_task_id,self.file_id) }
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.client_request_id.trim().is_empty() && !self.source_path.is_empty() && self.size_bytes>0,
            "saved video metadata incomplete");
        for id in [&self.server_task_id,&self.file_id,&self.billing_account_group_id] {
            anyhow::ensure!(api::uuid_path_segment(id).is_ok_and(|canonical| &canonical==id),"saved video identity invalid");
        }
        anyhow::ensure!(self.sha256.len()==64 && self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),"saved video digest invalid");
        Ok(())
    }
}
#[derive(Default)]
struct Store {
    video_outputs: BTreeMap<String,SavedVideoOutput>,
    private_persistence: Option<PrivatePersistence>,
    model_groups: Vec<ModelGroupData>,
    generations: Vec<AssetData>,
    assets: Vec<AssetData>,
    inspiration: Vec<AssetData>,
    notifications: Vec<NotificationData>,
    notification_page_epoch: u64,
    references: ReferenceGroups,
    prompt_drafts: PromptDrafts,
    dismissed_prompt_history: BTreeSet<String>,
    custom_prompts: Vec<String>,
    selected_custom_prompts: BTreeMap<String, BTreeSet<String>>,
    custom_prompt_times: BTreeMap<String, String>,
    custom_prompt_profiles: BTreeMap<String, CustomPromptProfile>,
    canvas_notes: Vec<CanvasNoteData>,
    canvas_links: Vec<CanvasLinkData>,
    canvas_references: Vec<ReferenceData>,
    active_canvas_workspace_id: String,
    canvas_workspaces: BTreeMap<String, CanvasWorkspaceData>,
    credit_ledger_pagination: CreditLedgerPagination,
    /// Last applied server credit-account version. This prevents an idempotency replay or a
    /// slower account refresh from moving the visible balance backwards.
    credit_account_version: Option<String>,
    /// Orders every request that can replace the combined credit balance and ledger view.
    /// Full backend snapshots, redemption reconciliation, and ledger pagination all share this
    /// epoch so an older response can never overwrite a newer credit-state operation.
    credit_sync_epoch: u64,
    /// Only the newest lightweight credit-account refresh may update the current account view.
    credit_account_refresh_epoch: u64,
    /// Keep the same request id while an outcome is ambiguous (for example a timeout after the
    /// server committed). Entries are account-scoped so one user can never replay another's code.
    pending_credit_redemptions_by_owner: BTreeMap<String, PendingCreditRedemption>,
    /// Server task ids are account-bound. Keep them partitioned by backend user id so a
    /// different account can neither overwrite nor resume another account's task.
    deep_prompt_jobs_by_owner: BTreeMap<String, String>,
    /// A billable create request is persisted before it is sent. If the response is lost, the
    /// same account replays this exact request id and body instead of creating a second job.
    deep_prompt_pending_requests_by_owner: BTreeMap<String, CreatePromptOptimization>,
    /// Pre-partition local stores only persisted a bare task id. It remains quarantined until
    /// an account-scoped server lookup proves which signed-in account owns it.
    legacy_deep_prompt_job_id: String,
    deep_prompt_bindings: BTreeMap<String, DeepPromptBinding>,
    contact_popup_dismissed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct PendingCreditRedemption {
    code: String,
    client_request_id: String,
    billing_account_group_id: String,
}

fn begin_credit_sync_epoch(store: &mut Store) -> u64 {
    store.credit_sync_epoch = store.credit_sync_epoch.wrapping_add(1);
    store.credit_sync_epoch
}

fn invalidate_credit_sync_epoch(store: &mut Store) {
    let _ = begin_credit_sync_epoch(store);
}

fn credit_sync_epoch_is_current(store: &Store, request_epoch: u64) -> bool {
    store.credit_sync_epoch == request_epoch
}

#[derive(Default)]
struct GenerationRegistry {
    active: RefCell<BTreeMap<String, ActiveGeneration>>,
    statuses: RefCell<BTreeMap<String, String>>,
    delivery_downloads: RefCell<BTreeMap<DeliveryDownloadKey, u64>>,
    next_delivery_download_reservation_id: Cell<u64>,
}

#[derive(Clone)]
struct ActivePaymentSession {
    client_request_id: String,
    billing_account_group_id: String,
    checkout_url: Option<String>,
    session_scope: SessionScope,
}

#[derive(Clone, Default)]
struct AppContext {
    data_root_capability: Option<Arc<DataRootCapability>>,
    file_index: Option<FileIndex>,
    active_namespace: Arc<Mutex<Option<NamespaceLease>>>,
    namespace_operations: NamespaceOperationGate,
    user_activity: UserActivityGate,
    billing_context: Arc<BillingContextManager>,
    account_transition: Option<Rc<AccountTransitionCoordinator>>,
    team_groups: Rc<RefCell<Vec<AccountGroupChoice>>>,
    store: Rc<RefCell<Store>>,
    canvas_history: Rc<RefCell<CanvasController>>,
    generations: Rc<GenerationRegistry>,
    recovering_orders: Rc<RefCell<BTreeSet<String>>>,
    active_payment: Rc<RefCell<Option<ActivePaymentSession>>>,
    cancelled_generation_requests: Arc<Mutex<BTreeSet<String>>>,
    active_prompt_task_requests: Arc<Mutex<BTreeSet<String>>>,
    auth_operation_epoch: Arc<AtomicU64>,
    current_user_id: Arc<Mutex<Option<String>>>,
    account_snapshot_scope: Arc<Mutex<Option<SessionScope>>>,
    prompt_optimization_polling: Rc<RefCell<Option<String>>>,
    backend: Option<Arc<BackendRuntime>>,
}

impl AppContext {
    fn capture_billing_action(&self, capability: KnownCapability) -> std::result::Result<(BillingScope, Arc<NamespaceStorageAuthority>, UserActivityPermit), ApiError> {
        let backend = self.backend.as_ref().ok_or(ApiError::AuthenticationRequired)?;
        if let Some(required) = backend.api.upgrade_latch().snapshot() { return Err(required.as_error()); }
        let scope = self.billing_context.current_scope(capability)?;
        let lease = self.namespace_for(&scope.request.session)?;
        let permit = self.user_activity.begin_recovery_unit(&lease).map_err(transition_error)?;
        let authority = Arc::new(self.storage_authority_for(&lease)?);
        if !self.billing_context.is_current(&scope) || permit.is_quiescing() { return Err(ApiError::AuthenticationRequired); }
        Ok((scope, authority, permit))
    }
    fn namespace_for(&self, scope: &SessionScope) -> std::result::Result<NamespaceLease, ApiError> {
        let backend = self.backend.as_ref().ok_or(ApiError::AuthenticationRequired)?;
        if !backend.api.session().is_scope_current(scope) { return Err(ApiError::AuthenticationRequired); }
        self.active_namespace.lock().map_err(transition_error)?.as_ref()
            .filter(|lease| lease.auth_epoch == scope.auth_epoch && lease.namespace.user_public_id() == scope.owner_user_id)
            .cloned().ok_or_else(|| ApiError::LocalState { message: "用户命名空间尚未激活".into() })
    }
    fn apply_user_completion<R>(&self, lease: &NamespaceLease, apply: impl FnOnce() -> R) -> std::result::Result<R, ApiError> {
        let _permit = self.user_activity.begin_recovery_unit(lease).map_err(transition_error)?;
        if self.active_namespace.lock().map_err(transition_error)?.as_ref() != Some(lease) { return Err(ApiError::AuthenticationRequired); }
        let backend = self.backend.as_ref().ok_or(ApiError::AuthenticationRequired)?;
        backend.api.upgrade_latch().apply_if_open(apply).map_err(|required| required.as_error())
    }
    fn storage_authority(
        &self,
        lease: &NamespaceLease,
    ) -> std::result::Result<NamespaceStorageAuthority, ApiError> {
        self.storage_authority_for(lease)
    }
    fn storage_authority_for(&self, lease: &NamespaceLease) -> std::result::Result<NamespaceStorageAuthority, ApiError> {
        if self.active_namespace.lock().map_err(transition_error)?.as_ref() != Some(lease)
            || self.namespace_operations.active_lease().as_ref() != Some(lease) {
            return Err(ApiError::LocalState { message: "用户命名空间已失效或未激活".into() });
        }
        let root = self
            .data_root_capability
            .as_ref()
            .ok_or_else(|| ApiError::LocalState {
                message: "retained data-root capability is unavailable".into(),
            })?;
        let client = self.backend.as_ref().ok_or(ApiError::AuthenticationRequired)?.api.clone();
        let index = self.file_index.clone().ok_or_else(|| ApiError::LocalState { message: "文件索引尚未初始化".into() })?;
        NamespaceStorageAuthority::open_active(Arc::clone(root), lease, client, index).map_err(|error| {
            ApiError::LocalState {
                message: format!("cannot open captured namespace storage: {error:#}"),
            }
        })
    }

    fn current_account_session_scope(&self) -> Option<SessionScope> {
        let owner_user_id = self
            .current_user_id
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .clone()
            .filter(|value| !value.trim().is_empty())?;
        self.backend
            .as_ref()?
            .api
            .session()
            .scope_for_user(&owner_user_id)
    }

    fn account_scope_disposition(&self, scope: &SessionScope) -> AccountScopeDisposition {
        let current_owner_user_id = self
            .current_user_id
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .clone();
        let Some(backend) = self.backend.as_ref() else {
            return AccountScopeDisposition::Stale;
        };
        account_scope_disposition(
            current_owner_user_id.as_deref(),
            backend.api.session(),
            scope,
        )
    }
}

#[cfg(test)]
mod namespace_storage_authority_bridge_tests {
    use super::*;
    fn lease(path: &Path) -> NamespaceLease {
        NamespaceLease {
            namespace: UserNamespace::new(path, "11111111-1111-4111-8111-111111111111").unwrap(),
            auth_epoch: 81,
            namespace_epoch: 13,
        }
    }
    #[test]
    fn task8b_bridge_absent_root_fails_before_namespace_creation() {
        let directory = tempfile::tempdir().unwrap();
        let lease = lease(&directory.path().canonicalize().unwrap());
        assert!(matches!(
            AppContext::default().storage_authority(&lease),
            Err(ApiError::LocalState { .. })
        ));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
    #[test]
    fn task8b_bridge_retains_explicit_user_and_epochs() {
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let lease = fixture.persistence.lease().clone();
        assert!(fixture.context.storage_authority(&lease).is_err(),"unpublished namespace must refuse");
        let transition=fixture.context.namespace_operations.try_begin_transition().unwrap();
        let proof=transition.begin_prepublication_recovery(&lease).unwrap();
        proof.verify_no_unsupported_imports(&fixture.authority).unwrap();
        let recovered=proof.finish().unwrap();
        transition.prepare_publication(&lease,recovered).unwrap().publish();
        let authority=fixture.context.storage_authority(&lease).unwrap();
        assert_eq!(authority.lease(),&lease);
        assert_eq!(authority.user_public_id(),lease.namespace.user_public_id());
        let changed=NamespaceLease { auth_epoch:lease.auth_epoch+1,namespace_epoch:lease.namespace_epoch+1,..lease.clone() };
        assert!(fixture.context.storage_authority(&changed).is_err(),"caller cannot synthesize new epochs");
        authority.create_new_regular(&ManagedFileKey::new(ManagedUserArea::Output,"proof").unwrap()).unwrap();
        assert!(lease.namespace.output_dir().join("proof").is_file());
        fixture.drain();
    }

    #[test]
    fn task8b_bridge_wrong_root_does_not_reopen_or_create() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let root =
            Arc::new(NamespaceFs::open_data_root(&first.path().canonicalize().unwrap()).unwrap());
        let context = AppContext {
            data_root_capability: Some(root),
            ..AppContext::default()
        };
        assert!(matches!(
            context.storage_authority(&lease(&second.path().canonicalize().unwrap())),
            Err(ApiError::LocalState { .. })
        ));
        assert_eq!(fs::read_dir(first.path()).unwrap().count(), 0);
        assert_eq!(fs::read_dir(second.path()).unwrap().count(), 0);
    }
    #[test]
    fn task8b_bridge_replaced_parent_fails_without_creating_in_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().canonicalize().unwrap().join("parent");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("root");
        fs::create_dir(&path).unwrap();
        let context = AppContext {
            data_root_capability: Some(Arc::new(NamespaceFs::open_data_root(&path).unwrap())),
            ..AppContext::default()
        };
        let lease = lease(&path);
        fs::rename(&parent, parent.with_file_name("retained")).unwrap();
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            context.storage_authority(&lease),
            Err(ApiError::LocalState { .. })
        ));
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct LocalStoreData {
    #[serde(default)]
    video_outputs: BTreeMap<String,SavedVideoOutput>,
    #[serde(default)]
    references: ReferenceGroups,
    #[serde(default)]
    pending_credit_redemptions_by_owner: BTreeMap<String, PendingCreditRedemption>,
    #[serde(default)]
    generations: Vec<StoredAssetData>,
    #[serde(default)]
    assets: Vec<StoredAssetData>,
    #[serde(default)]
    notifications: Vec<NotificationData>,
    #[serde(default)]
    image_model: String,
    #[serde(default)]
    reasoning_model: String,
    #[serde(default)]
    video_model: String,
    #[serde(default)]
    prompt_drafts: PromptDrafts,
    #[serde(default)]
    dismissed_prompt_history: BTreeSet<String>,
    #[serde(default)]
    custom_prompts: Vec<String>,
    #[serde(default)]
    selected_custom_prompts: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    custom_prompt_times: BTreeMap<String, String>,
    #[serde(default)]
    custom_prompt_profiles: BTreeMap<String, CustomPromptProfile>,
    #[serde(default)]
    canvas_notes: Vec<CanvasNoteData>,
    #[serde(default)]
    canvas_links: Vec<CanvasLinkData>,
    #[serde(default)]
    active_canvas_workspace_id: String,
    #[serde(default)]
    canvas_workspaces: BTreeMap<String, CanvasWorkspaceData>,
    #[serde(default)]
    deep_prompt_job_id: String,
    #[serde(default)]
    deep_prompt_jobs_by_owner: BTreeMap<String, String>,
    #[serde(default)]
    deep_prompt_pending_requests_by_owner: BTreeMap<String, CreatePromptOptimization>,
    #[serde(default)]
    deep_prompt_bindings: BTreeMap<String, DeepPromptBinding>,
    #[serde(default)]
    contact_popup_dismissed: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct DeepPromptBinding {
    chinese: String,
    english: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct CustomPromptProfile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    format: String,
    #[serde(default)]
    negative_prompt: String,
    #[serde(default)]
    reference_path: String,
    #[serde(default)]
    reference_paths: Vec<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct PromptDrafts {
    #[serde(default)]
    video_by_owner: BTreeMap<String, VideoPromptDraft>,
    #[serde(default)]
    character: String,
    #[serde(default)]
    scene: String,
    #[serde(default)]
    ui: String,
    #[serde(default)]
    effect: String,
    #[serde(default)]
    negative_character: String,
    #[serde(default)]
    negative_scene: String,
    #[serde(default)]
    negative_ui: String,
    #[serde(default)]
    negative_effect: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct VideoPromptDraft {
    source_id: String,
    prompt: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredAssetData {
    id: String,
    conversation_id: String,
    title: String,
    category: String,
    kind: String,
    time: String,
    prompt: String,
    ratio: String,
    quality: String,
    model: String,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    width: i32,
    #[serde(default)]
    height: i32,
    source_path: String,
    #[serde(default)]
    reference_paths: Vec<String>,
    #[serde(default)]
    cutout_done: bool,
    #[serde(default)]
    remove_black_done: bool,
    #[serde(default)]
    upscale_done: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct UserProfileData {
    #[serde(default)]
    logged_in: bool,
    #[serde(default)]
    nickname: String,
    #[serde(default)]
    backend_auth_version: u32,
    #[serde(default)]
    ever_authenticated: bool,
    #[serde(default)]
    email_mask: String,
    #[serde(default)]
    accepted_user_terms_version: String,
    #[serde(default)]
    accepted_privacy_version: String,
    #[serde(default)]
    asset_type: String,
}

// Quarantine-only decoder. Never serialize this v1 identity/presentation shape
// into an assigned user's settings.
#[derive(Deserialize)]
struct LegacyUserProfileData {
    #[serde(default)]
    theme_id: String,
    #[serde(default = "default_card_style")]
    card_style: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    close_behavior: String,
    #[serde(default)]
    ui_preferences: UiPreferencesData,
}

impl LegacyUserProfileData {
    fn device_settings(&self) -> DeviceSettings {
        DeviceSettings {
            theme_id: self.theme_id.clone(),
            card_style: self.card_style.clone(),
            language: self.language.clone(),
            close_behavior: self.close_behavior.clone(),
            generation_gallery_layout: self.ui_preferences.generation_gallery_layout.clone(),
            asset_gallery_layout: self.ui_preferences.asset_gallery_layout.clone(),
            inspiration_gallery_layout: self.ui_preferences.inspiration_gallery_layout.clone(),
        }
        .normalized()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct DeviceSettings {
    theme_id: String,
    card_style: String,
    language: String,
    close_behavior: String,
    generation_gallery_layout: String,
    asset_gallery_layout: String,
    inspiration_gallery_layout: String,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            theme_id: "light".into(),
            card_style: default_card_style(),
            language: "zh".into(),
            close_behavior: "ask".into(),
            generation_gallery_layout: default_gallery_layout(),
            asset_gallery_layout: default_gallery_layout(),
            inspiration_gallery_layout: default_gallery_layout(),
        }
    }
}

impl DeviceSettings {
    fn normalized(&self) -> Self {
        Self {
            theme_id: if self.theme_id.trim().is_empty() {
                "light".into()
            } else {
                self.theme_id.trim().into()
            },
            card_style: if self.card_style == "square" {
                "square".into()
            } else {
                "rounded".into()
            },
            language: if self.language.trim().is_empty() {
                "zh".into()
            } else {
                self.language.trim().into()
            },
            close_behavior: normalize_close_behavior(&self.close_behavior).into(),
            generation_gallery_layout: normalize_gallery_layout(&self.generation_gallery_layout)
                .into(),
            asset_gallery_layout: normalize_gallery_layout(&self.asset_gallery_layout).into(),
            inspiration_gallery_layout: normalize_gallery_layout(&self.inspiration_gallery_layout)
                .into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ExportDirectoryPreference {
    normalized_path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
struct UiPreferencesData {
    #[serde(default = "default_gallery_layout")]
    generation_gallery_layout: String,
    #[serde(default = "default_gallery_layout")]
    asset_gallery_layout: String,
    #[serde(default = "default_gallery_layout")]
    inspiration_gallery_layout: String,
}

impl Default for UiPreferencesData {
    fn default() -> Self {
        Self {
            generation_gallery_layout: default_gallery_layout(),
            asset_gallery_layout: default_gallery_layout(),
            inspiration_gallery_layout: default_gallery_layout(),
        }
    }
}

fn default_gallery_layout() -> String {
    "grid".to_string()
}

fn default_card_style() -> String {
    "rounded".to_string()
}

#[derive(Clone, Default, Deserialize)]
struct UpdateManifest {
    version: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    downloads: UpdateDownloads,
    #[serde(default)]
    artifacts: UpdateArtifacts,
}

#[derive(Clone, Default, Deserialize)]
struct UpdateDownloads {
    #[serde(default)]
    macos_aarch64: String,
    #[serde(default)]
    macos_x64: String,
    #[serde(default)]
    windows_x64: String,
}

#[derive(Clone, Default, Deserialize)]
struct UpdateArtifacts {
    #[serde(default)]
    macos_aarch64: UpdateArtifact,
    #[serde(default)]
    macos_x64: UpdateArtifact,
    #[serde(default)]
    windows_x64: UpdateArtifact,
}

#[derive(Clone, Default, Deserialize)]
struct UpdateArtifact {
    #[serde(default)]
    size_bytes: u64,
    #[serde(default)]
    sha256: String,
}
