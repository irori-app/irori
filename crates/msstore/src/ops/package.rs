use crate::XBOX_LIVE_PACKAGES_PC;
use crate::models::packagespc::{PackageDetails, PackageResponse};
use crate::models::secrets::Token;
use crate::tokens::TokenManager;

pub async fn get_content_id(
    client: &reqwest::Client,
    product: String,
    market: Option<String>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let region = market.unwrap_or("US".into()).to_uppercase();
    let plan = crate::ops::package_plan::resolve(client, &product.to_uppercase(), &region)
        .await
        .map_err(std::io::Error::other)?;
    Ok(plan.select(None).map_err(std::io::Error::other)?)
}

pub async fn get_packages(
    client: &reqwest::Client,
    tokens: &TokenManager,
    content_id: String,
) -> Result<PackageDetails, Box<dyn std::error::Error + Send + Sync>> {
    let dev_token = tokens.get_device_sts_token()?;
    let Token::Legacy(dev_token) = dev_token else {
        return Err(Box::new(std::io::Error::other("Invalid STS token")));
    };
    let snapshot = tokens.user_token_snapshot_role(crate::tokens::AccountRole::License)?;
    let user_token = snapshot.token();
    let Token::Legacy(legacy) = user_token else {
        return Err(Box::new(std::io::Error::other("Unsupported user token")));
    };

    let xsts_token =
        crate::api::xbox::run(client, dev_token, legacy, "http://update.xboxlive.com").await?;

    let response = client
        .get(format!(
            "{XBOX_LIVE_PACKAGES_PC}/GetBasePackage/{content_id}"
        ))
        .header("x-xbl-contract-version", "3")
        .header(
            "Authorization",
            crate::api::xbox::get_xsts_auth_header(xsts_token)?,
        )
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| {
            std::io::Error::other(
                "Microsoft package request failed. Check your connection and retry.",
            )
        })?
        .error_for_status()
        .map_err(|_| {
            std::io::Error::other(
                "Microsoft refused the package request. Check your sign-in and retry.",
            )
        })?;

    let res: PackageResponse = response
        .json()
        .await
        .map_err(|_| std::io::Error::other("Microsoft returned an unreadable package response."))?;

    let PackageResponse::Found(package) = res else {
        return Err(Box::new(std::io::Error::other(
            "Package was not found, is it owned by the user?",
        )));
    };
    tokens.validate_user_token_snapshot(&snapshot)?;
    Ok(package)
}

#[cfg(test)]
mod credential_tests {
    use super::*;
    #[tokio::test]
    async fn credential_missing_device_stops_package_request_without_panic_or_network() {
        let tokens = TokenManager::with_memory();
        let error = get_packages(&reqwest::Client::new(), &tokens, "synthetic".into())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Required Xodus credentials are missing. Please sign in again."
        );
    }
}
