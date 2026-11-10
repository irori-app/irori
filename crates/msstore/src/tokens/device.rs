use crate::hardware;
use crate::licensing::splicense::SPLicense;
use crate::licensing::utils::{generate_string, parse_bcrypt_rsa_private};
use crate::models::devicecredential::{Authentication, ClientInfo, DeviceAddRequest, DeviceInfo};
use crate::models::secrets::Device;
use crate::models::soap::BodyContent;
use crate::tokens::manager::TokenManager;
use crate::tokens::store::TokenStoreError;

/// Provisions a device (if none is stored yet) or re-authenticates an existing one
/// (if its STS token is missing/expired), persisting the result through `tokens`.
pub async fn ensure_device_credentials(
    client: &reqwest::Client,
    tokens: &TokenManager,
) -> Result<(), TokenStoreError> {
    match tokens.get_device_license() {
        Err(TokenStoreError::NotFound) => provision_device(client, tokens).await?,
        Err(error) => return Err(error),
        Ok(license) => match tokens.get_device_sts_token() {
            Ok(token)
                if super::session::state(&token, chrono::Utc::now())
                    == super::session::State::Current => {}
            Ok(_) => reauthenticate_device(client, tokens, license, true).await?,
            Err(TokenStoreError::NotFound | TokenStoreError::Serde(_)) => {
                reauthenticate_device(client, tokens, license, true).await?;
            }
            Err(error) => return Err(error),
        },
    }
    // Network success alone does not prove that a usable token was saved.
    tokens.get_device_sts_token()?;
    Ok(())
}

/// Renew only an already registered device. Optional reads never provision one.
pub async fn refresh_existing_device_credentials(
    client: &reqwest::Client,
    tokens: &TokenManager,
) -> Result<(), TokenStoreError> {
    let license = tokens.get_device_license()?;
    reauthenticate_device(client, tokens, license, true).await
}

async fn provision_device(
    client: &reqwest::Client,
    tokens: &TokenManager,
) -> Result<(), TokenStoreError> {
    let username = format!("02{}", generate_string(14));
    let password = generate_string(20);
    let provision = DeviceAddRequest {
        client_info: ClientInfo::default(),
        authentication: Authentication::new(username.clone(), password.clone()),
        device_info: Some(DeviceInfo {
            id: "DeviceInfo".to_string(),
            components: hardware::probe_provision_components(),
            tpm_info: None,
        }),
    };

    let dev = crate::api::live::login_device_credential(client, provision)
        .await
        .map_err(|_| TokenStoreError::DeviceAuthentication)?;

    let device = Device {
        username: username.clone(),
        password: password.clone(),
        puid: dev.puid,
        hwid: dev.hw_device_id,
        device_id: dev.license.binding.device_id.unwrap_or_default(),
        splicense: dev.license.splicense_block,
    };

    tokens.save_device_license(&device)?;

    reauthenticate_device(client, tokens, device, false).await
}

async fn reauthenticate_device(
    client: &reqwest::Client,
    tokens: &TokenManager,
    license: Device,
    reuse_current: bool,
) -> Result<(), TokenStoreError> {
    // Shared by service, login and optional reads. Waiters reuse genuine renewal.
    let _guard = super::session::renewal_lock().await;
    if !same_device(&tokens.get_device_license()?, &license) {
        return Err(TokenStoreError::DeviceAuthentication);
    }
    if reuse_current
        && tokens.get_device_sts_token().is_ok_and(|token| {
            super::session::state(&token, chrono::Utc::now()) == super::session::State::Current
        })
    {
        return Ok(());
    }
    let sp_license = SPLicense::parse_base64(&license.splicense)
        .map_err(|_| TokenStoreError::DeviceAuthentication)?;
    let clep_sign_state = sp_license
        .clep_sign_state
        .ok_or(TokenStoreError::DeviceAuthentication)?;
    let key = clep_sign_state.get_rsa_key();
    let private_key =
        parse_bcrypt_rsa_private(&key).map_err(|_| TokenStoreError::DeviceAuthentication)?;
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        crate::api::live::authenticate_device(client, license.username.clone(), private_key),
    )
    .await
    .map_err(|_| TokenStoreError::DeviceAuthentication)?
    .map_err(|_| TokenStoreError::DeviceAuthentication)?;

    if let BodyContent::RequestSecurityTokenResponse(resp) = resp.body.body {
        // An explicit sign-out/device replacement must not be undone by a late response.
        let current = tokens.get_device_license()?;
        if !same_device(&current, &license) {
            return Err(TokenStoreError::DeviceAuthentication);
        }
        save_device_sts_token(tokens, resp)
    } else {
        Err(TokenStoreError::DeviceAuthentication)
    }
}

fn same_device(left: &Device, right: &Device) -> bool {
    left.username == right.username
        && left.device_id == right.device_id
        && left.hwid == right.hwid
        && left.puid == right.puid
        && left.password == right.password
        && left.splicense == right.splicense
}

fn save_device_sts_token(
    tokens: &TokenManager,
    resp: Box<crate::models::soap::RequestSecurityTokenResponse>,
) -> Result<(), TokenStoreError> {
    let key_name = resp
        .requested_security_token
        .encrypted_data
        .as_ref()
        .ok_or(TokenStoreError::DeviceAuthentication)?
        .key_info
        .key_name
        .as_ref()
        .ok_or(TokenStoreError::DeviceAuthentication)?
        .clone();
    let token = crate::models::secrets::Token::try_from(*resp)
        .map_err(|_| TokenStoreError::DeviceAuthentication)?;
    if super::session::state(&token, chrono::Utc::now()) != super::session::State::Current {
        return Err(TokenStoreError::DeviceAuthentication);
    }
    tokens.save_device_token(key_name, token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        secrets::{LegacyToken, Token},
        soap::Timestamp,
    };

    fn identity() -> Device {
        Device {
            username: "synthetic-device".into(),
            password: "unused".into(),
            puid: "unused".into(),
            hwid: "stable-hwid".into(),
            device_id: "stable-id".into(),
            splicense: "not-a-license".into(),
        }
    }
    #[tokio::test]
    async fn credential_shared_refresh_reuses_current_without_network_or_identity_change() {
        let tokens = TokenManager::with_memory();
        tokens.save_device_license(&identity()).unwrap();
        tokens
            .save_device_token(
                crate::tokens::PASSPORT_STS.into(),
                Token::Legacy(LegacyToken {
                    key_name: None,
                    token: "synthetic-unused".into(),
                    binary_secret: None,
                    tpm_key: None,
                    lifetime: Timestamp {
                        id: None,
                        created: "2000-01-01T00:00:00Z".into(),
                        expires: "2099-01-01T00:00:00Z".into(),
                    },
                }),
            )
            .unwrap();
        let client = reqwest::Client::new();
        refresh_existing_device_credentials(&client, &tokens)
            .await
            .unwrap();
        ensure_device_credentials(&client, &tokens).await.unwrap();
        assert!(same_device(
            &identity(),
            &tokens.get_device_license().unwrap()
        ));
        let mut stale = identity();
        stale.hwid = "replaced".into();
        assert!(
            reauthenticate_device(&client, &tokens, stale, true)
                .await
                .is_err()
        );
        assert!(same_device(
            &identity(),
            &tokens.get_device_license().unwrap()
        ));
    }
    #[tokio::test]
    async fn credential_shared_refresh_never_provisions_missing_or_invalid_identity() {
        let tokens = TokenManager::with_memory();
        let client = reqwest::Client::new();
        assert!(
            refresh_existing_device_credentials(&client, &tokens)
                .await
                .is_err()
        );
        assert!(matches!(
            tokens.get_device_license(),
            Err(TokenStoreError::NotFound)
        ));
        tokens.save_device_license(&identity()).unwrap();
        assert!(
            refresh_existing_device_credentials(&client, &tokens)
                .await
                .is_err()
        );
        assert!(same_device(
            &identity(),
            &tokens.get_device_license().unwrap()
        ));
        assert!(tokens.get_device_sts_token().is_err());
    }
}
