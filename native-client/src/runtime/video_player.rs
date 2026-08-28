use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlayerCommand {
    Download,
    OpenFolder,
    Regenerate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VideoPlayerBounds {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl VideoPlayerBounds {
    fn from_logical(x: f32, y: f32, width: f32, height: f32, scale_factor: f32) -> Option<Self> {
        if ![x, y, width, height, scale_factor]
            .iter()
            .all(|value| value.is_finite())
            || scale_factor <= 0.0
            || width < 2.0
            || height < 2.0
        {
            return None;
        }
        Some(Self {
            x: (x.max(0.0) * scale_factor).round() as u32,
            y: (y.max(0.0) * scale_factor).round() as u32,
            width: (width * scale_factor).round().max(1.0) as u32,
            height: (height * scale_factor).round().max(1.0) as u32,
        })
    }
}

fn validated_local_video_url(path: &Path) -> Result<reqwest::Url> {
    let canonical = fs::canonicalize(path).context("视频文件不存在")?;
    if !canonical.is_file()
        || !matches!(
            canonical
                .extension()
                .and_then(|value| value.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("mp4" | "webm" | "mov")
        )
    {
        return Err(anyhow!("播放器只支持本地视频文件"));
    }
    let managed_root =
        fs::canonicalize(app_data_dir().join("videos")).context("视频保存目录不可用")?;
    if !canonical.starts_with(&managed_root) {
        return Err(anyhow!("播放器拒绝加载未校验的视频文件"));
    }
    reqwest::Url::from_file_path(&canonical).map_err(|_| anyhow!("本地视频地址无效"))
}

fn parse_player_command(body: &str) -> Option<PlayerCommand> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value.as_object()?.len() != 1 {
        return None;
    }
    match value.get("command")?.as_str()? {
        "download" => Some(PlayerCommand::Download),
        "open_folder" => Some(PlayerCommand::OpenFolder),
        "regenerate" => Some(PlayerCommand::Regenerate),
        _ => None,
    }
}

fn player_html(video_url: &reqwest::Url) -> Result<String> {
    let encoded_url = serde_json::to_string(video_url.as_str()).context("视频地址编码失败")?;
    Ok(include_str!("video_player/player.html").replace("__VIDEO_SRC_JSON__", &encoded_url))
}

pub(super) fn sync_video_player(
    app: &AppWindow,
    local_path: &Path,
    logical_bounds: (f32, f32, f32, f32),
) -> Result<()> {
    let scale_factor = app.window().scale_factor();
    let Some(bounds) = VideoPlayerBounds::from_logical(
        logical_bounds.0,
        logical_bounds.1,
        logical_bounds.2,
        logical_bounds.3,
        scale_factor,
    ) else {
        return Err(anyhow!("播放器区域无效"));
    };
    let video_url = validated_local_video_url(local_path)?;
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        desktop_video_player::sync(app, local_path.to_path_buf(), video_url, bounds)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (app, video_url, bounds);
        Err(anyhow!("当前平台不支持应用内视频播放器"))
    }
}

pub(super) fn close_video_player() {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    desktop_video_player::close();
}

pub(super) fn set_video_player_visible(visible: bool) {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    desktop_video_player::set_visible(visible);
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = visible;
}

fn handle_player_command(app: &AppWindow, local_path: &Path, command: PlayerCommand) {
    let state = app.global::<AppState>();
    match command {
        PlayerCommand::Download => {
            let default_name = local_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("elunvi-video.mp4");
            if let Some(destination) = rfd::FileDialog::new()
                .set_title("保存视频")
                .set_file_name(default_name)
                .add_filter("MP4 视频", &["mp4"])
                .save_file()
            {
                match fs::copy(local_path, &destination) {
                    Ok(_) => state.set_video_status("视频已保存".into()),
                    Err(error) => state.set_video_status(format!("视频保存失败：{error}").into()),
                }
            }
        }
        PlayerCommand::OpenFolder => match reveal_path_in_file_manager(local_path) {
            Ok(_) => state.set_video_status("已打开视频所在文件夹".into()),
            Err(error) => state.set_video_status(format!("打开文件夹失败：{error}").into()),
        },
        PlayerCommand::Regenerate => {
            close_video_player();
            state.set_video_result_path("".into());
            state.set_video_quote_ready(false);
            state.set_video_status("正在更新服务端报价...".into());
            state.invoke_request_video_quote(
                state.get_video_aspect_ratio(),
                state.get_video_resolution(),
                state.get_video_duration_seconds(),
            );
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod desktop_video_player {
    use super::*;
    use std::cell::RefCell;
    use wry::dpi::{PhysicalPosition, PhysicalSize};
    use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder};

    struct ActivePlayer {
        path: PathBuf,
        webview: WebView,
    }

    thread_local! {
        static VIDEO_WEBVIEW: RefCell<Option<ActivePlayer>> = const { RefCell::new(None) };
    }

    pub(super) fn sync(
        app: &AppWindow,
        local_path: PathBuf,
        video_url: reqwest::Url,
        bounds: VideoPlayerBounds,
    ) -> Result<()> {
        let rect = Rect {
            position: PhysicalPosition::new(bounds.x, bounds.y).into(),
            size: PhysicalSize::new(bounds.width, bounds.height).into(),
        };
        let reused = VIDEO_WEBVIEW.with(|slot| {
            let slot = slot.borrow();
            if let Some(active) = slot.as_ref().filter(|active| active.path == local_path) {
                active.webview.set_bounds(rect).ok();
                active.webview.set_visible(true).ok();
                true
            } else {
                false
            }
        });
        if reused {
            return Ok(());
        }

        close();
        let html = player_html(&video_url)?;
        let weak = app.as_weak();
        let command_path = local_path.clone();
        let window_handle = app.window().window_handle();
        let webview = WebViewBuilder::new()
            .with_html(html)
            .with_bounds(rect)
            .with_devtools(false)
            .with_clipboard(false)
            .with_navigation_handler(|candidate| candidate == "about:blank")
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_ipc_handler(move |request| {
                let Some(command) = parse_player_command(request.body()) else {
                    return;
                };
                let command_path = command_path.clone();
                let _ = weak.clone().upgrade_in_event_loop(move |app| {
                    handle_player_command(&app, &command_path, command);
                });
            })
            .build_as_child(&window_handle)
            .context("应用内视频播放器初始化失败")?;
        VIDEO_WEBVIEW.with(|slot| {
            *slot.borrow_mut() = Some(ActivePlayer {
                path: local_path,
                webview,
            });
        });
        Ok(())
    }

    pub(super) fn close() {
        VIDEO_WEBVIEW.with(|slot| {
            slot.borrow_mut().take();
        });
    }

    pub(super) fn set_visible(visible: bool) {
        VIDEO_WEBVIEW.with(|slot| {
            if let Some(active) = slot.borrow().as_ref() {
                active.webview.set_visible(visible).ok();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_commands_are_a_strict_allowlist() {
        assert_eq!(
            parse_player_command(r#"{"command":"download"}"#),
            Some(PlayerCommand::Download)
        );
        assert_eq!(
            parse_player_command(r#"{"command":"open_folder"}"#),
            Some(PlayerCommand::OpenFolder)
        );
        assert_eq!(
            parse_player_command(r#"{"command":"regenerate"}"#),
            Some(PlayerCommand::Regenerate)
        );
        for body in [
            r#"{"command":"open_url"}"#,
            r#"{"command":"seek","value":999999}"#,
            r#"{"command":"download","path":"C:\\secret.txt"}"#,
            "not-json",
        ] {
            assert_eq!(parse_player_command(body), None);
        }
    }

    #[test]
    fn player_bounds_reject_invalid_or_zero_areas() {
        assert_eq!(
            VideoPlayerBounds::from_logical(10.0, 20.0, 640.0, 360.0, 1.5),
            Some(VideoPlayerBounds {
                x: 15,
                y: 30,
                width: 960,
                height: 540
            })
        );
        assert!(VideoPlayerBounds::from_logical(0.0, 0.0, 0.0, 360.0, 1.0).is_none());
        assert!(VideoPlayerBounds::from_logical(0.0, 0.0, 640.0, f32::NAN, 1.0).is_none());
    }
}
