//! An OpenMLS provider whose storage is a borrowed SQLite connection, so MLS
//! state commits or rolls back together with the app's own tables (spike 1,
//! design section 11).

use openmls_libcrux_crypto::CryptoProvider;
use openmls_sqlite_storage::{Codec, Connection, SqliteStorageProvider};
use openmls_traits::OpenMlsProvider;

/// JSON encoding for OpenMLS's stored values.
#[derive(Default)]
pub struct Json;

impl Codec for Json {
    type Error = serde_json::Error;
    fn to_vec<T: serde::Serialize>(v: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(v)
    }
    fn from_slice<T: serde::de::DeserializeOwned>(b: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(b)
    }
}

/// OpenMLS storage over a connection or an open transaction.
pub type Storage<'c> = SqliteStorageProvider<Json, &'c Connection>;

/// A provider for one unit of work. Construct it from the transaction the
/// work runs in.
pub struct Provider<'c> {
    crypto: CryptoProvider,
    storage: Storage<'c>,
}

impl<'c> Provider<'c> {
    /// A provider storing through `conn`.
    pub fn on(conn: &'c Connection) -> Self {
        Self { crypto: postern_mls::crypto(), storage: SqliteStorageProvider::new(conn) }
    }
}

impl<'c> OpenMlsProvider for Provider<'c> {
    type CryptoProvider = CryptoProvider;
    type RandProvider = CryptoProvider;
    type StorageProvider = Storage<'c>;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }
    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }
    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

/// Create or upgrade OpenMLS's tables.
pub fn migrate(conn: &mut Connection) -> Result<(), String> {
    SqliteStorageProvider::<Json, &mut Connection>::new(conn).run_migrations().map_err(|e| e.to_string())
}
