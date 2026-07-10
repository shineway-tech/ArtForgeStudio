use super::endpoint;
use artait_provider::request::ImageGenerationRequest;
use artait_provider::{ProviderContext, ProviderModelList};

pub(crate) struct ImageParams {
    pub(crate) size: &'static str,
    pub(crate) pixel_size: &'static str,
    pub(crate) resolution: Option<&'static str>,
    pub(crate) response_format: Option<&'static str>,
    pub(crate) quality: Option<&'static str>,
    pub(crate) image_size: Option<&'static str>,
    pub(crate) aspect_ratio: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImageCompatStyle {
    Auto,
    Cpa,
    ToApis,
    Sub2Api,
    NewApi,
}

impl ImageCompatStyle {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ImageCompatStyle::Auto => "auto",
            ImageCompatStyle::Cpa => "cpa",
            ImageCompatStyle::ToApis => "toapis",
            ImageCompatStyle::Sub2Api => "sub2api",
            ImageCompatStyle::NewApi => "newapi",
        }
    }

    pub(crate) fn uses_toapis_gpt_image_body(self) -> bool {
        matches!(
            self,
            ImageCompatStyle::Auto | ImageCompatStyle::Cpa | ImageCompatStyle::ToApis
        )
    }

    pub(crate) fn includes_proxy_image_fields(self) -> bool {
        self.uses_toapis_gpt_image_body()
    }
}

pub(crate) fn classify_models(models: Vec<String>) -> ProviderModelList {
    let mut generation = Vec::new();
    let mut analysis = Vec::new();
    let mut all = Vec::new();

    for model in models {
        let normalized = model.trim().trim_start_matches("models/").to_string();
        if normalized.is_empty() || all.iter().any(|item| item == &normalized) {
            continue;
        }
        all.push(normalized.clone());
        if looks_like_image_model(&model) {
            push_model_unique(&mut generation, normalized.clone());
        }
        if !looks_like_image_only_model(&model) {
            push_model_unique(&mut analysis, normalized);
        }
    }

    if generation.is_empty() {
        generation = all.clone();
    }
    if analysis.is_empty() {
        analysis = all;
    }

    ProviderModelList {
        generation,
        analysis,
        video: Vec::new(),
    }
}

pub(crate) fn push_model_unique(models: &mut Vec<String>, model: String) {
    let model = model.trim().trim_start_matches("models/").to_string();
    if model.is_empty() || models.iter().any(|item| item == &model) {
        return;
    }
    models.push(model);
}

pub(crate) fn looks_like_image_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("image")
        || lower.contains("img")
        || lower.contains("dall-e")
        || lower.contains("imagen")
        || lower.contains("banana")
        || lower.contains("flux")
        || lower.contains("sdxl")
        || lower.contains("stable")
        || lower.contains("midjourney")
        || lower.contains("seedream")
        || lower.contains("kling")
}

pub(crate) fn looks_like_image_only_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("dall-e") || lower.contains("gpt-image") || lower.contains("imagen")
}

pub(crate) fn pick_image_params(model: &str, req: &ImageGenerationRequest) -> ImageParams {
    let model = model.to_ascii_lowercase();
    let quality = req.quality.as_deref().unwrap_or("2K");
    let aspect = req.aspect_ratio.as_deref().unwrap_or("1:1");

    if endpoint::is_gemini_model(&model) {
        let size = gemini_pixel_size(&model, quality, aspect);
        return ImageParams {
            size,
            pixel_size: size,
            resolution: None,
            response_format: None,
            quality: None,
            image_size: gemini_image_size(&model, quality),
            aspect_ratio: Some(gemini_aspect_ratio_for_model(&model, aspect)),
        };
    }

    if is_gpt_image_2_model(&model) {
        let resolution = gpt_image_2_resolution(quality);
        let aspect = gpt_image_2_aspect_for_resolution(resolution, aspect);
        return ImageParams {
            size: aspect,
            pixel_size: gpt_image_2_pixel_size_for(resolution, aspect),
            resolution: Some(resolution),
            response_format: Some("url"),
            quality: None,
            image_size: None,
            aspect_ratio: None,
        };
    }

    let size = match explicit_size(req.size.as_deref()) {
        Some(size) => size,
        None => req
            .resolution
            .map(|(w, h)| openai_legacy_size_from_resolution(w, h))
            .unwrap_or_else(|| openai_legacy_size_for(aspect)),
    };
    ImageParams {
        size,
        pixel_size: size,
        resolution: None,
        response_format: None,
        quality: openai_image_quality(&model, quality),
        image_size: None,
        aspect_ratio: None,
    }
}

pub(crate) fn image_compat_style(ctx: &ProviderContext) -> ImageCompatStyle {
    match ctx
        .extra
        .get("api_style")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "cpa" | "cpa_api" => ImageCompatStyle::Cpa,
        "sub2api" | "sub2_api" => ImageCompatStyle::Sub2Api,
        "newapi" | "new_api" => ImageCompatStyle::NewApi,
        "toapis" | "to_apis" | "toapis_gpt_image_2" => ImageCompatStyle::ToApis,
        _ => ImageCompatStyle::Auto,
    }
}

pub(crate) fn pick_openai_image_params(
    model: &str,
    req: &ImageGenerationRequest,
    style: ImageCompatStyle,
) -> ImageParams {
    let model = model.to_ascii_lowercase();
    let quality = req.quality.as_deref().unwrap_or("2K");
    let aspect = req.aspect_ratio.as_deref().unwrap_or("1:1");

    if is_gpt_image_2_model(&model) {
        if matches!(style, ImageCompatStyle::Sub2Api | ImageCompatStyle::NewApi) {
            let size = gpt_image_2_sub2api_pixel_size(quality, aspect);
            return ImageParams {
                size,
                pixel_size: size,
                resolution: None,
                response_format: Some("url"),
                quality: None,
                image_size: None,
                aspect_ratio: None,
            };
        }
        return pick_image_params(&model, req);
    }

    if is_nano_banana_model(&model) {
        let aspect = nano_banana_openai_aspect_ratio(aspect);
        let size = nano_banana_openai_pixel_size(quality, aspect);
        let include_proxy_fields = style.includes_proxy_image_fields();
        return ImageParams {
            size,
            pixel_size: size,
            resolution: None,
            response_format: None,
            quality: None,
            image_size: include_proxy_fields
                .then(|| nano_banana_openai_image_size(quality))
                .flatten(),
            aspect_ratio: include_proxy_fields.then_some(aspect),
        };
    }

    pick_image_params(&model, req)
}

pub(crate) fn explicit_size(size: Option<&str>) -> Option<&'static str> {
    match size {
        Some("1024x1024") => Some("1024x1024"),
        Some("1536x864") => Some("1536x864"),
        Some("864x1536") => Some("864x1536"),
        Some("1536x1024") => Some("1536x1024"),
        Some("1024x1536") => Some("1024x1536"),
        Some("2048x2048") => Some("2048x2048"),
        Some("2048x1152") => Some("2048x1152"),
        Some("1152x2048") => Some("1152x2048"),
        Some("2880x2880") => Some("2880x2880"),
        Some("3840x2160") => Some("3840x2160"),
        Some("2160x3840") => Some("2160x3840"),
        _ => None,
    }
}

pub fn is_gpt_image_2_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("gpt-image-2")
}

pub(crate) fn is_nano_banana_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("nano-banana")
}

pub(crate) fn gpt_image_2_resolution(quality: &str) -> &'static str {
    match quality {
        "1K" => "1K",
        "4K" => "4K",
        _ => "2K",
    }
}

pub(crate) fn gpt_image_2_aspect_for_resolution(resolution: &str, aspect: &str) -> &'static str {
    match resolution {
        "1K" => match aspect {
            "3:2" => "3:2",
            "2:3" => "2:3",
            _ => "1:1",
        },
        "4K" => match aspect {
            "9:16" => "9:16",
            "2:1" => "2:1",
            "1:2" => "1:2",
            "21:9" => "21:9",
            "9:21" => "9:21",
            _ => "16:9",
        },
        _ => match aspect {
            "3:2" => "3:2",
            "2:3" => "2:3",
            "4:3" => "4:3",
            "3:4" => "3:4",
            "5:4" => "5:4",
            "4:5" => "4:5",
            "16:9" => "16:9",
            "9:16" => "9:16",
            "2:1" => "2:1",
            "1:2" => "1:2",
            "21:9" => "21:9",
            "9:21" => "9:21",
            _ => "1:1",
        },
    }
}

pub(crate) fn gpt_image_2_pixel_size_for(resolution: &str, aspect: &str) -> &'static str {
    match (resolution, aspect) {
        ("1K", "3:2") => "1536x1024",
        ("1K", "2:3") => "1024x1536",
        ("1K", _) => "1024x1024",
        ("4K", "9:16") => "2160x3840",
        ("4K", "2:1") => "3840x1920",
        ("4K", "1:2") => "1920x3840",
        ("4K", "21:9") => "3840x1648",
        ("4K", "9:21") => "1648x3840",
        ("4K", _) => "3840x2160",
        (_, "3:2") => "2048x1360",
        (_, "2:3") => "1360x2048",
        (_, "4:3") => "2048x1536",
        (_, "3:4") => "1536x2048",
        (_, "5:4") => "2560x2048",
        (_, "4:5") => "2048x2560",
        (_, "16:9") => "2048x1152",
        (_, "9:16") => "1152x2048",
        (_, "2:1") => "2688x1344",
        (_, "1:2") => "1344x2688",
        (_, "21:9") => "2688x1152",
        (_, "9:21") => "1152x2688",
        _ => "2048x2048",
    }
}

pub(crate) fn gpt_image_2_sub2api_pixel_size(quality: &str, aspect: &str) -> &'static str {
    let resolution = gpt_image_2_resolution(quality);
    let aspect = gpt_image_2_aspect_for_resolution(resolution, aspect);
    gpt_image_2_pixel_size_for(resolution, aspect)
}

pub(crate) fn openai_image_quality(model: &str, quality: &str) -> Option<&'static str> {
    if model.contains("dall-e-2") {
        return None;
    }
    if model.contains("dall-e-3") {
        return Some(match quality {
            "4K" | "2K" | "high" => "hd",
            "hd" => "hd",
            "standard" => "standard",
            _ => "standard",
        });
    }
    Some(match quality {
        "4K" | "high" => "high",
        "2K" | "medium" => "medium",
        "1K" | "low" => "low",
        "auto" => "auto",
        _ => "medium",
    })
}

pub(crate) fn gemini_image_size(model: &str, quality: &str) -> Option<&'static str> {
    match gemini_image_family(model) {
        GeminiImageFamily::Gemini31Flash => Some(match quality {
            "512" => "512",
            "4K" => "4K",
            "2K" => "2K",
            _ => "1K",
        }),
        GeminiImageFamily::Gemini3Pro => Some(match quality {
            "4K" => "4K",
            "2K" => "2K",
            _ => "1K",
        }),
        GeminiImageFamily::Gemini25Flash => None,
        GeminiImageFamily::Other => Some(match quality {
            "4K" => "4K",
            "2K" => "2K",
            _ => "1K",
        }),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GeminiImageFamily {
    Gemini31Flash,
    Gemini3Pro,
    Gemini25Flash,
    Other,
}

pub(crate) fn gemini_image_family(model: &str) -> GeminiImageFamily {
    let model = model.to_ascii_lowercase();
    if model.contains("gemini-3.1-flash-image") || model.contains("nano-banana-2") {
        GeminiImageFamily::Gemini31Flash
    } else if model.contains("gemini-3-pro-image") || model.contains("nano-banana-pro") {
        GeminiImageFamily::Gemini3Pro
    } else if model.contains("gemini-2.5-flash-image")
        || (model.contains("nano-banana")
            && !model.contains("nano-banana-2")
            && !model.contains("nano-banana-pro"))
    {
        GeminiImageFamily::Gemini25Flash
    } else {
        GeminiImageFamily::Other
    }
}

pub(crate) fn gemini_pixel_size(model: &str, quality: &str, aspect: &str) -> &'static str {
    let aspect = gemini_aspect_ratio_for_model(model, aspect);
    match gemini_image_family(model) {
        GeminiImageFamily::Gemini31Flash => gemini31_flash_pixel_size(quality, aspect),
        GeminiImageFamily::Gemini3Pro => gemini3_pro_pixel_size(quality, aspect),
        GeminiImageFamily::Gemini25Flash => gemini25_flash_pixel_size(aspect),
        GeminiImageFamily::Other => "1024x1024",
    }
}

pub(crate) fn gemini31_flash_pixel_size(quality: &str, aspect: &str) -> &'static str {
    match (quality, aspect) {
        ("512", "1:4") => "256x1024",
        ("512", "1:8") => "192x1536",
        ("512", "2:3") => "424x632",
        ("512", "3:2") => "632x424",
        ("512", "3:4") => "448x600",
        ("512", "4:1") => "1024x256",
        ("512", "4:3") => "600x448",
        ("512", "4:5") => "464x576",
        ("512", "5:4") => "576x464",
        ("512", "8:1") => "1536x192",
        ("512", "9:16") => "384x688",
        ("512", "16:9") => "688x384",
        ("512", "21:9") => "792x168",
        ("512", _) => "512x512",
        ("4K", "1:4") => "2048x8192",
        ("4K", "1:8") => "1536x12288",
        ("4K", "2:3") => "3392x5056",
        ("4K", "3:2") => "5056x3392",
        ("4K", "3:4") => "3584x4800",
        ("4K", "4:1") => "8192x2048",
        ("4K", "4:3") => "4800x3584",
        ("4K", "4:5") => "3712x4608",
        ("4K", "5:4") => "4608x3712",
        ("4K", "8:1") => "12288x1536",
        ("4K", "9:16") => "3072x5504",
        ("4K", "16:9") => "5504x3072",
        ("4K", "21:9") => "6336x2688",
        ("4K", _) => "4096x4096",
        ("2K", "1:4") => "1024x4096",
        ("2K", "1:8") => "768x6144",
        ("2K", "2:3") => "1696x2528",
        ("2K", "3:2") => "2528x1696",
        ("2K", "3:4") => "1792x2400",
        ("2K", "4:1") => "4096x1024",
        ("2K", "4:3") => "2400x1792",
        ("2K", "4:5") => "1856x2304",
        ("2K", "5:4") => "2304x1856",
        ("2K", "8:1") => "6144x768",
        ("2K", "9:16") => "1536x2752",
        ("2K", "16:9") => "2752x1536",
        ("2K", "21:9") => "3168x1344",
        ("2K", _) => "2048x2048",
        (_, "1:4") => "512x2048",
        (_, "1:8") => "384x3072",
        (_, "2:3") => "848x1264",
        (_, "3:2") => "1264x848",
        (_, "3:4") => "896x1200",
        (_, "4:1") => "2048x512",
        (_, "4:3") => "1200x896",
        (_, "4:5") => "928x1152",
        (_, "5:4") => "1152x928",
        (_, "8:1") => "3072x384",
        (_, "9:16") => "768x1376",
        (_, "16:9") => "1376x768",
        (_, "21:9") => "1584x672",
        _ => "1024x1024",
    }
}

pub(crate) fn gemini3_pro_pixel_size(quality: &str, aspect: &str) -> &'static str {
    match (quality, aspect) {
        ("4K", "2:3") => "3392x5056",
        ("4K", "3:2") => "5056x3392",
        ("4K", "3:4") => "3584x4800",
        ("4K", "4:3") => "4800x3584",
        ("4K", "4:5") => "3712x4608",
        ("4K", "5:4") => "4608x3712",
        ("4K", "9:16") => "3072x5504",
        ("4K", "16:9") => "5504x3072",
        ("4K", "21:9") => "6336x2688",
        ("4K", _) => "4096x4096",
        ("2K", "2:3") => "1696x2528",
        ("2K", "3:2") => "2528x1696",
        ("2K", "3:4") => "1792x2400",
        ("2K", "4:3") => "2400x1792",
        ("2K", "4:5") => "1856x2304",
        ("2K", "5:4") => "2304x1856",
        ("2K", "9:16") => "1536x2752",
        ("2K", "16:9") => "2752x1536",
        ("2K", "21:9") => "3168x1344",
        ("2K", _) => "2048x2048",
        (_, "2:3") => "848x1264",
        (_, "3:2") => "1264x848",
        (_, "3:4") => "896x1200",
        (_, "4:3") => "1200x896",
        (_, "4:5") => "928x1152",
        (_, "5:4") => "1152x928",
        (_, "9:16") => "768x1376",
        (_, "16:9") => "1376x768",
        (_, "21:9") => "1584x672",
        _ => "1024x1024",
    }
}

pub(crate) fn gemini25_flash_pixel_size(aspect: &str) -> &'static str {
    match aspect {
        "2:3" => "832x1248",
        "3:2" => "1248x832",
        "3:4" => "864x1184",
        "4:3" => "1184x864",
        "4:5" => "896x1152",
        "5:4" => "1152x896",
        "9:16" => "768x1344",
        "16:9" => "1344x768",
        "21:9" => "1536x672",
        _ => "1024x1024",
    }
}

pub(crate) fn nano_banana_openai_image_size(quality: &str) -> Option<&'static str> {
    match quality {
        "1K" => Some("1K"),
        "4K" => Some("4K"),
        _ => Some("2K"),
    }
}

pub(crate) fn nano_banana_openai_aspect_ratio(aspect: &str) -> &'static str {
    match aspect {
        "1:1" => "1:1",
        "16:9" => "16:9",
        "9:16" => "9:16",
        "4:3" => "4:3",
        "3:4" => "3:4",
        "3:2" => "3:2",
        "2:3" => "2:3",
        "5:4" => "5:4",
        "4:5" => "4:5",
        "21:9" => "21:9",
        "9:21" => "9:21",
        "1:3" => "1:3",
        "3:1" => "3:1",
        "2:1" => "2:1",
        "1:2" => "1:2",
        _ => "1:1",
    }
}

pub(crate) fn nano_banana_openai_pixel_size(quality: &str, aspect: &str) -> &'static str {
    match (quality, aspect) {
        ("1K", "16:9") => "1280x720",
        ("1K", "9:16") => "720x1280",
        ("1K", "4:3") => "1152x864",
        ("1K", "3:4") => "864x1152",
        ("1K", "3:2") => "1536x1024",
        ("1K", "2:3") => "1024x1536",
        ("1K", "5:4") => "1120x896",
        ("1K", "4:5") => "896x1120",
        ("1K", "21:9") => "1456x624",
        ("1K", "9:21") => "624x1456",
        ("1K", "1:3") => "688x2048",
        ("1K", "3:1") => "2048x688",
        ("1K", "2:1") => "1536x768",
        ("1K", "1:2") => "768x1536",
        ("1K", _) => "1024x1024",
        ("4K", "16:9") => "3840x2160",
        ("4K", "9:16") => "2160x3840",
        ("4K", "4:3") => "3264x2448",
        ("4K", "3:4") => "2448x3264",
        ("4K", "3:2") => "3504x2336",
        ("4K", "2:3") => "2336x3504",
        ("4K", "5:4") => "3200x2560",
        ("4K", "4:5") => "2560x3200",
        ("4K", "21:9") => "3840x1648",
        ("4K", "9:21") => "1648x3840",
        ("4K", "1:3") => "1280x3840",
        ("4K", "3:1") => "3840x1280",
        ("4K", "2:1") => "3840x1920",
        ("4K", "1:2") => "1920x3840",
        ("4K", _) => "2880x2880",
        (_, "16:9") => "2048x1152",
        (_, "9:16") => "1152x2048",
        (_, "4:3") => "2304x1728",
        (_, "3:4") => "1728x2304",
        (_, "3:2") => "2048x1360",
        (_, "2:3") => "1360x2048",
        (_, "5:4") => "2240x1792",
        (_, "4:5") => "1792x2240",
        (_, "21:9") => "2912x1248",
        (_, "9:21") => "1248x2912",
        (_, "1:3") => "688x2048",
        (_, "3:1") => "2048x688",
        (_, "2:1") => "3072x1536",
        (_, "1:2") => "1536x3072",
        _ => "2048x2048",
    }
}

pub(crate) fn openai_legacy_size_for(aspect: &str) -> &'static str {
    match aspect {
        "16:9" => "1536x1024",
        "9:16" => "1024x1536",
        _ => "1024x1024",
    }
}

pub(crate) fn openai_legacy_size_from_resolution(w: u32, h: u32) -> &'static str {
    if w == h {
        "1024x1024"
    } else if w > h {
        "1536x1024"
    } else {
        "1024x1536"
    }
}

pub(crate) fn gemini_aspect_ratio_for_model(model: &str, aspect: &str) -> &'static str {
    match gemini_image_family(model) {
        GeminiImageFamily::Gemini31Flash => gemini31_flash_aspect_ratio(aspect),
        GeminiImageFamily::Gemini3Pro | GeminiImageFamily::Gemini25Flash => {
            gemini_standard_aspect_ratio(aspect)
        }
        GeminiImageFamily::Other => gemini_standard_aspect_ratio(aspect),
    }
}

pub(crate) fn gemini31_flash_aspect_ratio(aspect: &str) -> &'static str {
    match aspect {
        "1:4" => "1:4",
        "1:8" => "1:8",
        "2:3" => "2:3",
        "3:2" => "3:2",
        "3:4" => "3:4",
        "4:1" => "4:1",
        "4:3" => "4:3",
        "4:5" => "4:5",
        "5:4" => "5:4",
        "8:1" => "8:1",
        "9:16" => "9:16",
        "16:9" => "16:9",
        "21:9" => "21:9",
        _ => "1:1",
    }
}

pub(crate) fn gemini_standard_aspect_ratio(aspect: &str) -> &'static str {
    match aspect {
        "2:3" => "2:3",
        "3:2" => "3:2",
        "3:4" => "3:4",
        "4:3" => "4:3",
        "4:5" => "4:5",
        "5:4" => "5:4",
        "9:16" => "9:16",
        "16:9" => "16:9",
        "21:9" => "21:9",
        _ => "1:1",
    }
}
