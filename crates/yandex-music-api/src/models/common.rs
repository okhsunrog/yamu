use serde::{Deserialize, Serialize};

/// An API identifier. Yandex Music uses both JSON numbers and strings for IDs.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum Id {
    Integer(u64),
    String(String),
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Integer(value) => value.fmt(f),
            Self::String(value) => value.fmt(f),
        }
    }
}

impl From<u64> for Id {
    fn from(value: u64) -> Self {
        Self::Integer(value)
    }
}

impl From<String> for Id {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for Id {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

/// Zero-based page and bounded page size for paginated endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    page: u32,
    page_size: u32,
}

impl PageRequest {
    pub fn new(page: u32, page_size: u32) -> Self {
        Self {
            page,
            page_size: page_size.max(1),
        }
    }

    pub const fn page(self) -> u32 {
        self.page
    }

    pub const fn page_size(self) -> u32 {
        self.page_size
    }

    pub fn next(self) -> Self {
        Self {
            page: self.page.saturating_add(1),
            ..self
        }
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self::new(0, 100)
    }
}
