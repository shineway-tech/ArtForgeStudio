//! 角色库回调 handler。
//!
//! 处理角色库页面的用户交互：搜索、选择、创建、删除、生成。

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::callbacks::CbCtx;
use crate::ui::{AppShell, AppState, CharacterCardItem};

/// 注册所有角色库回调。
pub(crate) fn init(ctx: &CbCtx, _app: &AppShell) {
    let state = _app.global::<AppState>();
    let app_weak = ctx.app.clone();
    let char_store = ctx.character_store.clone();

    // ── 搜索 ────────────────────────────────────────────
    {
        let _app_weak = app_weak.clone();
        let _char_store = char_store.clone();
        state.on_character_library_search_changed(move |query| {
            let q = query.to_string();
            if let Some(app) = _app_weak.upgrade() {
                let s = app.global::<AppState>();
                let store = _char_store.borrow();
                let results = store.search(&q);
                let items: Vec<CharacterCardItem> =
                    results.iter().map(|c| character_to_card(c)).collect();
                s.set_character_library_items(ModelRc::new(VecModel::from(items)));
            }
        });
    }

    // ── 选择角色：填充详情 + 编辑表单 ────────────────
    {
        let _app_weak = app_weak.clone();
        let _char_store = char_store.clone();
        state.on_character_library_select(move |id| {
            let id_str = id.to_string();
            if let Some(app) = _app_weak.upgrade() {
                let s = app.global::<AppState>();
                let store = _char_store.borrow();
                s.set_character_library_selected_id(id_str.clone().into());
                s.set_character_library_detail_visible(true);
                s.set_character_library_editing(false);
                s.set_current_page("character_detail".into());

                if let Some(c) = store.get_character(&id_str) {
                    // 详情显示
                    s.set_character_library_detail_name(c.name.clone().into());
                    s.set_character_library_detail_status(c.status.display_name().into());
                    s.set_character_library_detail_desc(
                        c.description.clone().unwrap_or_default().into(),
                    );
                    s.set_character_library_detail_tags(c.tags.join(", ").into());
                    s.set_character_library_detail_view_count(c.view_count() as i32);
                    s.set_character_library_detail_variation_count(c.variation_count() as i32);

                    // 编辑表单
                    s.set_char_edit_name(c.name.clone().into());
                    s.set_char_edit_gender(c.gender.clone().unwrap_or_default().into());
                    s.set_char_edit_age(c.age.clone().unwrap_or_default().into());
                    s.set_char_edit_role(c.role.clone().unwrap_or_default().into());
                    s.set_char_edit_personality(c.personality.clone().unwrap_or_default().into());
                    s.set_char_edit_appearance(c.appearance.clone().unwrap_or_default().into());
                    s.set_char_edit_visual_en(
                        c.visual_prompt_en.clone().unwrap_or_default().into(),
                    );
                    s.set_char_edit_visual_zh(
                        c.visual_prompt_zh.clone().unwrap_or_default().into(),
                    );
                    s.set_char_edit_tags(c.tags.join(", ").into());

                    // 6层锚点
                    if let Some(ref a) = c.identity_anchors {
                        s.set_anchor_1_face(a.face_shape.clone().unwrap_or_default().into());
                        s.set_anchor_1_jaw(a.jawline.clone().unwrap_or_default().into());
                        s.set_anchor_1_cheek(a.cheekbones.clone().unwrap_or_default().into());
                        s.set_anchor_2_eye(a.eye_shape.clone().unwrap_or_default().into());
                        s.set_anchor_2_eye_detail(a.eye_details.clone().unwrap_or_default().into());
                        s.set_anchor_2_nose(a.nose_shape.clone().unwrap_or_default().into());
                        s.set_anchor_2_lip(a.lip_shape.clone().unwrap_or_default().into());
                        s.set_anchor_3_marks(a.unique_marks.join(", ").into());
                        if let Some(ref ca) = a.color_anchors {
                            s.set_anchor_4_iris(ca.iris.clone().unwrap_or_default().into());
                            s.set_anchor_4_hair(ca.hair.clone().unwrap_or_default().into());
                            s.set_anchor_4_skin(ca.skin.clone().unwrap_or_default().into());
                            s.set_anchor_4_lips(ca.lips.clone().unwrap_or_default().into());
                        }
                        s.set_anchor_5_texture(a.skin_texture.clone().unwrap_or_default().into());
                        s.set_anchor_6_style(a.hair_style.clone().unwrap_or_default().into());
                        s.set_anchor_6_hairline(
                            a.hairline_details.clone().unwrap_or_default().into(),
                        );
                    }
                }
            }
        });
    }

    // ── 保存编辑 ────────────────────────────────────────
    {
        let _app_weak = app_weak.clone();
        let _char_store = char_store.clone();
        state.on_character_library_save(move || {
            if let Some(app) = _app_weak.upgrade() {
                let s = app.global::<AppState>();
                let id = s.get_character_library_selected_id().to_string();
                if id.is_empty() {
                    return;
                }

                let mut store = _char_store.borrow_mut();
                let _ = store.update_character(&id, |c| {
                    let name = s.get_char_edit_name().to_string();
                    let trimmed_name = name.trim();
                    if !trimmed_name.is_empty() {
                        c.name = trimmed_name.to_string();
                    }
                    c.gender = opt(s.get_char_edit_gender().to_string());
                    c.age = opt(s.get_char_edit_age().to_string());
                    c.role = opt(s.get_char_edit_role().to_string());
                    c.personality = opt(s.get_char_edit_personality().to_string());
                    c.appearance = opt(s.get_char_edit_appearance().to_string());
                    c.visual_prompt_en = opt(s.get_char_edit_visual_en().to_string());
                    c.visual_prompt_zh = opt(s.get_char_edit_visual_zh().to_string());
                    c.tags = split_tags(&s.get_char_edit_tags().to_string());

                    // 构建 6 层锚点
                    c.identity_anchors = Some(artait_model::CharacterIdentityAnchors {
                        face_shape: opt(s.get_anchor_1_face().to_string()),
                        jawline: opt(s.get_anchor_1_jaw().to_string()),
                        cheekbones: opt(s.get_anchor_1_cheek().to_string()),
                        eye_shape: opt(s.get_anchor_2_eye().to_string()),
                        eye_details: opt(s.get_anchor_2_eye_detail().to_string()),
                        nose_shape: opt(s.get_anchor_2_nose().to_string()),
                        lip_shape: opt(s.get_anchor_2_lip().to_string()),
                        unique_marks: split_marks(&s.get_anchor_3_marks().to_string()),
                        color_anchors: Some(artait_model::ColorAnchors {
                            iris: opt(s.get_anchor_4_iris().to_string()),
                            hair: opt(s.get_anchor_4_hair().to_string()),
                            skin: opt(s.get_anchor_4_skin().to_string()),
                            lips: opt(s.get_anchor_4_lips().to_string()),
                        }),
                        skin_texture: opt(s.get_anchor_5_texture().to_string()),
                        hair_style: opt(s.get_anchor_6_style().to_string()),
                        hairline_details: opt(s.get_anchor_6_hairline().to_string()),
                    });
                });

                store.flush();
                refresh_character_list(&s, store.all_characters());
                s.set_character_library_editing(false);

                // 刷新详情
                if let Some(c) = store.get_character(&id) {
                    s.set_character_library_detail_name(c.name.clone().into());
                    s.set_character_library_detail_desc(
                        c.description.clone().unwrap_or_default().into(),
                    );
                    s.set_character_library_detail_tags(c.tags.join(", ").into());
                    s.set_character_library_detail_view_count(c.view_count() as i32);
                    s.set_character_library_detail_variation_count(c.variation_count() as i32);
                }
            }
        });
    }

    // ── 创建角色 ────────────────────────────────────────
    {
        let _app_weak = app_weak.clone();
        let _char_store = char_store.clone();
        state.on_character_library_create(move || {
            if let Some(app) = _app_weak.upgrade() {
                let s = app.global::<AppState>();
                let mut store = _char_store.borrow_mut();
                let id = format!("char-{}", chrono::Utc::now().timestamp_millis());
                let c = artait_model::Character::new(id, "新角色".into());
                if let Ok(id) = store.create_character(c) {
                    store.flush();
                    refresh_character_list(&s, store.all_characters());
                    s.set_character_library_selected_id(id.clone().into());
                    s.set_character_library_detail_visible(true);
                    s.set_character_library_detail_name("新角色".into());
                    s.set_character_library_detail_status("草稿".into());
                    s.set_character_library_detail_desc("".into());
                    s.set_character_library_detail_tags("".into());
                    s.set_character_library_detail_view_count(0);
                    s.set_character_library_detail_variation_count(0);
                    s.set_char_edit_name("新角色".into());
                    s.set_char_edit_gender("".into());
                    s.set_char_edit_age("".into());
                    s.set_char_edit_role("".into());
                    s.set_char_edit_personality("".into());
                    s.set_char_edit_appearance("".into());
                    s.set_char_edit_visual_en("".into());
                    s.set_char_edit_visual_zh("".into());
                    s.set_char_edit_tags("".into());
                    clear_anchor_fields(&s);
                    s.set_character_library_editing(true);
                    s.set_current_page("character_detail".into());
                }
            }
        });
    }

    // ── 删除角色 ────────────────────────────────────────
    {
        let _app_weak = app_weak.clone();
        let _char_store = char_store.clone();
        state.on_character_library_delete(move |id| {
            let id_str = id.to_string();
            if let Some(app) = _app_weak.upgrade() {
                let s = app.global::<AppState>();
                let mut store = _char_store.borrow_mut();
                if store.delete_character(&id_str).is_ok() {
                    store.flush();
                    refresh_character_list(&s, store.all_characters());
                    s.set_character_library_detail_visible(false);
                    s.set_character_library_selected_id("".into());
                    s.set_character_library_editing(false);
                    s.set_current_page("character_library".into());
                }
            }
        });
    }

    // ── 生成角色图：构建 prompt 并调用现有生图流程 ─────
    {
        let _app_weak = app_weak.clone();
        let _char_store = char_store.clone();
        state.on_character_library_generate(move |id| {
            let id_str = id.to_string();
            if let Some(app) = _app_weak.upgrade() {
                let s = app.global::<AppState>();
                let store = _char_store.borrow();
                if let Some(c) = store.get_character(&id_str) {
                    let prompt = c.primary_visual_prompt().unwrap_or(&c.name).to_string();
                    // 导航到角色创作页面并填入 prompt
                    s.set_ws_prompt(prompt.into());
                    s.set_ws_aspect("1:1".into());
                    s.set_ws_quality("2K".into());
                    s.set_ws_count(1);
                    s.set_current_page("character".into());
                }
            }
        });
    }
}

fn character_to_card(c: &artait_model::Character) -> CharacterCardItem {
    CharacterCardItem {
        id: c.id.clone().into(),
        name: c.name.clone().into(),
        status: c.status.display_name().into(),
        view_count: c.view_count() as i32,
        variation_count: c.variation_count() as i32,
        description: c.description.clone().unwrap_or_default().into(),
        tags: c.tags.join(", ").into(),
        has_thumb: c.thumbnail_url.is_some(),
        thumb: slint::Image::default(),
    }
}

#[allow(dead_code)]
pub(crate) fn refresh_character_list(state: &AppState, characters: &[artait_model::Character]) {
    let items: Vec<CharacterCardItem> = characters.iter().map(character_to_card).collect();
    state.set_character_library_items(ModelRc::new(VecModel::from(items)));
}

fn opt(s: String) -> Option<String> {
    let t = s.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn split_tags(s: &str) -> Vec<String> {
    s.split(&[',', '，', ' ', '#', '、'][..])
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn split_marks(s: &str) -> Vec<String> {
    s.split(&[',', '，', '、'][..])
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn clear_anchor_fields(s: &AppState) {
    s.set_anchor_1_face("".into());
    s.set_anchor_1_jaw("".into());
    s.set_anchor_1_cheek("".into());
    s.set_anchor_2_eye("".into());
    s.set_anchor_2_eye_detail("".into());
    s.set_anchor_2_nose("".into());
    s.set_anchor_2_lip("".into());
    s.set_anchor_3_marks("".into());
    s.set_anchor_4_iris("".into());
    s.set_anchor_4_hair("".into());
    s.set_anchor_4_skin("".into());
    s.set_anchor_4_lips("".into());
    s.set_anchor_5_texture("".into());
    s.set_anchor_6_style("".into());
    s.set_anchor_6_hairline("".into());
}
