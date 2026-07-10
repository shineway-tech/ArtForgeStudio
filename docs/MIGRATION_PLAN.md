# ArtStudio ← Moyin Creator 迁移计划

> 路线C：完整迁移 5 步流水线（剧本 → 角色 → 场景 → 导演/分镜 → Seedance 2.0）  
> 第一阶段：角色系统（Character System）

---

## 总览：5 阶段路线图

```
Phase 1: 角色系统 (Character)          ← 当前阶段
Phase 2: 剧本解析引擎 (Script)
Phase 3: 场景系统 (Scene)
Phase 4: 导演/分镜系统 (Director)
Phase 5: Seedance 2.0 视频 (S-Class)
```

每个 Phase 遵循同样的模式：**数据模型 → Service 层 → Provider 能力 → Slint UI → 集成到流水线**

---

## Phase 1：角色系统 (Character System)

### 1.1 数据模型层 (`artait-model`)

在 `crates/artait-model/src/` 下新建 `character.rs`，定义以下核心结构体：

```rust
// ===== 6 层身份锚点（角色一致性核心） =====
pub struct CharacterIdentityAnchors {
    // ① 骨相层
    pub face_shape: Option<String>,       // oval/square/heart/round/diamond/oblong
    pub jawline: Option<String>,          // sharp angular/soft rounded/prominent
    pub cheekbones: Option<String>,       // high prominent/subtle/wide set
    
    // ② 五官层
    pub eye_shape: Option<String>,        // almond/round/hooded/monolid/upturned
    pub eye_details: Option<String>,      
    pub nose_shape: Option<String>,       
    pub lip_shape: Option<String>,        
    
    // ③ 辨识标记层（最强锚点，必填）
    pub unique_marks: Vec<String>,        // ["small mole 2cm below left eye"]
    
    // ④ 色彩锚点层
    pub color_anchors: Option<ColorAnchors>,
    
    // ⑤ 皮肤纹理层
    pub skin_texture: Option<String>,
    
    // ⑥ 发型锚点层
    pub hair_style: Option<String>,
    pub hairline_details: Option<String>,
}

pub struct ColorAnchors {
    pub iris: Option<String>,     // "#3D2314"
    pub hair: Option<String>,     // "#1A1A1A"
    pub skin: Option<String>,     // "#E8C4A0"
    pub lips: Option<String>,     // "#C4727E"
}

// ===== 角色视图（生成的多角度图片） =====
pub struct CharacterView {
    pub view_type: ViewType,      // Front | Side | Back | ThreeQuarter
    pub image_url: String,
    pub generated_at: i64,
}

pub enum ViewType {
    Front,
    Side,
    Back,
    ThreeQuarter,
}

// ===== 角色变体（衣柜系统） =====
pub struct CharacterVariation {
    pub id: String,
    pub name: String,                         // "日常装", "战斗装"
    pub visual_prompt: String,
    pub visual_prompt_zh: Option<String>,
    pub reference_image: Option<String>,
    pub clothing_reference_images: Vec<String>,
    pub generated_at: Option<i64>,
    // 阶段变体
    pub is_stage_variation: bool,
    pub episode_range: Option<(u32, u32)>,
    pub age_description: Option<String>,
    pub stage_description: Option<String>,
}

// ===== 负面提示词 =====
pub struct CharacterNegativePrompt {
    pub avoid: Vec<String>,             // ["glasses", "beard"]
    pub style_exclusions: Vec<String>,  // ["photorealistic"]
}

// ===== 角色库主结构 =====
pub struct Character {
    pub id: String,
    pub name: String,
    
    // 基本属性
    pub gender: Option<String>,
    pub age: Option<String>,
    pub personality: Option<String>,
    pub role: Option<String>,            // 身份/背景
    pub traits: Option<String>,
    pub skills: Option<String>,
    pub key_actions: Option<String>,
    pub appearance: Option<String>,
    pub relationships: Option<String>,
    pub tags: Vec<String>,
    pub notes: Option<String>,
    
    // 视觉描述
    pub visual_prompt_en: Option<String>,
    pub visual_prompt_zh: Option<String>,
    pub description: Option<String>,
    
    // 6 层锚点
    pub identity_anchors: Option<CharacterIdentityAnchors>,
    pub negative_prompt: Option<CharacterNegativePrompt>,
    
    // 关联数据
    pub views: Vec<CharacterView>,
    pub variations: Vec<CharacterVariation>,
    pub thumbnail_url: Option<String>,
    pub reference_images: Vec<String>,
    pub style_id: Option<String>,
    
    // 组织
    pub folder_id: Option<String>,
    pub project_id: Option<String>,
    pub status: CharacterStatus,           // Draft | Linked
    pub linked_episode_id: Option<String>,
    
    // 时间戳
    pub created_at: i64,
    pub updated_at: i64,
}

pub enum CharacterStatus {
    Draft,
    Linked,
}

// ===== 角色文件夹 =====
pub struct CharacterFolder {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub project_id: Option<String>,
    pub is_auto_created: bool,
    pub created_at: i64,
}

// ===== AI 角色校准结果 =====
pub struct CharacterCalibrationResult {
    pub characters: Vec<CalibratedCharacter>,
    pub filtered_words: Vec<String>,
    pub filtered_characters: Vec<FilteredCharacterRecord>,
    pub merge_records: Vec<MergeRecord>,
    pub analysis_notes: String,
}

pub struct CalibratedCharacter {
    pub id: String,
    pub name: String,
    pub importance: Importance,      // Protagonist | Supporting | Minor | Extra
    pub episode_range: Option<(u32, u32)>,
    pub appearance_count: u32,
    pub role: Option<String>,
    pub age: Option<String>,
    pub gender: Option<String>,
    pub relationships: Option<String>,
    pub name_variants: Vec<String>,
    pub visual_prompt_en: Option<String>,
    pub visual_prompt_zh: Option<String>,
    pub facial_features: Option<String>,
    pub unique_marks: Option<String>,
    pub clothing_style: Option<String>,
    pub identity_anchors: Option<CharacterIdentityAnchors>,
    pub negative_prompt: Option<CharacterNegativePrompt>,
}

pub enum Importance {
    Protagonist,
    Supporting,
    Minor,
    Extra,
}

pub struct FilteredCharacterRecord {
    pub name: String,
    pub reason: String,
}

pub struct MergeRecord {
    pub from: Vec<String>,    // ["王总", "投资人王总"]
    pub to: String,           // "王总"
    pub reason: String,
}
```

**任务清单：**
- [ ] 新建 `crates/artait-model/src/character.rs`
- [ ] 在 `crates/artait-model/src/lib.rs` 中 `pub mod character;`
- [ ] 所有 struct 派生 `Serialize, Deserialize, Clone, Debug`
- [ ] 为需要 UI 展示的 struct 添加 `Default` 实现

---

### 1.2 Provider 能力层 (`artait-provider` / `artait-providers`)

#### 1.2.1 审查现有 `CharacterGenerator` trait

ArtStudio 已有 `CharacterGenerator` trait（定义在 `artait-provider/src/lib.rs`），需要审查是否满足需求：

```rust
// 现有 trait（需确认）
pub trait CharacterGenerator {
    async fn generate_character(&self, config: CharacterGenConfig) -> Result<...>;
}
```

**扩展需求：**
- 支持多视角生成（front/side/back/three-quarter）
- 支持角色表生成（character sheet with expressions/poses/proportions）
- 支持服装变体生成（wardrobe variation，含服装参考图融合）
- 支持 6 层锚点注入到 prompt
- 返回值需包含 preview URL 列表

#### 1.2.2 角色 Prompt 构建器

在 `artait-service` 中新建 `character_prompt.rs`，核心功能：

```
输入：Character + 生成配置
输出：(positive_prompt, negative_prompt)

处理流程：
1. 选择主视觉提示词（按语言偏好: zh / en / zh+en）
2. 构建锚点提示词：
   - 有参考图 → 只用 ③ unique_marks + ④ color_anchors
   - 无参考图 → 完整的 ①-⑥ 六层锁定
3. 融合基础描述 + 锚点 + 时代服装提示词
4. 组装最终 prompt：
   [professional character sheet for "name"]
   + [description]
   + [sheet elements: three-view/expressions/proportions/poses]
   + [white background]
   + [style tokens]
   + [quality modifiers]
5. 区分 realistic/anime 分支
6. 构建 negative prompt：
   base (blurry/low quality/watermark)
   + character-specific avoids
   + style exclusions
```

**任务清单：**
- [ ] `crates/artait-service/src/character_prompt.rs` — prompt 构建器
- [ ] 添加 `build_anchor_prompt()` 函数
- [ ] 添加 `build_character_sheet_prompt()` 函数
- [ ] 添加 `extract_era_fashion_prompt()` 函数
- [ ] 单元测试

---

### 1.3 Service 层 (`artait-service`)

#### 1.3.1 角色存储服务 `character_store.rs`

```rust
// 文件系统持久化：portable_data_dir() / characters / {project_id}.json
pub struct CharacterStore {
    // 内存缓存
    characters: HashMap<String, Character>,
    folders: Vec<CharacterFolder>,
    // 文件路径
    store_path: PathBuf,
}

impl CharacterStore {
    // CRUD
    pub fn load(project_id: &str) -> Result<Self>;
    pub fn save(&self) -> Result<()>;
    
    pub fn add_character(&mut self, character: Character) -> Result<()>;
    pub fn update_character(&mut self, id: &str, update: CharacterUpdate) -> Result<()>;
    pub fn delete_character(&mut self, id: &str) -> Result<()>;
    pub fn get_character(&self, id: &str) -> Option<&Character>;
    
    // 文件夹
    pub fn add_folder(&mut self, folder: CharacterFolder) -> Result<()>;
    pub fn rename_folder(&mut self, id: &str, name: &str) -> Result<()>;
    pub fn delete_folder(&mut self, id: &str) -> Result<()>;
    pub fn get_folder_characters(&self, folder_id: Option<&str>, project_id: &str) -> Vec<&Character>;
    
    // 查询
    pub fn search(&self, query: &str) -> Vec<&Character>;
    pub fn filter_by_project(&self, project_id: &str) -> Vec<&Character>;
    pub fn filter_by_episode(&self, episode_id: Option<&str>) -> Vec<&Character>;
    
    // 视图/变体
    pub fn add_view(&mut self, char_id: &str, view: CharacterView) -> Result<()>;
    pub fn add_variation(&mut self, char_id: &str, var: CharacterVariation) -> Result<()>;
    pub fn update_variation(&mut self, char_id: &str, var_id: &str, update: VariationUpdate) -> Result<()>;
    pub fn delete_variation(&mut self, char_id: &str, var_id: &str) -> Result<()>;
}
```

#### 1.3.2 角色校准服务 `character_calibrator.rs`

从 moyin-creator 的 `character-calibrator.ts` 迁移 4 步流水线：

```
Step 1: extract_characters_from_script(script) → Vec<String>
        扫描所有场的 characters + dialogue.character → 去重集合

Step 2: collect_character_stats(script) → HashMap<name, CharacterStats>
        统计出场次数、对白次数、集数范围、对白采样(前3条)

Step 3: calibrate_characters(char_names, stats, strictness) → CharacterCalibrationResult
        - 按优先级排序：具名角色(+1000) > 无名角色(按出场数排序)
        - 配角群演 -1000
        - 上限 150 个角色发给 AI
        - 3 种严格度：strict（过滤群演）/ normal（保留具名）/ loose（全部保留）
        - 分批处理（processBatched）
        - AI 返回 calibrated characters + filtered + merge records
        - 失败时优雅降级：返回基于统计的结果

Step 4: enrich_characters_with_visual_prompts(calibrated) → Vec<CalibratedCharacter>
        - 逐个为 Protagonist/Supporting 角色调用 AI
        - 生成完整 6 层 identityAnchors + negativePrompt
        - 包含时代感知的服装指导（唐/宋/明/清/民国/现代）
        - 合并回 CalibratedCharacter
```

#### 1.3.3 角色生成服务 `character_generation.rs`

```
流程：
1. 接收 Character + GenerationConfig
2. 调用 character_prompt::build_character_sheet_prompt() 构建 prompt
3. 选择 provider（通过 CharacterGenerator trait 或 ImageGenerator）
4. 调用 provider 生成图片
5. 保存图片到本地 → 生成 thumbnail
6. 更新 Character.views
7. 发布 TaskEvent 通知 UI
```

**任务清单：**
- [ ] `crates/artait-service/src/character_store.rs` — 持久化
- [ ] `crates/artait-service/src/character_calibrator.rs` — AI 校准流水线
- [ ] `crates/artait-service/src/character_generation.rs` — 生成流程
- [ ] 在 `crates/artait-service/src/lib.rs` 注册模块

---

### 1.4 Slint UI 层 (`artait-app`)

#### 1.4.1 新增 Slint 页面

```
ui/pages/
├── character-library.slint      角色库主页（3 栏布局）
└── components/
    ├── character-card.slint     角色卡片（缩略图 + 名称 + 视图数）
    ├── character-detail.slint   角色详情面板（信息/锚点/视图/变体）
    ├── character-form.slint     角色创建/编辑表单（含 6 层锚点编辑）
    ├── character-gallery.slint  角色画廊（文件夹导航 + 网格/列表视图）
    ├── anchor-editor.slint      6 层锚点编辑器（折叠面板）
    ├── wardrobe-panel.slint     衣柜面板（变体列表 + 预设 + 服装参考图）
    └── character-generate-bar.slint  生成控制栏
```

#### 1.4.2 角色库主页布局

```
┌─────────────────────────────────────────────────────┐
│  角色库                     [+新建角色] [搜索...]    │
├──────────┬──────────────────────────┬───────────────┤
│          │                          │               │
│  文件夹   │   角色网格（3 列）        │  角色详情      │
│  树      │                          │  · 缩略图     │
│          │  [卡片] [卡片] [卡片]     │  · 基本信息   │
│  · 全部   │  [卡片] [卡片] [卡片]     │  · 6层锚点    │
│  · 项目A  │  [卡片] [卡片] [卡片]     │  · 视图列表   │
│  · 项目B  │                          │  · 变体列表   │
│          │                          │  · 操作按钮   │
│          │                          │               │
├──────────┴──────────────────────────┴───────────────┤
│  状态栏：已选 X 个角色  |  生成进度                  │
└─────────────────────────────────────────────────────┘
```

#### 1.4.3 6 层锚点编辑器

以折叠面板（Accordion）形式呈现，每层独立展开/折叠：

```
▼ ① 骨相结构 (Bone Structure)
   脸型:     [oval ▾]    下颌: [sharp angular ▾]    颧骨: [high prominent ▾]

▶ ② 五官特征 (Facial Features)                          [+]
▶ ③ 辨识标记 (Distinctive Marks) 🔴最强锚点              [+]
▶ ④ 色彩锚点 (Color Anchors)                             [+]
▶ ⑤ 皮肤纹理 (Skin Texture)                              [+]
▶ ⑥ 发型锚点 (Hairstyle)                                 [+]
```

每层预置下拉选项（来自 moyin-creator 的英文选项），也可自由输入。

#### 1.4.4 衣柜面板

```
┌──────────────────────────────┐
│  衣柜 · 角色名               │
│                              │
│  预设: [日常装] [正装] [战斗装]│
│        [睡衣] [运动装] [受伤] │
│        [雨天] [冬装]          │
│                              │
│  ▼ 变体列表                  │
│  ┌─────────────────────────┐ │
│  │ 🖼 日常装    [✓已生成]    │ │
│  │ 🖼 战斗装    [生成中...]  │ │
│  │ 🖼 青年版    [待生成]     │ │
│  └─────────────────────────┘ │
│                              │
│  [+ 添加变体]                 │
│                              │
│  服装参考图: [📎上传] (最多3张)│
│  ┌───┐ ┌───┐ ┌───┐          │
│  │   │ │   │ │ + │          │
│  └───┘ └───┘ └───┘          │
│                              │
│  [生成选中] [批量生成全部]     │
└──────────────────────────────┘
```

#### 1.4.5 AppState 扩展

在 `ui/app-state.slint` 中添加：

```slint
// 角色库状态
property <[CharacterModel]> character-list;
property <[CharacterFolderModel]> character-folders;
property <int> selected-character-index: -1;
property <string> character-search-query;
property <string> current-character-folder-id;

// 角色详情
property <CharacterModel> editing-character;
property <bool> character-detail-visible;

// 衣柜
property <[CharacterVariationModel]> wardrobe-variations;
property <bool> wardrobe-modal-open;

// 锚点编辑器
property <bool> anchor-editor-expanded-1;  // 骨相
property <bool> anchor-editor-expanded-2;  // 五官
// ... 共 6 层

// 回调
callback character-create(CharacterModel);
callback character-update(string id, CharacterModel);
callback character-delete(string id);
callback character-generate(string id, string view-type);  // front/side/back/three-quarter
callback character-generate-variation(string char-id, string var-id);
callback character-calibrate-from-script();
callback character-search(string query);
callback character-select-folder(string folder-id);
```

#### 1.4.6 路由注册

在 `ui/main.slint` 的 `AppShell` 中注册路由：

```slint
if route == "character-library" : CharacterLibraryPage {
    // ...
}
```

**任务清单：**
- [ ] 新建 `ui/pages/character-library.slint`
- [ ] 新建 `ui/components/character-card.slint`
- [ ] 新建 `ui/components/character-detail.slint`
- [ ] 新建 `ui/components/anchor-editor.slint`
- [ ] 新建 `ui/components/wardrobe-panel.slint`
- [ ] 在 `main.slint` 注册路由
- [ ] 在 `TabBar` 中添加"角色库"导航项
- [ ] 在 `app-state.slint` 添加角色相关属性/回调
- [ ] 在 `main.rs` 或 `callbacks/` 注册回调 handler（转发到 service）

---

### 1.5 集成到流水线

角色系统完成后，需要与后续模块打通：

```
剧本 (Phase 2) → 角色提取 ──→ 角色库
                              │
                              ├→ 角色校准（AI 补全 6 层锚点）
                              ├→ 角色生图（多视角/角色表）
                              ├→ 服装变体（衣柜）
                              │
场景 (Phase 3) ← 引用角色 ────┘
导演 (Phase 4) ← 引用角色 ────┘
S-Class (Phase 5) ← 引用角色 ─┘
```

---

## Phase 2-5 概要（后续展开）

### Phase 2：剧本解析引擎

- 新建 `artait-model` 中的 `Script`/`Episode`/`Scene`/`Shot`/`Dialogue` 类型
- 新建 `script_parser.rs`（从 markdown/纯文本 解析结构化剧本）
- 新建 `script_normalizer.rs`（格式标准化）
- 扩展 `animation_script` 页面的 Slint UI（当前只是 markdown 预览 + AI 生成后按 `## 镜头` 分割）
- 支持导入、分集、角色/场景自动提取

### Phase 3：场景系统

- 新建 `artait-model` 中的 `Scene`/`SceneViewpoint` 类型
- 新建 `scene_store.rs`、`scene_calibrator.rs`、`scene_prompt_generator.rs`
- 新建 `ui/pages/scene-library.slint`
- 多视角联合生成、场景→视觉提示词自动转换

### Phase 4：导演/分镜系统

- 新建 `artait-model` 中的 `Shot`/`Storyboard`/`CinematographyProfile` 类型
- 新建 `director_store.rs`、`shot_generator.rs`
- 大幅重写 `ui/pages/storyboard.slint`（当前只是基础 markdown 预览 + 单图生成）
- 加入电影级参数：景别/机位/运动/角度
- 批量生成、分镜时间轴

### Phase 5：Seedance 2.0 (S-Class)

- 扩展 `VideoGenerator` trait 的实际实现
- 新建 `sclass_service.rs`：多模态引用解析、3 层 prompt 融合、宫格拼接
- 新建 `ui/pages/sclass.slint`
- 约束自动校验（≤9 图 + ≤3 视频 + ≤3 音频）

---

## 迁移原则

遵循 AGENTS.md 中的重构策略：

1. **数据模型先行** — 每个 Phase 先在 `artait-model` 中定义好类型，再写 service，最后写 UI
2. **Service 层承载业务逻辑** — Slint callback handler 只做参数转发
3. **渐进式，不一次全搬** — 每个 Phase 是一个完整的功能闭环，完成后即可使用
4. **不破坏现有功能** — 新增页面通过路由注册，不删除已有页面
5. **先功能闭环，再视觉细节** — 角色系统先走通「创建→编辑→生成→保存」完整流程，再打磨 UI
6. **文件存储优于数据库** — 角色数据用 JSON 文件存储（与现有 task_history 一致），不引入 IndexedDB/SQLite
