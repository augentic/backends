#![cfg(not(target_arch = "wasm32"))]

//! Azure Identity Client.

mod identity;

use std::fmt::Debug;

use anyhow::{Context, Result};
use fromenv::FromEnv;
use qwasr::Backend;
use tracing::instrument;

#[derive(Clone)]
pub struct Client {
    /// Type of credential to use for authentication.
    pub credential_type: CredentialType,
}

impl Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzIdentiyClient").finish()
    }
}

impl Backend for Client {
    type ConnectOptions = ConnectOptions;

    #[instrument]
    async fn connect_with(options: Self::ConnectOptions) -> Result<Self> {
        Ok(Self {
            credential_type: options.credential_type,
        })
    }
}

#[derive(Debug, Clone, Default, FromEnv)]
pub struct ConnectOptions{
    /// Future override credential type. Current implementation only supports
    /// Managed Identity.
    #[env(from = "CREDENTIAL_TYPE", default = "ManagedIdentity")]
    pub credential_type: CredentialType,
}

impl qwasr::FromEnv for ConnectOptions {
    fn from_env() -> Result<Self> {
        Self::from_env().finalize().context("issue loading connection options")
    }
}

/// Type of credential to use for authentication.
#[derive(Clone, Debug, Default)]
pub enum CredentialType {
    #[default]
    ManagedIdentity,
}

/// Error parsing `CredentialType` from string to support `FromEnv` derivation.
#[derive(Debug)]
pub struct CredentialTypeParseError {
    message: String,
}

impl CredentialTypeParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CredentialTypeParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CredentialTypeParseError {}

impl std::str::FromStr for CredentialType {
    type Err = CredentialTypeParseError;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "" | "managedidentity" | "managed_identity" | "managed-identity" => Ok(Self::ManagedIdentity),
            _ => Err(CredentialTypeParseError::new(format!(
                "unsupported credential type '{value}'"
            ))),
        }
    }
}
