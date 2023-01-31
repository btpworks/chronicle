use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::address::{hash_and_append, HasSawtoothAddress};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRegistration {
    // Der encoded public key
    pub key: String,
    pub date: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keys {
    pub id: String,
    pub current: KeyRegistration,
    pub expired: Option<KeyRegistration>,
}

impl HasSawtoothAddress for Keys {
    fn get_address(&self) -> String {
        key_address(&self.id)
    }
}

pub fn policy_address(id: impl AsRef<str>) -> String {
    hash_and_append(format!("opa:policy:binary:{}", id.as_ref()))
}

pub fn policy_meta_address(id: impl AsRef<str>) -> String {
    hash_and_append(format!("opa:policy:meta:{}", id.as_ref()))
}

pub fn key_address(id: impl AsRef<str>) -> String {
    hash_and_append(format!("opa:keys:{}", id.as_ref()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyMeta {
    pub id: String,
    pub date: DateTime<Utc>,
    pub policy_address: String,
}

impl HasSawtoothAddress for PolicyMeta {
    fn get_address(&self) -> String {
        policy_address(&self.id)
    }
}