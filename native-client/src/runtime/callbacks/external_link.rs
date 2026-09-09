use super::*;

const AUDIO_SEPARATION_URL: &str = "https://www.shineway.tech/biyi/feature/audio";
const CHAT_INSIGHT_URL: &str = "https://www.shineway.tech/biyi/feature/chat";
const MARKETING_MASTER_URL: &str = "https://www.shineway.tech/product/marketing-master/";

fn trusted_recommendation_url(candidate: &str) -> Option<&'static str> {
    match candidate.trim() {
        AUDIO_SEPARATION_URL => Some(AUDIO_SEPARATION_URL),
        CHAT_INSIGHT_URL => Some(CHAT_INSIGHT_URL),
        MARKETING_MASTER_URL => Some(MARKETING_MASTER_URL),
        _ => None,
    }
}

fn open_recommendation_link(candidate: &str, upgrade: &UpgradeLatch) -> Result<()> {
    let url = trusted_recommendation_url(candidate)
        .ok_or_else(|| anyhow!("untrusted recommendation link"))?;
    let permit = upgrade.defer_external_effect().map_err(|required| anyhow!(required.as_error().user_message()))?;
    upgrade.commit_deferred_external_effect_if_open(permit, || open_trusted_recommendation_link(url))
        .map_err(|required| anyhow!(required.as_error().user_message()))?
}

fn open_trusted_recommendation_link(url: &str) -> Result<()> {

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

pub(super) fn wire_external_link_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    state.on_open_external_link(move |candidate| {
        let Some(backend) = context.backend.as_ref() else { return; };
        if let Err(error) = open_recommendation_link(candidate.as_str(), backend.api.upgrade_latch()) {
            eprintln!("{error:#}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_required_upgrade_denies_actual_recommendation_launch_front_door() {
        let latch = UpgradeLatch::default();
        latch.trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        assert!(open_recommendation_link(AUDIO_SEPARATION_URL, &latch).is_err());
    }

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
            trusted_recommendation_url(MARKETING_MASTER_URL),
            Some(MARKETING_MASTER_URL)
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
