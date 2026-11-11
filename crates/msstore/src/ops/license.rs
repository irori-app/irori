use std::path::Path;

use crate::licensing::content::{LicenseContentError, lease_policy, parse_lease};
use crate::licensing::splicense::{DeviceKey, SPLicense};
use crate::models::devicecredential::License;
use crate::models::live::ExchangeUserTokenOutcome;
use crate::models::secrets::{Token, User};
use crate::models::soap;
use crate::tokens::TokenManager;

use crate::ops::license_cache::{Holder, LicenseCache, Stored, Unusable, diagnostic, unix_now};

/// Why the online content-license request did not produce a license.
#[derive(Debug)]
pub enum OnlineFailure {
    /// Microsoft could not be reached or answered with a transient service
    /// error. A stored offline license may be used.
    Unreachable(String),
    /// Microsoft answered and refused this account or content. The stored
    /// offline license for the content is removed.
    Refused(String),
    /// Microsoft refused this device (Store device limit). Every stored
    /// offline license is removed.
    DeviceRefused(String),
    /// Credential, sign-in or response problems that offline play must not hide.
    Other(String),
}

impl OnlineFailure {
    pub fn message(&self) -> &str {
        match self {
            Self::Unreachable(message)
            | Self::Refused(message)
            | Self::DeviceRefused(message)
            | Self::Other(message) => message,
        }
    }
}

/// A content license as Microsoft issued it: the key license plus the leases
/// that came with it, both parsed and as the documents Microsoft sent.
pub struct OnlineLicense {
    pub key: License,
    pub leases: Vec<License>,
    pub lease_documents: Vec<String>,
}

/// A content licence ready to unpack keys, and how it was obtained. Xodus
/// reports the two flags through process-wide statics, which overlapping
/// launches in one process would mix up; here they travel with the licence.
pub struct Grant {
    pub device_key: DeviceKey,
    pub license: SPLicense,
    /// The launch continues on the stored offline licence: later online-only
    /// steps (PlayFab authorisation) are skipped instead of waiting for their
    /// own timeouts, and the runtime may sign in from its offline identity.
    pub offline_used: bool,
    /// Microsoft issued an offline lease that was stored: the device is the
    /// account's designated offline device, so the runtime may keep the
    /// signed-in identity for offline use.
    pub lease_stored: bool,
}

/// What the runtime may do with its cached Xbox identity
/// (`XODUS_OFFLINE_IDENTITY`): sign in from it on a launch running on a
/// stored licence (`use`), keep it when Microsoft issued this device an
/// offline lease (`store`), else nothing and the runtime removes it.
pub fn offline_identity_mode(license_used: bool, lease_stored: bool) -> Option<&'static str> {
    if license_used {
        Some("use")
    } else if lease_stored {
        Some("store")
    } else {
        None
    }
}

impl Grant {
    fn online((device_key, license): (DeviceKey, SPLicense), lease_stored: bool) -> Self {
        Self {
            device_key,
            license,
            offline_used: false,
            lease_stored,
        }
    }
}

/// This account on this device registration. Without a stored device
/// registration no offline license is stored or used.
fn holder(tokens: &TokenManager, user: &User) -> Option<Holder> {
    let device = tokens.get_device_license().ok()?;
    Some(Holder {
        puid: user.puid.clone(),
        device_id: registration_id(&device)?,
    })
}

/// Identifies this device registration; it changes when the device is
/// registered again (sign-out with device reset). Microsoft often omits the
/// license's `DeviceId`, which Xodus then stores empty, so the device
/// account's PUID (always issued at registration) stands in, then the
/// hardware id.
fn registration_id(device: &crate::models::secrets::Device) -> Option<String> {
    [
        ("device", &device.device_id),
        ("puid", &device.puid),
        ("hwid", &device.hwid),
    ]
    .into_iter()
    .find(|(_, value)| !value.is_empty())
    .map(|(kind, value)| format!("{kind}:{value}"))
}

/// Fetches the content license online. `cache` enables device-bound offline
/// use: a license that arrives with an offline lease is recorded there and
/// consulted only when Microsoft is unreachable. `offline` states that the
/// caller already found Microsoft unreachable: no request is made and only
/// the stored license can satisfy the launch.
pub async fn get_license(
    client: &reqwest::Client,
    tokens: &TokenManager,
    content_id: String,
    market: String,
    cache: Option<&Path>,
    offline: bool,
) -> std::result::Result<Grant, String> {
    let identity = tokens
        .account_identity_snapshot(crate::tokens::AccountRole::License)
        .map_err(|e| e.to_string())?;
    let user = identity.user().clone();
    let holder = holder(tokens, &user);
    if offline {
        // Skip the ticket and license requests and their connect timeouts;
        // the stored license is held to the same rules as after a failed
        // request, so nothing is granted that an unreachable Microsoft would
        // not have allowed.
        let recovered = recover_offline(
            tokens,
            cache,
            &content_id,
            holder.as_ref(),
            OnlineFailure::Unreachable(
                "Microsoft is unreachable (offline launch requested).".to_string(),
            ),
            unix_now(),
        );
        tokens
            .validate_account_identity(&identity)
            .map_err(|e| e.to_string())?;
        return recovered;
    }
    let snapshot = tokens
        .user_token_snapshot_role(crate::tokens::AccountRole::License)
        .map_err(|e| e.to_string())?;
    tokens
        .validate_account_identity(&identity)
        .map_err(|e| e.to_string())?;
    let result = fetch_online(
        client,
        tokens,
        &user,
        &snapshot,
        content_id.clone(),
        market.clone(),
    )
    .await;
    tokens
        .validate_user_token_snapshot(&snapshot)
        .map_err(|e| e.to_string())?;
    let material = match result {
        Ok(license) => {
            let material = license_material(tokens, &license.key.splicense_block)?;
            let stored = cache.is_some_and(|root| {
                record_license(
                    tokens,
                    root,
                    &content_id,
                    holder.as_ref(),
                    &market,
                    &license,
                    unix_now(),
                )
            });
            Ok(Grant::online(material, stored))
        }
        Err(failure) => recover_offline(
            tokens,
            cache,
            &content_id,
            holder.as_ref(),
            failure,
            unix_now(),
        ),
    };
    tokens
        .validate_account_identity(&identity)
        .map_err(|e| e.to_string())?;
    material
}

/// Microsoft's answer is authoritative for this device: an offline lease is
/// stored; a key license that is not device-bound means this device is not
/// the account's designated offline device, so every stored license ends.
pub fn record_license(
    tokens: &TokenManager,
    root: &Path,
    content_id: &str,
    holder: Option<&Holder>,
    market: &str,
    license: &OnlineLicense,
    now: u64,
) -> bool {
    // Policy metadata only: no identifiers, keys or license blocks.
    let leases: Vec<String> = license
        .leases
        .iter()
        .map(|lease| match lease_policy(lease) {
            Some(policy) => format!(
                "lease offline={} expires={} poll={} enforcement={:?}",
                policy.is_offline,
                policy
                    .expires_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "none".into()),
                policy
                    .poll_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "none".into()),
                policy.lease_enforcement
            ),
            None => "lease without readable policy".to_string(),
        })
        .collect();
    tracing::debug!(
        "Content license policy: type {:?}, usage {:?}, category {:?}, binding {}, lease required {:?}; {} lease(s): {}.",
        license.key.license_info.license_type,
        license.key.license_info.license_usage,
        license.key.license_info.license_category,
        license.key.binding.binding_type,
        license.key.binding.lease_required,
        license.leases.len(),
        if leases.is_empty() {
            "none".to_string()
        } else {
            leases.join("; ")
        },
    );
    let cache = LicenseCache::new(root);
    if !license
        .key
        .binding
        .binding_type
        .eq_ignore_ascii_case("Machine")
    {
        match cache.revoke_all() {
            Ok(0) => {}
            Ok(_) => {
                tracing::debug!(
                    "Microsoft no longer treats this device as the account's offline device; its stored offline licenses were removed."
                );
                diagnostic("content-cache-revoked");
            }
            Err(_) => diagnostic("content-cache-write-failed"),
        }
        tracing::debug!(
            "No offline lease was issued for this device; offline play requires it to be the account's designated offline device."
        );
        diagnostic("content-cache-skipped");
        return false;
    }
    let Some(holder) = holder else {
        let _ = cache.purge(content_id);
        diagnostic("content-cache-skipped");
        return false;
    };
    let Ok(device) = tokens.get_device_license() else {
        diagnostic("content-cache-skipped");
        return false;
    };
    match LicenseCache::authenticated(root, device.password.as_bytes()).store(
        content_id,
        holder,
        market,
        &license.key,
        &license.lease_documents,
        now,
    ) {
        Ok(Stored::Saved) => {
            diagnostic("content-cache-stored");
            true
        }
        Ok(Stored::OnlineOnly) => {
            tracing::debug!("Microsoft issued no offline lease for this game on this device.");
            diagnostic("content-cache-skipped");
            false
        }
        Err(_) => {
            diagnostic("content-cache-write-failed");
            false
        }
    }
}

/// Decides whether a stored offline license may stand in for the failed
/// online request. Only an unreachable service qualifies; refusals revoke.
pub fn recover_offline(
    tokens: &TokenManager,
    cache: Option<&Path>,
    content_id: &str,
    holder: Option<&Holder>,
    failure: OnlineFailure,
    now: u64,
) -> Result<Grant, String> {
    let message = failure.message().to_string();
    match failure {
        OnlineFailure::Refused(_) => {
            if let Some(root) = cache
                && LicenseCache::new(root).purge(content_id) == Ok(true)
            {
                diagnostic("content-cache-purged");
            }
            Err(message)
        }
        OnlineFailure::DeviceRefused(_) => {
            if let Some(root) = cache
                && LicenseCache::new(root)
                    .revoke_all()
                    .is_ok_and(|removed| removed > 0)
            {
                diagnostic("content-cache-revoked");
            }
            Err(message)
        }
        OnlineFailure::Other(_) => Err(message),
        OnlineFailure::Unreachable(_) => {
            let Some(root) = cache else {
                return Err(message);
            };
            let Some(holder) = holder else {
                diagnostic("content-cache-missing");
                return Err(format!(
                    "{message} No device registration is stored, so no offline license can be used. Sign in again while online."
                ));
            };
            let device = tokens.get_device_license().map_err(|_| {
                "Stored device registration is unavailable. Sign in again while online.".to_string()
            })?;
            match LicenseCache::authenticated(root, device.password.as_bytes())
                .load(content_id, holder, now)
            {
                Ok(Ok(entry)) => {
                    diagnostic("content-cache-used");
                    tracing::debug!(
                        "Microsoft content licensing is unreachable; using the offline license stored on this device (its lease allows {} more day(s)).",
                        entry.expires_at.saturating_sub(now).div_ceil(24 * 60 * 60)
                    );
                    license_material(tokens, &entry.splicense_block).map(|(device_key, license)| {
                        Grant {
                            device_key,
                            license,
                            offline_used: true,
                            lease_stored: false,
                        }
                    })
                }
                Ok(Err(Unusable::Clock)) => Err(format!(
                    "{message} The system clock is earlier than this offline license was last used. Correct the date and time, or connect to the internet."
                )),
                Ok(Err(_)) => Err(format!(
                    "{message} No valid offline license is stored for this game. Play it once while online on the account's designated offline device to enable offline play."
                )),
                Err(error) => Err(format!("{message} {error}")),
            }
        }
    }
}

fn transport_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(request) = error.downcast_ref::<reqwest::Error>() {
            return request.is_connect() || request.is_timeout() || request.is_request();
        }
        current = error.source();
    }
    false
}

fn ticket_failure(error: &(dyn std::error::Error + 'static), message: &str) -> OnlineFailure {
    if transport_failure(error) {
        OnlineFailure::Unreachable(message.to_string())
    } else {
        OnlineFailure::Other(message.to_string())
    }
}

fn content_failure(error: LicenseContentError) -> OnlineFailure {
    let message = error.to_string();
    match error {
        LicenseContentError::Timeout
        | LicenseContentError::Request
        | LicenseContentError::Read
        | LicenseContentError::RateLimited
        | LicenseContentError::Server(_) => OnlineFailure::Unreachable(message),
        LicenseContentError::Forbidden | LicenseContentError::NotEntitled => {
            OnlineFailure::Refused(message)
        }
        LicenseContentError::DeviceLimit => OnlineFailure::DeviceRefused(message),
        _ => OnlineFailure::Other(message),
    }
}

async fn fetch_online(
    client: &reqwest::Client,
    tokens: &TokenManager,
    user: &User,
    snapshot: &crate::tokens::UserTokenSnapshot,
    content_id: String,
    market: String,
) -> Result<OnlineLicense, OnlineFailure> {
    let other = |message: String| OnlineFailure::Other(message);
    let dev_token = tokens
        .get_device_sts_token()
        .map_err(|error| other(error.to_string()))?;
    let Token::Legacy(dev_token) = dev_token else {
        return Err(other("Invalid STS token".to_string()));
    };
    let user_token = snapshot.token();
    let Token::Legacy(legacy) = user_token else {
        return Err(other("Unspported user token".to_string()));
    };

    let ms_device_token = crate::api::live::exchange_device_token(
        client,
        dev_token.clone(),
        "{d6d5a677-0872-4ab0-9442-bb792fce85c5}".to_string(),
        "www.microsoft.com".to_owned(),
        Some(soap::PolicyReference::mbi_ssl()),
    )
    .await
    .map_err(|error| {
        ticket_failure(
            &error,
            "Microsoft device-ticket request failed. Check your connection and sign-in.",
        )
    })?;

    let user_token = crate::api::live::exchange_user_token(
        client,
        legacy,
        user.username.clone(),
        dev_token,
        None,
        Some("Silent".to_string()),
        "{d6d5a677-0872-4ab0-9442-bb792fce85c5}".to_string(),
        &[(
            "www.microsoft.com".to_owned(),
            Some(soap::PolicyReference::mbi_ssl()),
        )],
    )
    .await
    .map_err(|error| {
        ticket_failure(
            &error,
            "Microsoft user-ticket request failed. Check your connection and sign-in.",
        )
    })?;

    let ms_device_token =
        Token::try_from(ms_device_token).map_err(|error| other(error.to_string()))?;
    let Token::Compact(ms_device_token) = ms_device_token else {
        return Err(other("Unsupported token".to_string()));
    };

    let user_token: Token = match user_token {
        ExchangeUserTokenOutcome::Fault(_) => {
            return Err(other("Failed to get exchange MS token".to_string()));
        }
        ExchangeUserTokenOutcome::Issued(
            soap::BodyContent::RequestSecurityTokenResponseCollection(collection),
        ) => {
            let token = collection
                .security_tokens
                .into_iter()
                .next()
                .ok_or_else(|| {
                    other("Microsoft returned no content-licensing user ticket.".into())
                })?;
            Token::try_from(token).map_err(|error| other(error.to_string()))?
        }
        ExchangeUserTokenOutcome::Issued(soap::BodyContent::RequestSecurityTokenResponse(
            token,
        )) => Token::try_from(*token).map_err(|error| other(error.to_string()))?,
        _ => {
            return Err(other(
                "Microsoft returned an unsupported content-licensing user ticket.".into(),
            ));
        }
    };
    let Token::Compact(user_token) = user_token else {
        return Err(other("Unsupported token".to_string()));
    };
    let user_token = crate::models::secrets::licensing_ticket(user_token);

    let (content, key) = crate::licensing::content::get_license_content(
        client,
        ms_device_token,
        user_token,
        user.puid.clone(),
        content_id,
        market,
    )
    .await
    .map_err(content_failure)?;

    // Leases are optional policy metadata; an unreadable lease never blocks
    // the launch, it only means no offline permission is recorded.
    let leases = content
        .leases
        .iter()
        .filter_map(|lease| parse_lease(&lease.value).ok())
        .collect();
    // The documents as issued are what the offline cache keeps and validates again.
    let lease_documents = content
        .leases
        .into_iter()
        .map(|lease| lease.value)
        .collect();
    Ok(OnlineLicense {
        key,
        leases,
        lease_documents,
    })
}

fn license_material(tokens: &TokenManager, block: &str) -> Result<(DeviceKey, SPLicense), String> {
    let game_splicense = SPLicense::parse_base64(block).map_err(|_| {
        tracing::debug!("NEXUS_CONTENT_LICENSE_DIAGNOSTIC_V1 content-license-blob 0");
        "Microsoft returned an invalid content-license block. Share the backend diagnostic log."
            .to_string()
    })?;

    let dev_license = tokens
        .get_device_license()
        .map_err(|error| error.to_string())?;
    let device_license = SPLicense::parse_base64(&dev_license.splicense).map_err(|_| {
        "Stored device-license data is invalid. Sign in again in Settings.".to_string()
    })?;
    let key = device_license
        .encrypted_device_key
        .ok_or("Stored device license has no decryption key. Sign in again in Settings.")?
        .derive_device_key();
    Ok((key, game_splicense))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::secrets::Device;
    use base64::{Engine, prelude::BASE64_STANDARD};
    #[test]
    fn content_license_bad_blocks_return_fixed_errors_without_touching_credentials() {
        let tokens = TokenManager::with_memory();
        let error = license_material(&tokens, "private-secret-bad-base64")
            .err()
            .unwrap();
        assert!(error.contains("invalid content-license block"));
        assert!(!error.contains("private-secret"));
        assert!(tokens.get_device_license().is_err());
        let empty = BASE64_STANDARD.encode([0u8; 8]);
        assert!(license_material(&tokens, &empty).is_err());
        tokens
            .save_device_license(&Device {
                username: "synthetic".into(),
                password: "unused".into(),
                puid: "unused".into(),
                hwid: "unchanged".into(),
                device_id: "unchanged".into(),
                splicense: empty.clone(),
            })
            .unwrap();
        assert!(
            license_material(&tokens, &empty)
                .err()
                .unwrap()
                .contains("no decryption key")
        );
        assert_eq!(tokens.get_device_license().unwrap().hwid, "unchanged");
    }

    fn offline_test_tokens() -> TokenManager {
        let tokens = TokenManager::with_memory();
        tokens
            .save_device_license(&Device {
                username: "synthetic".into(),
                password: "unused".into(),
                puid: "unused".into(),
                hwid: "unused".into(),
                device_id: "device-a".into(),
                splicense: BASE64_STANDARD.encode([0u8; 8]),
            })
            .unwrap();
        tokens
    }

    use crate::ops::license_cache::tests::{DAY, holder, key, lease_with, offline_lease, rfc3339};

    /// A structurally valid, empty SPLicense: header and offset only. Using it
    /// proceeds to the keyring device key, which proves the cache was accepted.
    fn empty_block() -> String {
        BASE64_STANDARD.encode([0u8; 8])
    }

    fn issued(binding: &str, leases: Vec<String>) -> OnlineLicense {
        OnlineLicense {
            key: key(binding, &empty_block()),
            leases: leases
                .iter()
                .filter_map(|document| parse_lease(document).ok())
                .collect(),
            lease_documents: leases,
        }
    }

    fn accepted(error: &str) -> bool {
        !error.contains("Could not connect")
            && !error.contains("offline license is stored")
            && !error.contains("system clock")
    }

    fn unreachable() -> OnlineFailure {
        OnlineFailure::Unreachable("Could not connect.".into())
    }

    #[test]
    fn the_designated_device_stores_its_offline_lease_and_uses_it_only_while_unreachable() {
        let tokens = offline_test_tokens();
        let root = tempfile::tempdir().unwrap();
        let now = 1_800_000_000;
        let device_a = holder("puid", "device-a");
        assert!(record_license(
            &tokens,
            root.path(),
            "content-a",
            Some(&device_a),
            "US",
            &issued("Machine", vec![offline_lease(now + 30 * DAY)]),
            now,
        ));
        assert!(root.path().join("content-a.json").exists());

        let error = recover_offline(
            &tokens,
            Some(root.path()),
            "content-a",
            Some(&device_a),
            unreachable(),
            now + DAY,
        )
        .err()
        .unwrap();
        // The stored licence was used: only its (synthetic) key failed.
        assert!(accepted(&error), "{error}");

        // No cache, no device registration or no entry keep the original failure.
        for (cache_root, who, content) in [
            (None, Some(&device_a), "content-a"),
            (Some(root.path()), None, "content-a"),
            (Some(root.path()), Some(&device_a), "content-b"),
        ] {
            let error =
                recover_offline(&tokens, cache_root, content, who, unreachable(), now + DAY)
                    .err()
                    .unwrap();
            assert!(error.starts_with("Could not connect."), "{error}");
        }
        // Sign-in and response problems never consult the cache.
        let error = recover_offline(
            &tokens,
            Some(root.path()),
            "content-a",
            Some(&device_a),
            OnlineFailure::Other("Sign in again.".into()),
            now + DAY,
        )
        .err()
        .unwrap();
        assert_eq!(error, "Sign in again.");
        assert!(root.path().join("content-a.json").exists());
    }

    #[test]
    fn switching_the_designation_from_a_to_b_ends_offline_play_on_a_when_it_reconnects() {
        let tokens = offline_test_tokens();
        let root = tempfile::tempdir().unwrap();
        let now = 1_800_000_000;
        let device_a = holder("puid", "device-a");
        for content in ["content-a", "content-b"] {
            record_license(
                &tokens,
                root.path(),
                content,
                Some(&device_a),
                "US",
                &issued("Machine", vec![offline_lease(now + 30 * DAY)]),
                now,
            );
        }
        // The account designates device B. Device A reconnects and launches any
        // game: Microsoft now issues it a User-bound key and no offline lease.
        record_license(
            &tokens,
            root.path(),
            "content-c",
            Some(&device_a),
            "US",
            &issued("User", vec![]),
            now + 2 * DAY,
        );
        for content in ["content-a", "content-b", "content-c"] {
            assert!(
                !root.path().join(format!("{content}.json")).exists(),
                "{content}"
            );
        }
        // Going offline again, nothing stored on A grants access.
        let error = recover_offline(
            &tokens,
            Some(root.path()),
            "content-a",
            Some(&device_a),
            unreachable(),
            now + 3 * DAY,
        )
        .err()
        .unwrap();
        assert!(
            error.contains("No valid offline license is stored"),
            "{error}"
        );

        // A Machine-bound key without an offline lease keeps the other games but not this one.
        record_license(
            &tokens,
            root.path(),
            "content-d",
            Some(&device_a),
            "US",
            &issued("Machine", vec![offline_lease(now + 30 * DAY)]),
            now,
        );
        record_license(
            &tokens,
            root.path(),
            "content-e",
            Some(&device_a),
            "US",
            &issued("Machine", vec![offline_lease(now + 30 * DAY)]),
            now,
        );
        let online = lease_with(
            &format!(
                "{{\"isOffline\":false,\"expiresAt\":\"{}\"}}",
                rfc3339(now + DAY)
            ),
            "synthetic-device",
            None,
        );
        record_license(
            &tokens,
            root.path(),
            "content-e",
            Some(&device_a),
            "US",
            &issued("Machine", vec![online]),
            now,
        );
        assert!(root.path().join("content-d.json").exists());
        assert!(!root.path().join("content-e.json").exists());
    }

    #[test]
    fn expired_leases_fail_closed_and_disappear() {
        let tokens = offline_test_tokens();
        let root = tempfile::tempdir().unwrap();
        let now = 1_800_000_000;
        let device_a = holder("puid", "device-a");
        record_license(
            &tokens,
            root.path(),
            "content-a",
            Some(&device_a),
            "US",
            &issued("Machine", vec![offline_lease(now + 2 * DAY)]),
            now,
        );
        let error = recover_offline(
            &tokens,
            Some(root.path()),
            "content-a",
            Some(&device_a),
            unreachable(),
            now + 2 * DAY,
        )
        .err()
        .unwrap();
        assert!(error.contains("offline license is stored"), "{error}");
        assert!(!root.path().join("content-a.json").exists());
    }

    #[test]
    fn an_account_change_removes_every_stored_license() {
        let tokens = offline_test_tokens();
        let root = tempfile::tempdir().unwrap();
        let now = 1_800_000_000;
        let first = holder("puid-a", "device-a");
        record_license(
            &tokens,
            root.path(),
            "content-a",
            Some(&first),
            "US",
            &issued("Machine", vec![offline_lease(now + 30 * DAY)]),
            now,
        );
        let error = recover_offline(
            &tokens,
            Some(root.path()),
            "content-a",
            Some(&holder("puid-b", "device-a")),
            unreachable(),
            now + DAY,
        )
        .err()
        .unwrap();
        assert!(error.contains("offline license is stored"), "{error}");
        assert!(!root.path().join("content-a.json").exists());
        let error = recover_offline(
            &tokens,
            Some(root.path()),
            "content-a",
            Some(&first),
            unreachable(),
            now + DAY,
        )
        .err()
        .unwrap();
        assert!(error.contains("offline license is stored"), "{error}");
    }

    #[test]
    fn explicit_licensing_refusals_remove_stored_licenses() {
        let tokens = offline_test_tokens();
        let root = tempfile::tempdir().unwrap();
        let now = 1_800_000_000;
        let device_a = holder("puid", "device-a");
        for content in ["content-a", "content-b", "content-c"] {
            record_license(
                &tokens,
                root.path(),
                content,
                Some(&device_a),
                "US",
                &issued("Machine", vec![offline_lease(now + 30 * DAY)]),
                now,
            );
        }
        // Not entitled to one game: only that game's license goes.
        let error = recover_offline(
            &tokens,
            Some(root.path()),
            "content-a",
            Some(&device_a),
            content_failure(LicenseContentError::NotEntitled),
            now,
        )
        .err()
        .unwrap();
        assert!(error.contains("did not grant a license"), "{error}");
        assert!(!root.path().join("content-a.json").exists());
        assert!(root.path().join("content-b.json").exists());
        // Forbidden is a refusal of the same kind.
        recover_offline(
            &tokens,
            Some(root.path()),
            "content-b",
            Some(&device_a),
            content_failure(LicenseContentError::Forbidden),
            now,
        )
        .err()
        .unwrap();
        assert!(!root.path().join("content-b.json").exists());
        // The Store refuses this device: nothing stored on it may be used.
        record_license(
            &tokens,
            root.path(),
            "content-b",
            Some(&device_a),
            "US",
            &issued("Machine", vec![offline_lease(now + 30 * DAY)]),
            now,
        );
        let error = recover_offline(
            &tokens,
            Some(root.path()),
            "content-a",
            Some(&device_a),
            content_failure(LicenseContentError::DeviceLimit),
            now,
        )
        .err()
        .unwrap();
        assert!(error.contains("device limit"), "{error}");
        for content in ["content-b", "content-c"] {
            assert!(
                !root.path().join(format!("{content}.json")).exists(),
                "{content}"
            );
        }
        assert!(
            recover_offline(
                &tokens,
                None,
                "content-c",
                Some(&device_a),
                OnlineFailure::Refused("Not entitled.".into()),
                now
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn offline_launch_never_contacts_microsoft_and_uses_only_the_stored_license() {
        let tokens = offline_test_tokens();
        tokens
            .save_user(&crate::models::secrets::User {
                puid: "puid".into(),
                username: "user@example.com".into(),
            })
            .unwrap();
        tokens
            .save_device_license(&Device {
                username: "synthetic".into(),
                password: "unused".into(),
                puid: "unused".into(),
                hwid: "unused".into(),
                device_id: "device-a".into(),
                splicense: empty_block(),
            })
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let now = unix_now();
        record_license(
            &tokens,
            root.path(),
            "content-a",
            Some(&holder("puid", "device:device-a")),
            "US",
            &issued("Machine", vec![offline_lease(now + 20 * DAY)]),
            now,
        );
        // Any request would have to connect within a millisecond and fail: the
        // offline path must never issue one.
        let client = reqwest::ClientBuilder::new()
            .connect_timeout(std::time::Duration::from_millis(1))
            .build()
            .unwrap();
        let error = get_license(
            &client,
            &tokens,
            "content-a".into(),
            "US".into(),
            Some(root.path()),
            true,
        )
        .await
        .err()
        .unwrap();
        assert!(!error.contains("offline launch requested"), "{error}");
        assert!(root.path().join("content-a.json").exists());

        // Without a stored license the launch fails closed and says why.
        let error = get_license(
            &client,
            &tokens,
            "content-b".into(),
            "US".into(),
            Some(root.path()),
            true,
        )
        .await
        .err()
        .unwrap();
        assert!(
            error.starts_with("Microsoft is unreachable (offline launch requested)."),
            "{error}"
        );
        assert!(
            error.contains("No valid offline license is stored"),
            "{error}"
        );
        let error = get_license(
            &client,
            &tokens,
            "content-a".into(),
            "US".into(),
            None,
            true,
        )
        .await
        .err()
        .unwrap();
        assert!(
            error.starts_with("Microsoft is unreachable (offline launch requested)."),
            "{error}"
        );
        // A different device registration on this machine cannot use it.
        tokens
            .save_device_license(&Device {
                username: "synthetic".into(),
                password: "unused".into(),
                puid: "unused".into(),
                hwid: "unused".into(),
                device_id: "device-b".into(),
                splicense: empty_block(),
            })
            .unwrap();
        let error = get_license(
            &client,
            &tokens,
            "content-a".into(),
            "US".into(),
            Some(root.path()),
            true,
        )
        .await
        .err()
        .unwrap();
        assert!(
            error.contains("No valid offline license is stored"),
            "{error}"
        );
        assert!(!root.path().join("content-a.json").exists());
    }

    #[test]
    fn the_device_registration_is_identified_even_without_a_license_device_id() {
        let device = |device_id: &str, puid: &str, hwid: &str| Device {
            username: "synthetic".into(),
            password: "unused".into(),
            puid: puid.into(),
            hwid: hwid.into(),
            device_id: device_id.into(),
            splicense: empty_block(),
        };
        assert_eq!(
            registration_id(&device("dev-a", "puid-a", "hw-a")).as_deref(),
            Some("device:dev-a")
        );
        // Microsoft omitted DeviceId (seen on a real registration): the device account's PUID identifies it.
        assert_eq!(
            registration_id(&device("", "puid-a", "hw-a")).as_deref(),
            Some("puid:puid-a")
        );
        assert_eq!(
            registration_id(&device("", "", "hw-a")).as_deref(),
            Some("hwid:hw-a")
        );
        assert_eq!(registration_id(&device("", "", "")), None);
        // A new registration is a different holder, so its predecessor's licenses are never used.
        assert_ne!(
            registration_id(&device("", "puid-a", "hw-a")),
            registration_id(&device("", "puid-b", "hw-a"))
        );

        let tokens = TokenManager::with_memory();
        let user = User {
            puid: "user".into(),
            username: "user@example.com".into(),
        };
        assert!(super::holder(&tokens, &user).is_none());
        tokens
            .save_device_license(&device("", "puid-a", "hw-a"))
            .unwrap();
        assert_eq!(
            super::holder(&tokens, &user),
            Some(crate::ops::license_cache::Holder {
                puid: "user".into(),
                device_id: "puid:puid-a".into()
            })
        );
    }

    #[test]
    fn online_failures_are_classified_without_inferring_entitlement() {
        for error in [
            LicenseContentError::Timeout,
            LicenseContentError::Request,
            LicenseContentError::Read,
            LicenseContentError::RateLimited,
            LicenseContentError::Server(503),
        ] {
            assert!(matches!(
                content_failure(error),
                OnlineFailure::Unreachable(_)
            ));
        }
        for error in [
            LicenseContentError::Forbidden,
            LicenseContentError::NotEntitled,
        ] {
            assert!(matches!(content_failure(error), OnlineFailure::Refused(_)));
        }
        assert!(matches!(
            content_failure(LicenseContentError::DeviceLimit),
            OnlineFailure::DeviceRefused(_)
        ));
        for error in [
            LicenseContentError::Unauthorized,
            LicenseContentError::Http(418),
            LicenseContentError::Json,
            LicenseContentError::Schema,
            LicenseContentError::EmptyKeys,
            LicenseContentError::Size,
            LicenseContentError::Xml,
        ] {
            assert!(matches!(content_failure(error), OnlineFailure::Other(_)));
        }
        let local = std::io::Error::other("not a transport failure");
        assert!(!transport_failure(&local));
        assert!(
            matches!(ticket_failure(&local, "ticket"), OnlineFailure::Other(message) if message == "ticket")
        );
    }
}
