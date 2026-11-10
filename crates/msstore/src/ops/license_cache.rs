//! Device-bound offline license cache for installed games.
//!
//! Microsoft grants offline play only to the account's designated offline
//! device: the content license then arrives with a device-bound *lease* whose
//! policy says `isOffline` and carries an expiry (30 days, renewed on every
//! online launch). This cache stores the encrypted SPLicense block together
//! with that lease document exactly as Microsoft issued it. The block's
//! content keys stay wrapped by the device key in the system keyring, so the
//! file alone decrypts nothing.
//!
//! Nothing Nexus writes itself decides whether an entry may be used. Every
//! load parses the stored lease again and enforces Microsoft's restrictions:
//! an offline lease of type Lease, its own expiry and expiration date, its
//! device binding against the key license, and this device and account. The
//! clock may not move behind the last use, and no entry outlives the fixed
//! offline window after Microsoft issued it.
//!
//! Every revocation Nexus learns about removes entries: Microsoft reporting
//! this device as not the designated offline device (an online license
//! without a device-bound key, or the offline-PC status reading `false`), a
//! designation change, another account, and licensing refusals. A device that
//! stays disconnected cannot hear about a revocation; its access stays bounded
//! by the lease Microsoft issued.
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::licensing::content::{lease_policy, parse_lease};
use crate::licensing::offline_integrity;
use crate::models::devicecredential::{License, LicenseType};
use serde_json::{Value, json};

const SCHEMA: u64 = 4;
const MAX_FILE_BYTES: u64 = 160 * 1024;
const MAX_BLOCK_CHARS: usize = 48 * 1024;
const MAX_LEASE_CHARS: usize = 96 * 1024;
/// Not `*.json`, so no content identifier can name it.
const OWNER_FILE: &str = "owner.holder";
/// Upper bound on offline use after Microsoft issued the lease, independent of
/// the lease's own expiry.
pub const MAX_OFFLINE_SECONDS: u64 = 30 * 24 * 60 * 60;
/// Backwards clock movement tolerated before a stored license is refused.
pub const CLOCK_SKEW_SECONDS: u64 = 10 * 60;

pub fn diagnostic(stage: &'static str) {
    tracing::debug!("NEXUS_CONTENT_LICENSE_DIAGNOSTIC_V1 {stage} 0");
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Who an entry belongs to: the signed-in account and this device's Xbox
/// device registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    pub puid: String,
    pub device_id: String,
}

/// Microsoft's verdict on a stored lease, re-derived from the lease itself.
#[derive(Debug, PartialEq, Eq)]
pub enum LeaseCheck {
    /// Usable offline until this Unix time.
    Valid(u64),
    Expired,
    /// Not an offline lease for this key license, or unreadable. The reason
    /// is a fixed word for diagnostics, never license content.
    Invalid(&'static str),
}

/// Applies Microsoft's restrictions to a lease document as issued: a lease
/// license whose policy allows offline use, before its own expiry and
/// expiration date, bound to the same device as the key license. The result
/// is never later than the offline window after `issued_at`.
pub fn check_lease(document: &str, key: &License, issued_at: u64, now: u64) -> LeaseCheck {
    if document.is_empty() || document.len() > MAX_LEASE_CHARS {
        return LeaseCheck::Invalid("size");
    }
    let Ok(lease) = parse_lease(document) else {
        return LeaseCheck::Invalid("unreadable");
    };
    if lease.license_info.license_type != LicenseType::Lease {
        return LeaseCheck::Invalid("not-a-lease");
    }
    // Microsoft marks the designated device's key license as Machine-bound and
    // lease-required; a User-bound key is never usable offline.
    if !key.binding.binding_type.eq_ignore_ascii_case("Machine")
        || !key
            .binding
            .lease_required
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    {
        return LeaseCheck::Invalid("key-not-device-bound");
    }
    if let (Some(lease_device), Some(key_device)) =
        (&lease.binding.device_id, &key.binding.device_id)
        && lease_device != key_device
    {
        return LeaseCheck::Invalid("other-device");
    }
    let Some(policy) = lease_policy(&lease) else {
        return LeaseCheck::Invalid("no-policy");
    };
    if !policy.is_offline {
        return LeaseCheck::Invalid("not-offline");
    }
    let Some(expires_at) = policy
        .expires_at
        .map(|time| time.timestamp())
        .filter(|seconds| *seconds > 0)
    else {
        return LeaseCheck::Invalid("no-expiry");
    };
    let mut until = expires_at as u64;
    // The lease cannot extend the underlying key's validity (for example a
    // trial) or start before either server-issued validity window begins.
    for info in [&lease.license_info, &key.license_info] {
        if let Some(begin) = info.begin_date.as_deref() {
            let Some(begin) = lease_time(begin) else {
                return LeaseCheck::Invalid("begin-date");
            };
            if begin > now as i64 {
                return LeaseCheck::Invalid("not-yet-valid");
            }
        }
        if let Some(expiration) = info.expiration_date.as_deref() {
            let Some(parsed) = lease_time(expiration) else {
                tracing::debug!(
                    "Unrecognised lease expiration date format: {}.",
                    date_shape(expiration)
                );
                return LeaseCheck::Invalid("expiration-date");
            };
            until = until.min(parsed.max(0) as u64);
        }
    }
    until = until.min(issued_at.saturating_add(MAX_OFFLINE_SECONDS));
    if until <= now {
        return LeaseCheck::Expired;
    }
    LeaseCheck::Valid(until)
}

/// A lease date as Unix seconds. Microsoft writes RFC 3339 in the policy and a
/// UTC date-time without an offset (seven fractional digits) in `LicenseInfo`.
pub fn lease_time(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(time.timestamp());
    }
    [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%m/%d/%Y %H:%M:%S",
        "%m/%d/%Y %I:%M:%S %p",
    ]
    .iter()
    .find_map(|format| chrono::NaiveDateTime::parse_from_str(text, format).ok())
    .map(|time| time.and_utc().timestamp())
}

/// The layout of an unrecognised date, digits masked, for diagnostics only.
fn date_shape(text: &str) -> String {
    text.chars()
        .take(40)
        .map(|c| {
            if c.is_ascii_digit() {
                '9'
            } else if c.is_ascii_alphabetic() || "-:./ +".contains(c) {
                c
            } else {
                '?'
            }
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Stored {
    Saved,
    /// Microsoft attached no usable offline lease for this content; nothing
    /// is kept for it.
    OnlineOnly,
}

pub struct CachedLicense {
    pub splicense_block: String,
    pub expires_at: u64,
}

/// Why no stored license is usable. Each outcome has already been reported
/// as a diagnostic and acted on (removed entries stay removed).
#[derive(Debug, PartialEq, Eq)]
pub enum Unusable {
    Missing,
    Expired,
    Invalid,
    /// The stored licenses belonged to another account or device registration
    /// and were all removed.
    Account,
    /// The clock is behind the last use of this license.
    Clock,
}

pub struct LicenseCache<'a> {
    root: PathBuf,
    secret: Option<&'a [u8]>,
}

impl<'a> LicenseCache<'a> {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            secret: None,
        }
    }

    pub fn authenticated(root: &Path, secret: &'a [u8]) -> Self {
        Self {
            root: root.to_path_buf(),
            secret: Some(secret),
        }
    }

    fn seal(&self, value: &mut Value) -> Result<(), String> {
        value
            .as_object_mut()
            .ok_or_else(cache_io_error)?
            .remove("integrity");
        let tag = offline_integrity::seal(
            self.secret.ok_or_else(cache_io_error)?,
            value.to_string().as_bytes(),
        )
        .ok_or_else(cache_io_error)?;
        value["integrity"] = json!(tag);
        Ok(())
    }

    fn authentic(&self, value: &mut Value) -> bool {
        let Some(object) = value.as_object_mut() else {
            return false;
        };
        let Some(Value::String(tag)) = object.remove("integrity") else {
            return false;
        };
        self.secret.is_some_and(|secret| {
            offline_integrity::verify(secret, value.to_string().as_bytes(), &tag)
        })
    }

    fn entry_path(&self, content_id: &str) -> Result<PathBuf, String> {
        let valid = !content_id.is_empty()
            && content_id.len() <= 64
            && content_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        if !valid {
            return Err(
                "The content identifier is not valid for the offline license cache.".to_string(),
            );
        }
        Ok(self
            .root
            .join(format!("{}.json", content_id.to_ascii_lowercase())))
    }

    fn ensure_root(&self) -> std::io::Result<()> {
        if fs::symlink_metadata(&self.root).is_ok_and(|metadata| !metadata.is_dir()) {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&self.root)
    }

    fn owner(&self) -> Option<Holder> {
        let bytes = read_bounded(&self.root.join(OWNER_FILE), 4096).ok()?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        Some(Holder {
            puid: value.get("puid")?.as_str()?.to_string(),
            device_id: value.get("deviceId")?.as_str()?.to_string(),
        })
    }

    /// Removes every stored license (and the owner record). Returns how many
    /// licenses were removed.
    pub fn revoke_all(&self) -> Result<usize, String> {
        match fs::symlink_metadata(&self.root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Ok(metadata) if metadata.is_dir() => {}
            _ => return Err(cache_io_error()),
        }
        // Revocation survives partial deletion and process exit. It grants
        // nothing and is removed only once every previous entry is gone.
        let revoked = self.root.join(".revoked");
        write_atomic(&revoked, b"revoked\n")?;
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(_) => return Err(cache_io_error()),
        };
        let mut removed = 0;
        let mut failed = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let license = name.ends_with(".json");
            if !(license || name.ends_with(".tmp") || name == OWNER_FILE) {
                continue;
            }
            match fs::remove_file(entry.path()) {
                Ok(()) => removed += usize::from(license),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => failed = true,
            }
        }
        if failed {
            return Err(cache_io_error());
        }
        fs::remove_file(revoked).map_err(|_| cache_io_error())?;
        Ok(removed)
    }

    /// Licenses another account or device registration left behind are never
    /// used; they are removed before this holder's are read or written.
    fn claim(&self, holder: &Holder) -> Result<bool, String> {
        if self.owner().as_ref() == Some(holder) && !self.root.join(".revoked").exists() {
            return Ok(false);
        }
        let removed = self.revoke_all()?;
        self.ensure_root().map_err(|_| cache_io_error())?;
        let owner = json!({ "schema": SCHEMA, "puid": holder.puid, "deviceId": holder.device_id });
        write_atomic(&self.root.join(OWNER_FILE), owner.to_string().as_bytes())?;
        Ok(removed > 0)
    }

    /// Records a license Microsoft just issued with the lease documents that
    /// came with it. Only an offline lease that passes `check_lease` is kept;
    /// otherwise any earlier entry for the same content is removed.
    pub fn store(
        &self,
        content_id: &str,
        holder: &Holder,
        market: &str,
        key: &License,
        leases: &[String],
        now: u64,
    ) -> Result<Stored, String> {
        self.ensure_root().map_err(|_| cache_io_error())?;
        let path = self.entry_path(content_id)?;
        if self.claim(holder)? {
            diagnostic("content-cache-account");
        }
        // The earliest-expiring valid offline lease bounds offline use.
        let mut refused = Vec::new();
        let lease = leases
            .iter()
            .filter_map(|document| match check_lease(document, key, now, now) {
                LeaseCheck::Valid(until) => Some((until, document)),
                LeaseCheck::Expired => {
                    refused.push("expired");
                    None
                }
                LeaseCheck::Invalid(reason) => {
                    refused.push(reason);
                    None
                }
            })
            .min_by_key(|(until, _)| *until);
        let Some((_, lease)) = lease else {
            if !refused.is_empty() {
                tracing::debug!("No usable offline lease: {}.", refused.join(", "));
            }
            remove_quietly(&path);
            return Ok(Stored::OnlineOnly);
        };
        if key.splicense_block.is_empty() || key.splicense_block.len() > MAX_BLOCK_CHARS {
            remove_quietly(&path);
            return Err("The content license block is not cacheable.".to_string());
        }
        let mut entry = json!({
            "schema": SCHEMA,
            "contentId": content_id.to_ascii_lowercase(),
            "puid": holder.puid,
            "deviceId": holder.device_id,
            "market": market,
            "issuedAt": now,
            "lastUsedAt": now,
            "keyBinding": {
                "type": key.binding.binding_type,
                "deviceId": key.binding.device_id,
                "leaseRequired": key.binding.lease_required,
            },
            "keyValidity": {
                "beginDate": key.license_info.begin_date,
                "expirationDate": key.license_info.expiration_date,
            },
            "lease": lease,
            "splicenseBlock": key.splicense_block,
        });
        self.seal(&mut entry)?;
        write_atomic(&path, entry.to_string().as_bytes())?;
        Ok(Stored::Saved)
    }

    /// Returns the stored license for this holder and content when its lease
    /// still allows offline use now. Everything else is reported and, where
    /// it can never become valid again, removed.
    pub fn load(
        &self,
        content_id: &str,
        holder: &Holder,
        now: u64,
    ) -> Result<Result<CachedLicense, Unusable>, String> {
        let path = self.entry_path(content_id)?;
        if fs::symlink_metadata(&self.root).is_ok_and(|metadata| !metadata.is_dir()) {
            return Err(cache_io_error());
        }
        match fs::symlink_metadata(self.root.join(".revoked")) {
            Ok(_) => return Ok(Err(Unusable::Invalid)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(cache_io_error()),
        }
        if self.owner().is_some_and(|owner| owner != *holder) {
            self.revoke_all()?;
            diagnostic("content-cache-account");
            return Ok(Err(Unusable::Account));
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                diagnostic("content-cache-missing");
                return Ok(Err(Unusable::Missing));
            }
            Err(_) => return Err(cache_io_error()),
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Ok(Self::invalid(&path));
        }
        let bytes = read_bounded(&path, MAX_FILE_BYTES).map_err(|_| cache_io_error())?;
        let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
            return Ok(Self::invalid(&path));
        };
        if !self.authentic(&mut value) {
            return Ok(Self::invalid(&path));
        }
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        let number = |key: &str| value.get(key).and_then(Value::as_u64);
        let (Some(block), Some(lease), Some(issued_at), Some(last_used_at)) = (
            text("splicenseBlock"),
            text("lease"),
            number("issuedAt"),
            number("lastUsedAt"),
        ) else {
            return Ok(Self::invalid(&path));
        };
        if number("schema") != Some(SCHEMA)
            || text("contentId").as_deref() != Some(content_id.to_ascii_lowercase().as_str())
            || block.is_empty()
            || block.len() > MAX_BLOCK_CHARS
        {
            return Ok(Self::invalid(&path));
        }
        if text("puid").as_deref() != Some(holder.puid.as_str())
            || text("deviceId").as_deref() != Some(holder.device_id.as_str())
        {
            self.revoke_all()?;
            diagnostic("content-cache-account");
            return Ok(Err(Unusable::Account));
        }
        // A clock set back must not stretch the lease: refuse while the clock
        // is behind the last use, without removing the entry.
        if now.saturating_add(CLOCK_SKEW_SECONDS) < issued_at.max(last_used_at) {
            diagnostic("content-cache-clock");
            return Ok(Err(Unusable::Clock));
        }
        let Some(key) = stored_key(&value, &block) else {
            return Ok(Self::invalid(&path));
        };
        let expires_at = match check_lease(&lease, &key, issued_at, now) {
            LeaseCheck::Valid(until) => until,
            LeaseCheck::Expired => {
                remove_quietly(&path);
                diagnostic("content-cache-expired");
                return Ok(Err(Unusable::Expired));
            }
            LeaseCheck::Invalid(reason) => {
                tracing::debug!("The stored offline lease is not valid ({reason}).");
                return Ok(Self::invalid(&path));
            }
        };
        // Record the use so a later clock change cannot move behind it. A use
        // that cannot be recorded is not granted.
        value["lastUsedAt"] = json!(last_used_at.max(now));
        self.seal(&mut value)?;
        write_atomic(&path, value.to_string().as_bytes())?;
        Ok(Ok(CachedLicense {
            splicense_block: block,
            expires_at,
        }))
    }

    /// Removes the entry after Microsoft refused the content while online.
    pub fn purge(&self, content_id: &str) -> Result<bool, String> {
        let path = self.entry_path(content_id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(cache_io_error()),
        }
    }

    fn invalid(path: &Path) -> Result<CachedLicense, Unusable> {
        remove_quietly(path);
        diagnostic("content-cache-invalid");
        Err(Unusable::Invalid)
    }
}

/// The key license's binding as stored, for the lease's device check.
fn stored_key(value: &Value, block: &str) -> Option<License> {
    use crate::models::devicecredential::{Binding, LicenseInfo};
    let binding = value.get("keyBinding")?;
    let validity = value.get("keyValidity")?;
    let text = |key: &str| binding.get(key).and_then(Value::as_str).map(str::to_string);
    Some(License {
        splicense_block: block.to_string(),
        license_info: LicenseInfo {
            license_type: LicenseType::Full,
            license_usage: None,
            license_category: None,
            issued_date: None,
            last_updated_date: None,
            begin_date: validity
                .get("beginDate")
                .and_then(Value::as_str)
                .map(str::to_string),
            expiration_date: validity
                .get("expirationDate")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        custom_policies: None,
        binding: Binding {
            binding_type: text("type")?,
            device_id: text("deviceId"),
            product_id: None,
            pfm: None,
            user_id: None,
            usid: None,
            lease_required: text("leaseRequired"),
            lease_renewal_period: None,
            associated_pfns: None,
        },
    })
}

fn cache_io_error() -> String {
    "The offline license cache could not be accessed.".to_string()
}

fn remove_quietly(path: &Path) {
    let _ = fs::remove_file(path);
}

fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or_else(cache_io_error)?;
    // A fresh private file avoids following a stale or malicious .tmp link.
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| cache_io_error())?;
    temporary.write_all(bytes).map_err(|_| cache_io_error())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| cache_io_error())?;
    temporary.persist(path).map_err(|_| cache_io_error())?;
    #[cfg(unix)]
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| cache_io_error())?;
    Ok(())
}

#[cfg(test)]
#[path = "license_cache_tests.rs"]
pub(crate) mod tests;
