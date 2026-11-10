//! Compatibility metadata from authenticated MSA sessions, not an entitlement.
use irori_msstore_api::models::secrets::User;

fn account_id(puid: &str) -> Option<String> {
    if puid.len() != 16
        || !puid.bytes().all(|c| c.is_ascii_hexdigit())
        || puid.bytes().all(|c| c == b'0')
    {
        return None;
    }
    Some(puid.to_ascii_lowercase())
}

pub fn bound_ids(ticket: Option<&User>, store: Option<&User>) -> (Option<String>, Option<String>) {
    // Callers validate both role snapshots. Different principals are the
    // purpose of separate Store and player accounts, not evidence of a race.
    let valid = |user: Option<&User>| {
        user.filter(|u| !u.username.is_empty())
            .and_then(|u| account_id(&u.puid))
    };
    (valid(ticket), valid(store))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn user(puid: &str) -> User {
        User {
            puid: puid.into(),
            username: "synthetic@example.invalid".into(),
        }
    }
    #[test]
    fn account_metadata_uses_independently_bound_role_ids() {
        let a = user("0123456789ABCDEF");
        let b = user("fedcba9876543210");
        assert_eq!(
            bound_ids(Some(&a), Some(&a)),
            (
                Some("0123456789abcdef".into()),
                Some("0123456789abcdef".into())
            )
        );
        assert_eq!(
            bound_ids(Some(&a), Some(&b)),
            (
                Some("0123456789abcdef".into()),
                Some("fedcba9876543210".into())
            )
        );
        assert_eq!(
            bound_ids(Some(&a), None),
            (Some("0123456789abcdef".into()), None)
        );
        assert_eq!(
            bound_ids(None, Some(&a)),
            (None, Some("0123456789abcdef".into()))
        );
        let renamed = User {
            puid: a.puid.clone(),
            username: "other@example.invalid".into(),
        };
        assert_eq!(
            bound_ids(Some(&a), Some(&renamed)),
            (
                Some("0123456789abcdef".into()),
                Some("0123456789abcdef".into())
            )
        );
    }
    #[test]
    fn account_metadata_rejects_placeholder_malformed_and_empty_ids() {
        for value in [
            "",
            "0",
            "0000000000000000",
            "281474000000000000000",
            "synthetic",
            "0123456789abcdeg",
            "0123456789abcdef\n",
        ] {
            let bad = user(value);
            assert_eq!(bound_ids(Some(&bad), Some(&bad)), (None, None));
        }
    }
    #[test]
    fn account_metadata_xml_is_additive_and_omitted_for_unbound_sessions() {
        use irori_msstore_api::models::xgameruntime::xuser::MSATokenResponse;
        let mut value = MSATokenResponse {
            token: "synthetic-ticket".into(),
            expiry: 1,
            device_rps: "synthetic-device".into(),
            device_expiry: 2,
            xbox_msa_account_id: None,
            store_msa_account_id: None,
        };
        let xml = quick_xml::se::to_string(&value).unwrap();
        assert!(!xml.contains("MsaAccountId"));
        value.xbox_msa_account_id = Some("0123456789abcdef".into());
        value.store_msa_account_id = Some("fedcba9876543210".into());
        let xml = quick_xml::se::to_string(&value).unwrap();
        assert!(xml.contains("<XboxMsaAccountId>0123456789abcdef</XboxMsaAccountId>"));
        assert!(xml.contains("<StoreMsaAccountId>fedcba9876543210</StoreMsaAccountId>"));
    }
}
