use artait_model::{ProviderError, ReferenceImage};
use artait_provider::ProviderResult;

pub fn image_data_url(img: &ReferenceImage) -> ProviderResult<String> {
    if let Some(url) = &img.uploaded_url {
        if !url.trim().is_empty() {
            return Ok(url.clone());
        }
    }
    let bytes = std::fs::read(&img.local_path).map_err(|e| {
        ProviderError::Io(format!("读取参考图失败 {}: {e}", img.local_path.display()))
    })?;
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    let mime = if img.mime_type.is_empty() {
        "image/png"
    } else {
        img.mime_type.as_str()
    };
    Ok(format!("data:{mime};base64,{b64}"))
}

pub fn image_inline_data(img: &ReferenceImage) -> ProviderResult<(String, String)> {
    let bytes = std::fs::read(&img.local_path).map_err(|e| {
        ProviderError::Io(format!("读取参考图失败 {}: {e}", img.local_path.display()))
    })?;
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    let mime = if img.mime_type.is_empty() {
        "image/png".to_string()
    } else {
        img.mime_type.clone()
    };
    Ok((mime, data))
}

pub fn image_multipart_bytes(img: &ReferenceImage) -> ProviderResult<(String, Vec<u8>)> {
    let bytes = std::fs::read(&img.local_path).map_err(|e| {
        ProviderError::Io(format!("读取参考图失败 {}: {e}", img.local_path.display()))
    })?;
    let mime = if img.mime_type.is_empty() {
        "image/png".to_string()
    } else {
        img.mime_type.clone()
    };
    Ok((mime, bytes))
}

pub fn data_url_to_parts(data_url: &str) -> Option<(String, String)> {
    let rest = data_url.strip_prefix("data:")?;
    let (mime, data) = rest.split_once(";base64,")?;
    Some((mime.to_string(), data.to_string()))
}
