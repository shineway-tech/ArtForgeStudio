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

#[derive(Clone)]
struct ReferenceData {
    id: String,
    source_path: String,
}

#[derive(Clone, Default)]
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
        message: String,
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

#[derive(Clone)]
struct ActiveGeneration {
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
}

impl Default for ActiveGeneration {
    fn default() -> Self {
        Self {
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

#[derive(Default)]
struct Store {
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

#[derive(Clone)]
struct PendingCreditRedemption {
    code: String,
    client_request_id: String,
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
    delivery_downloads: RefCell<BTreeSet<String>>,
}

#[derive(Clone)]
struct ActivePaymentSession {
    client_request_id: String,
    checkout_url: Option<String>,
    session_scope: SessionScope,
}

#[derive(Clone, Default)]
struct AppContext {
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

#[derive(Clone, Default, Serialize, Deserialize)]
struct LocalStoreData {
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
    theme_id: String,
    #[serde(default = "default_card_style")]
    card_style: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    asset_type: String,
    #[serde(default)]
    ui_preferences: UiPreferencesData,
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
