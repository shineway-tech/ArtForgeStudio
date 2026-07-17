use super::*;

pub(super) fn load_showcase_images(app: &AppWindow) {
    let state = app.global::<AppState>();
    if let Some(path) = asset_path("showcase/character.png") {
        if let Ok(image) = load_image(&path) {
            state.set_showcase_character(image);
        }
    }
    if let Some(path) = asset_path("showcase/scene.png") {
        if let Ok(image) = load_image(&path) {
            state.set_showcase_scene(image);
        }
    }
    if let Some(path) = asset_path("showcase/ui.png") {
        if let Ok(image) = load_image(&path) {
            state.set_showcase_ui(image);
        }
    }
    if let Some(path) = asset_path("showcase/vfx.png") {
        if let Ok(image) = load_image(&path) {
            state.set_showcase_vfx(image);
        }
    }
}

pub(super) fn asset_path(relative: &str) -> Option<PathBuf> {
    resource_base_dirs()
        .into_iter()
        .map(|base| base.join("assets").join(relative))
        .find(|path| path.exists())
}

pub(super) fn seed_inspiration(app: &AppWindow, store: &Rc<RefCell<Store>>) -> Result<()> {
    let dirs = inspiration_dirs();
    let mut items = Vec::new();
    let mut seen_files = BTreeSet::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let mut paths = fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_string()
        });
        for path in paths {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                continue;
            }
            let file_key = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_ascii_lowercase())
                .unwrap_or_else(|| path.display().to_string().to_ascii_lowercase());
            if !seen_files.insert(file_key) {
                continue;
            }
            if let Ok(image) = load_image(&path) {
                let index = items.len() + 1;
                let (title, category, kind) = inspiration_meta(index);
                let (width, height) = image::image_dimensions(&path)
                    .map(|(w, h)| (w as i32, h as i32))
                    .unwrap_or((1254, 1254));
                let ratio = ratio_from_actual_dimensions(width, height);
                let quality = quality_from_actual_dimensions(width, height);
                items.push(AssetData {
                    id: format!("inspiration-{index}"),
                    conversation_id: String::new(),
                    title: title.to_string(),
                    category: category.to_string(),
                    kind: kind.to_string(),
                    time: "官方示例".to_string(),
                    prompt: inspiration_prompt(index, title, &ratio),
                    ratio,
                    quality,
                    model: "官方示例".to_string(),
                    width,
                    height,
                    image,
                    source_path: path.display().to_string(),
                    cutout_done: false,
                    remove_black_done: false,
                    upscale_done: false,
                });
            }
        }
    }
    store.borrow_mut().inspiration = items;
    push_all(app, &store.borrow());
    Ok(())
}

pub(super) fn inspiration_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for base in resource_base_dirs() {
        push_unique_path(&mut dirs, base.join("assets").join("sucai"));
    }
    dirs
}

pub(super) fn inspiration_meta(index: usize) -> (&'static str, &'static str, &'static str) {
    [
        ("东方巨像", "scene", "game"),
        ("游戏 UI 套件", "ui", "game"),
        ("奇幻角色图标", "character", "game"),
        ("村庄场景地图", "scene", "game"),
        ("角色设计图", "character", "game"),
        ("迷你角色图标", "character", "game"),
        ("沙漠场景", "scene", "game"),
        ("战略游戏画面", "scene", "film"),
        ("Q 版图标 UI", "ui", "game"),
        ("RPG 贴图集", "ui", "game"),
        ("技能特效", "effect", "game"),
        ("奇幻角色集", "character", "game"),
        ("怪物图标", "character", "game"),
        ("装备栏 UI", "ui", "game"),
        ("像素 BOSS 战", "scene", "film"),
        ("魔法森林", "scene", "game"),
        ("城市场景", "scene", "film"),
        ("丧尸角色", "character", "game"),
        ("像素魔女", "character", "game"),
        ("重甲骑士", "character", "game"),
        ("日式 RPG UI", "ui", "game"),
        ("复古游戏 UI", "ui", "game"),
        ("特效设计", "effect", "game"),
        ("游戏 CG 角色", "character", "game"),
    ]
    .get(index.saturating_sub(1))
    .copied()
    .unwrap_or(("官方示例", "other", "game"))
}

pub(super) fn inspiration_prompt(index: usize, title: &str, ratio: &str) -> String {
    match index {
        1 => "航拍，俯视，巨物恐惧症，背景呈现庞大的东方玄幻风格建筑。超大型力士像伸手向人类挥舞，游戏 CG 风格，全景镜头，全身动态动作，景深效果，倾斜失衡构图，巨型比例，C4D建模，Blender制作，虚幻引擎，Octane渲染，全局光照，光线追踪反射，屏幕空间环境光遮蔽，着色器，快速近似抗锯齿，电脑生成图像，实时光线追踪，视觉特效，4K画质，最佳品质，超精细，超写实，幽暗奇幻，暗黑风格，粗粝质感，微妙色调。".to_string(),
        2 => "生成一组游戏风格UI素材，包含上百个不同样式的按钮、面板、进度条，采用日式 RPG风格，不应受到阴影或光线的影响。大师，杰作。背景是白色的".to_string(),
        3 | 6 | 10 | 12 | 13 => "一套包含各种奇幻rpg迷你角色图标的贴图集，包括埃及妖精、兜帽法师、骑士和怪物等，采用可爱的q版风格，线条粗犷利落，色彩鲜艳，平面2d游戏画风，符合手游美学，细节丰富，纯深色灰背景，风格类似《王国保卫战》".to_string(),
        4 => "小村庄游戏场景地图，带顶视图的RPG Maker 风格地图，Chrono Trigger 风格，画面风格是 90 年代复古像素风，风格参考《塞尔达传说》复古像素游戏，轮廓用粗黑像素线条勾勒，色彩块面分明，色调以高饱和的复古游戏色绿和黄为主，红、蓝等为辅助，明亮的，包含：有两栋房子，几个村子，玉米地，喷泉，农场，养鸡场，聊天的村民，".to_string(),
        5 => "人设设计超详细图，背景白色，展示了每件作品的复杂设计过程，图纸包括对角色各部分大量尺寸和解释性文本注释，英文文字的设计说明,不同角度的零散缩略图增加了场景的深度，每个细节都有展示，极具想象力，丰富联想，水彩融合水墨，极具设计感服装，人设，超细节，吕布化身为巨大半透明由狂暴能量构成的深红色武魂真身，手持长枪呈战斗姿态，深红色煞气如火焰般燃烧升腾暗红色能量闪电在周身噼啪作响。暗黑美学，国风玄幻，CG艺术,特写,极具动态和攻击性，最高画质，压迫感强,超高细节。长卷构图，艺术设计。".to_string(),
        7 => "卡通风格的插图，一个游戏场景，一个沙漠场景，地面水平线在画面自上而下的十分之一处，画面中间是广阔的浅黄色的沙地，沙地占画面的十分之九，小小的绿色的仙人掌与小红色的花朵在左边，远处的废墟由红褐色的岩石和白色的岩石柱子还有一棵棕榈树与棕色的树干组成，色调不要太明亮，和平和安静的气氛，2D游戏资产".to_string(),
        8 => "生成即时战略游戏的游戏画面。".to_string(),
        9 => "2D游戏，游戏图标ui设计，Q版卡通游戏UI，等距视角，手绘治愈Q 版萌系、柔和暖色调，模拟经营游戏 UI、温馨田园风，日式治愈风格，手绘萌系质感风格，生成15个萌系游戏图标，15个一组在同一个画面中，生成2D 卡通 / 动漫游戏风格的图标，充满装饰性元素，可爱童话风格，生成不同组合的萌系厨房卫生间卧室空间需要有：（卧室、卫生间，厨房，书房，电竞房，健身房，客厅）；高级感，大师杰作，视觉上简洁明快，“平滑的卡通渲染质感”，色彩明亮清新，刻画细节，过渡柔和，矢量插画，细腻的写实光照，柔和的光影完美呈现，光影过渡自然，勾勒清晰的线稿，整体氛围轻松愉悦，层次丰富，塑造扁平，“低饱和对比色”，纯灰色背景，不要出现汉字".to_string(),
        11 => "游戏特效，没有人物，没有主角，灰色背景，有层次的，俯视角，平面，多个不一样的设计，素材，排列整齐，火焰".to_string(),
        14 => "游戏传奇界面的一组游戏风格UI装备栏合集，采用热血传奇游戏风格，火龙，精美 的细节，边缘有图案和装饰元素，不应受到阴影或光线的影响。大师杰作，黑色背景".to_string(),
        15 => "16-bit像素勇者大战复古游戏BOSS，怀旧像素颗粒，有限色板抖动，[RPG/横版动作]场景选择".to_string(),
        16 => "美漫卡通风格，游戏场景图，没有人，魔法森林，魔法祭台，粗线条，扁平画风，没有人".to_string(),
        17 => "设计一个2D的45度角游戏场景，漫画平涂简单风格，Q版。场景主题：城市战场边缘 核心关键词：铁丝网、沙袋、战壕、废弃建筑 地形：废弃城市的郊区地带，周围有废墟和破旧汽车 基地：现代化兵工厂，门口摆放弹药或军用设施".to_string(),
        18 => "生成q版2d游戏角色，丧尸，漫画平涂风格。两类丧尸怪形象，一类瘦丧尸，敏捷型，一栋速度快，血量少。另外一类肉型，移动慢，血量高。可以配合一些现代的武器或者配饰。".to_string(),
        19 => "游戏角色设计，像素风，纯白色背景，四视图，正视图，侧视图，背视图，可爱，小魔女，拿着一个小的法杖，带着魔法帽，可爱，高饱和度的配色，光影艺术，单色背景，美丽，近距离".to_string(),
        20 => "生成一个像素风格的重甲骑士三视图，并且要在下方展示武器特写".to_string(),
        22 => "生成一组游戏风格UI素材，包含上百个不同样式的按钮、面板、进度条，采用复古美食荒野牛仔风格，不应受到阴影或光线的影响。大师，杰作。".to_string(),
        23 => "游戏特效，没有人物，没有主角，灰色背景，有层次的，俯视角，平面，多个不一样的设计，素材，排列整齐，暗黑，爆炸后的地面痕迹，没有火焰，没有烟雾".to_string(),
        24 => "游戏CG风格，隐约的淡彩褪色，泛朦，对焦模糊，特写，剑风传奇格斯，高颜值，复古，迷离，低饱和，反射，质感，泛光模糊晕染，高噪点，胶片颗粒质感，极具艺术感，震撼人心，色彩丰富，暗部叠加，特写镜头，超高清。落雪飞溅，前景落雪虚化，动态模糊，背景动态虚化，阳光灿烂，蓝天白云，光影交错，特写镜头，突出速度感和视觉冲击力，强透视，原比例。".to_string(),
        _ => {
            format!("{title}，{ratio} 构图，官方灵感示例，可用于做同款或作为参考图继续创作。")
        }
    }
}
