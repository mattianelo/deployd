use anyhow::Result;

use crate::core::nexus_api::{NexusClient, RateLimitInfo};
use crate::core::tracker::Tracker;

#[derive(Debug, Default)]
pub(crate) struct Account {
    pub(super) username: Option<String>,
    pub(super) avatar_url: Option<String>,
    pub(super) premium: bool,
    pub(super) rate_limit: Option<RateLimitInfo>,
}

pub(super) async fn cached_account(
    tracker: &Tracker,
    warnings: &mut Vec<String>,
) -> Result<Account> {
    let rate_limit = optional(
        tracker.load_rate_limits().await,
        "Could not load cached Nexus rate limits",
        warnings,
    )
    .flatten();
    let signed_in = tracker
        .get_setting("nexus_api_key")
        .await?
        .is_some_and(|key| !key.is_empty());
    if !signed_in {
        return Ok(Account {
            rate_limit,
            ..Account::default()
        });
    }
    let (username, avatar_url) = optional(
        tracker.load_nexus_user().await,
        "Could not load cached Nexus user details",
        warnings,
    )
    .unwrap_or_default();
    let premium = tracker
        .get_setting("nexus_is_premium")
        .await?
        .is_some_and(|value| value == "true");
    Ok(Account {
        username,
        avatar_url,
        premium,
        rate_limit,
    })
}

pub(super) async fn refresh_account(tracker: &Tracker) -> Result<Option<Account>> {
    let Some(key) = tracker
        .get_setting("nexus_api_key")
        .await?
        .filter(|key| !key.is_empty())
    else {
        return Ok(None);
    };
    let client = NexusClient::new(key.clone());
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(20), client.validate_key()).await;
    let Ok(Ok((user, rate_limit))) = result else {
        return Ok(None);
    };
    if !tracker
        .cache_nexus_account_if_current(&key, &user, rate_limit.as_ref())
        .await?
    {
        return Ok(None);
    }
    Ok(Some(Account {
        username: Some(user.name),
        avatar_url: user.profile_url,
        premium: user.is_premium,
        rate_limit,
    }))
}

pub(super) fn optional<T, E: std::fmt::Display>(
    result: Result<T, E>,
    description: &str,
    warnings: &mut Vec<String>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            warnings.push(format!("{description}: {error}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;
    use crate::models::nexus::NexusUser;

    fn user() -> NexusUser {
        NexusUser {
            user_id: 1,
            name: "Cached user".into(),
            is_premium: true,
            is_supporter: false,
            profile_url: Some("https://example.invalid/avatar".into()),
        }
    }

    // @variants: both
    #[tokio::test]
    async fn startup_reads_cached_account_without_validating_credentials() -> Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        tracker
            .set_setting("nexus_api_key", "offline-test-key")
            .await?;
        let rate_limit = RateLimitInfo {
            hourly_remaining: 42,
            hourly_limit: 100,
            daily_remaining: 200,
            daily_limit: 1000,
        };
        assert!(
            tracker
                .cache_nexus_account_if_current("offline-test-key", &user(), Some(&rate_limit))
                .await?
        );
        let mut warnings = Vec::new();
        let account = cached_account(&tracker, &mut warnings).await?;
        assert_eq!(account.username.as_deref(), Some("Cached user"));
        assert!(account.premium);
        assert_eq!(
            account.rate_limit.context("Rate limits")?.hourly_remaining,
            42
        );
        assert!(warnings.is_empty());
        tracker.clear_nexus_user().await?;
        let signed_out = cached_account(&tracker, &mut warnings).await?;
        assert!(signed_out.username.is_none());
        assert!(!signed_out.premium);
        Ok(())
    }
}
