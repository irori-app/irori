use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MSATokenRequest {
    pub client_id: String,
    #[serde(default)]
    pub allow_ui: bool,
    #[serde(default, alias = "MSAFullTrust")]
    pub msa_full_trust: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MSATokenResponse {
    pub token: String,
    pub expiry: i64,
    pub device_rps: String,
    pub device_expiry: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xbox_msa_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_msa_account_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StoreLicenseTokenRequest {
    pub parent_product_id: String,
    #[serde(default, rename = "ProductId")]
    pub product_ids: Vec<String>,
    pub custom_developer_string: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StoreLicenseTokenResponse {
    pub hresult: u32,
    pub token: String,
}
