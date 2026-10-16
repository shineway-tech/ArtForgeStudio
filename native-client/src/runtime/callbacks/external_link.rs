use super::*;

const AUDIO_SEPARATION_URL: &str = "https://www.shineway.tech/biyi/feature/audio";
const CHAT_INSIGHT_URL: &str = "https://www.shineway.tech/biyi/feature/chat";
const MARKETING_MASTER_URL: &str = "https://www.shineway.tech/product/marketing-master/";
const OFFICIAL_GROUP_URL: &str = "https://qm.qq.com/q/ugUpnQFL2M";

fn trusted_external_url(candidate: &str) -> Option<&'static str> {
    match candidate.trim() {
        AUDIO_SEPARATION_URL => Some(AUDIO_SEPARATION_URL),
        CHAT_INSIGHT_URL => Some(CHAT_INSIGHT_URL),
        MARKETING_MASTER_URL => Some(MARKETING_MASTER_URL),
        OFFICIAL_GROUP_URL => Some(OFFICIAL_GROUP_URL),
        _ => None,
    }
}

fn open_external_link(candidate: &str, upgrade: &UpgradeLatch) -> Result<()> {
    let url = trusted_external_url(candidate)
        .ok_or_else(|| anyhow!("untrusted external link"))?;
    let permit = upgrade.defer_external_effect().map_err(|required| anyhow!(required.as_error().user_message()))?;
    upgrade.commit_deferred_external_effect_if_open(permit, || open_trusted_external_link(url))
        .map_err(|required| anyhow!(required.as_error().user_message()))?
}

fn open_trusted_external_link(url: &str) -> Result<()> {

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(url)
            .spawn()
            .context("failed to open external link")?;
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("rundll32.exe")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .spawn()
            .context("failed to open external link")?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .context("failed to open external link")?;
    }
    Ok(())
}

pub(super) fn wire_external_link_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    state.on_open_external_link(move |candidate| {
        let Some(backend) = context.backend.as_ref() else { return; };
        if let Err(error) = open_external_link(candidate.as_str(), backend.api.upgrade_latch()) {
            eprintln!("{error:#}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_required_upgrade_denies_external_link_launch() {
        let latch = UpgradeLatch::default();
        latch.trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        assert!(open_external_link(OFFICIAL_GROUP_URL, &latch).is_err());
    }

    #[test]
    fn only_known_https_external_links_are_allowed() {
        assert_eq!(
            trusted_external_url(AUDIO_SEPARATION_URL),
            Some(AUDIO_SEPARATION_URL)
        );
        assert_eq!(
            trusted_external_url(CHAT_INSIGHT_URL),
            Some(CHAT_INSIGHT_URL)
        );
        assert_eq!(
            trusted_external_url(MARKETING_MASTER_URL),
            Some(MARKETING_MASTER_URL)
        );
        assert_eq!(trusted_external_url(OFFICIAL_GROUP_URL), Some(OFFICIAL_GROUP_URL));
        assert_eq!(
            trusted_external_url(
                "https://www.shineway.tech.attacker.example/biyi/feature/audio"
            ),
            None
        );
        assert_eq!(
            trusted_external_url("http://www.shineway.tech/biyi/feature/audio"),
            None
        );
        assert_eq!(trusted_external_url("https://qm.qq.com.attacker.example/q/ugUpnQFL2M"), None);
        assert_eq!(trusted_external_url("http://qm.qq.com/q/ugUpnQFL2M"), None);
    }
}
