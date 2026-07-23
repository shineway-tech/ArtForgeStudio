const IMAGE_GENERATION_WAIT_SECS: u64 = 900;
const IMAGE_POLL_INTERVAL_MS: u64 = 2000;
const MAX_REFERENCE_IMAGES: usize = 4;
const IMAGE_DRAG_MIME: &str = "application/x-artforge-image-path";
const URI_LIST_MIME: &str = "text/uri-list";
const TEXT_PLAIN_MIME: &str = "text/plain";
const ACTION_SEQUENCE_RATIOS: [(&'static str, i32, i32); 3] =
    [("1:1", 1, 1), ("9:16", 9, 16), ("16:9", 16, 9)];

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

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
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
    #[serde(skip)]
    selected: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct CanvasLinkData {
    id: String,
    source_id: String,
    target_id: String,
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
    width: i32,
    height: i32,
    image: Image,
    source_path: String,
    cutout_done: bool,
    remove_black_done: bool,
    upscale_done: bool,
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
    image: Image,
    source_path: String,
}

#[derive(Clone, Default)]
struct ReferenceGroups {
    character: Vec<ReferenceData>,
    scene: Vec<ReferenceData>,
    ui: Vec<ReferenceData>,
    effect: Vec<ReferenceData>,
    action_sequence: Vec<ReferenceData>,
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
        bytes: Vec<u8>,
        optimized: String,
        time: String,
        upscale_done: bool,
        delivery: Option<DeliveryConfirmation>,
    },
    ImageFailure {
        reason: String,
        time: String,
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

#[derive(Clone)]
struct DeliveryConfirmation {
    client_request_id: String,
    item_index: usize,
    task_id: String,
    file_id: String,
    sha256: String,
    size_bytes: u64,
}

#[derive(Clone, Default)]
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
    progress: i32,
    eta: i32,
}

#[derive(Default)]
struct Store {
    model_groups: Vec<ModelGroupData>,
    generations: Vec<AssetData>,
    assets: Vec<AssetData>,
    inspiration: Vec<AssetData>,
    notifications: Vec<NotificationData>,
    references: ReferenceGroups,
    prompt_drafts: PromptDrafts,
    custom_prompts: Vec<String>,
    custom_prompt_times: BTreeMap<String, String>,
    canvas_notes: Vec<CanvasNoteData>,
    canvas_links: Vec<CanvasLinkData>,
    credit_ledger_pagination: CreditLedgerPagination,
}

#[derive(Default)]
struct GenerationRegistry {
    active: RefCell<BTreeMap<String, ActiveGeneration>>,
    statuses: RefCell<BTreeMap<String, String>>,
}

#[derive(Clone, Default)]
struct AppContext {
    store: Rc<RefCell<Store>>,
    generations: Rc<GenerationRegistry>,
    recovering_orders: Rc<RefCell<BTreeSet<String>>>,
    active_payment_request: Rc<RefCell<Option<String>>>,
    cancelled_payment_requests: Rc<RefCell<BTreeSet<String>>>,
    cancelled_generation_requests: Arc<Mutex<BTreeSet<String>>>,
    backend: Option<Arc<BackendRuntime>>,
}

#[derive(Default, Serialize, Deserialize)]
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
    prompt_drafts: PromptDrafts,
    #[serde(default)]
    custom_prompts: Vec<String>,
    #[serde(default)]
    custom_prompt_times: BTreeMap<String, String>,
    #[serde(default)]
    canvas_notes: Vec<CanvasNoteData>,
    #[serde(default)]
    canvas_links: Vec<CanvasLinkData>,
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
    action_sequence: String,
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
    width: i32,
    height: i32,
    source_path: String,
    #[serde(default)]
    cutout_done: bool,
    #[serde(default)]
    remove_black_done: bool,
    #[serde(default)]
    upscale_done: bool,
}

#[derive(Default, Serialize, Deserialize)]
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
}

fn default_card_style() -> String {
    "rounded".to_string()
}

#[derive(Default, Deserialize)]
struct UpdateManifest {
    version: String,
}
