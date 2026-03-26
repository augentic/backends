#![doc = include_str!("../README.md")]
#![cfg(not(target_arch = "wasm32"))]

pub mod store;

use std::fmt::Debug;

use anyhow::Context;
use omnia::Backend;

/// Backend client for Azure Table storage.
#[derive(Clone)]
pub struct Client {
    options: ConnectOptions,
}

impl Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzTableClient").finish()
    }
}

impl Backend for Client {
    type ConnectOptions = ConnectOptions;

    #[tracing::instrument]
    async fn connect_with(options: Self::ConnectOptions) -> anyhow::Result<Self> {
        Ok(Self {
            options: options.clone(),
        })
    }
}

#[allow(missing_docs)]
mod config {
    use fromenv::FromEnv;

    /// Azure Table connection options.
    #[derive(Clone, Debug, FromEnv)]
    pub struct ConnectOptions {
        /// Storage account name.
        #[env(from = "AZURE_STORAGE_ACCOUNT")]
        pub name: String,

        /// Storage account access key.
        #[env(from = "AZURE_STORAGE_KEY")]
        pub key: String,

        /// Table service endpoint URL. When empty (the default), the Azure
        /// public cloud URL `https://{name}.table.core.windows.net` is used.
        /// Set to `http://127.0.0.1:10002/{name}` for Azurite, or to a
        /// sovereign-cloud / Azure Stack endpoint as needed.
        #[env(from = "AZURE_TABLE_ENDPOINT", default = "")]
        pub endpoint: String,
    }

    impl ConnectOptions {
        /// Resolved base URL for the table service (no trailing slash).
        #[must_use]
        pub fn base_url(&self) -> String {
            if self.endpoint.is_empty() {
                format!("https://{}.table.core.windows.net", self.name)
            } else {
                self.endpoint.trim_end_matches('/').to_owned()
            }
        }
    }
}
pub use config::ConnectOptions;

impl omnia::FromEnv for ConnectOptions {
    fn from_env() -> anyhow::Result<Self> {
        Self::from_env().finalize().context("issue loading azure table connection options")
    }
}
