use super::*;

const AUDIO_SEPARATION_URL: &str = "https://www.shineway.tech/biyi/feature/audio";
const CHAT_INSIGHT_URL: &str = "https://www.shineway.tech/biyi/feature/chat";

fn trusted_recommendation_url(candidate: &str) -> Option<&'static str> {
    match candidate.trim() {
        AUDIO_SEPARATION_URL => Some(AUDIO_SEPARATION_URL),
        CHAT_INSIGHT_URL => Some(CHAT_INSIGHT_URL),
        _ => None,
    }
}

fn open_recommendation_link(candidate: &str) -> Result<()> {
    let url = trusted_recommendation_url(candidate)
        .ok_or_else(|| anyhow!("untrusted recommendation link"))?;

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(url)
            .spawn()
            .context("failed to open recommendation link")?;
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("rundll32.exe")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .spawn()
            .context("failed to open recommendation link")?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .context("failed to open recommendation link")?;
    }
    Ok(())
}

pub(super) fn wire_external_link_callbacks(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.on_open_external_link(move |candidate| {
        if let Err(error) = open_recommendation_link(candidate.as_str()) {
            eprintln!("{error:#}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_https_recommendation_links_are_allowed() {
        assert_eq!(
            trusted_recommendation_url(AUDIO_SEPARATION_URL),
            Some(AUDIO_SEPARATION_URL)
        );
        assert_eq!(
            trusted_recommendation_url(CHAT_INSIGHT_URL),
            Some(CHAT_INSIGHT_URL)
        );
        assert_eq!(
            trusted_recommendation_url(
                "https://www.shineway.tech.attacker.example/biyi/feature/audio"
            ),
            None
        );
        assert_eq!(
            trusted_recommendation_url("http://www.shineway.tech/biyi/feature/audio"),
            None
        );
    }
}
