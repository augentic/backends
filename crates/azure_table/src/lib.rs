#![cfg(not(target_arch = "wasm32"))]

//! Azure Table client for Qwasr WASM runtime.

mod sql;

use std::fmt::Debug;

use anyhow::Context;
use fromenv::FromEnv;
use qwasr::Backend;

/// Backend client for Azure Table storage
#[derive(Clone)]
pub struct Client {
    options: ConnectOptions
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
        Ok(Self { options: options.clone() } )
    }
}

/// Azure Table connection options
#[derive(Clone, Debug, FromEnv)]
pub struct ConnectOptions {
    /// Storage account name
    #[env(from = "AZURE_STORAGE_ACCOUNT")]
    pub name: String,

    /// Storage account access key
    #[env(from = "AZURE_STORAGE_KEY")]
    pub key: String,
}

impl qwasr::FromEnv for ConnectOptions {
    fn from_env() -> anyhow::Result<Self> {
        Self::from_env().finalize().context("issue loading azure table connection options")
    }
}
