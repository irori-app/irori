use std::time::Instant;

pub trait TokenMutationGuard {}
impl<T> TokenMutationGuard for T {}
static PROCESS_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub trait TokenBackend: Send + Sync {
    /// Hold across read/merge/write/verification, never across network work.
    /// Persistent backends shared across processes must override this guard.
    fn mutation_lock(&self) -> Result<Box<dyn TokenMutationGuard + '_>, TokenStoreError> {
        Ok(Box::new(PROCESS_WRITES.lock().map_err(|_| {
            std::io::Error::other("Credential update was interrupted")
        })?))
    }
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, TokenStoreError>;
    fn set(&self, key: &str, value: &[u8]) -> Result<(), TokenStoreError>;
    fn remove(&self, key: &str) -> Result<(), TokenStoreError>;
}

pub trait ExpiringTokenBackend: TokenBackend {
    fn set_with_expiry(
        &self,
        key: &str,
        value: &[u8],
        expires_at: Instant,
    ) -> Result<(), TokenStoreError>;
}

#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error("The credential store could not be accessed.")]
    Storage(String),
    #[error("Stored Xodus credentials could not be read or saved.")]
    Io(#[from] std::io::Error),
    #[error("Stored Xodus credentials are empty or invalid. Check the system keyring.")]
    Serde(#[from] serde_json::Error),
    #[error("Required Xodus credentials are missing. Please sign in again.")]
    NotFound,
    #[error(
        "The system keyring did not return the credentials just saved. Check the keyring service before trying sign-in again."
    )]
    WriteVerification,
    #[error("Device authentication did not complete. Check the network connection and try again.")]
    DeviceAuthentication,
    #[error("The signed-in session changed while authenticating. Try again.")]
    SessionChanged,
    #[error("Saved account limit reached. Forget a saved Store account before adding another.")]
    AccountCapacity,
    #[error(
        "This saved account name belongs to a different Microsoft identity. Forget it explicitly before replacing it."
    )]
    AccountConflict,
}

impl TokenStoreError {
    /// Stable allowlist, never includes provider messages or stored JSON values.
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::Storage(_) => "storage-unavailable",
            Self::Io(_) => "credential-io",
            Self::Serde(_) => "keyring-invalid-data",
            Self::NotFound => "keyring-missing-data",
            Self::WriteVerification => "keyring-write-verification",
            Self::DeviceAuthentication => "device-authentication",
            Self::SessionChanged => "session-changed",
            Self::AccountCapacity => "saved-account-limit",
            Self::AccountConflict => "saved-account-conflict",
        }
    }
    pub fn report(&self) {
        eprintln!("NEXUS_CREDENTIAL_ERROR_V1 {}", self.diagnostic_code());
        eprintln!("{self}");
    }
}
