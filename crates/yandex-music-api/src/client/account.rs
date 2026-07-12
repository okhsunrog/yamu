use super::Client;
use crate::{Result, models::AccountStatus};

impl Client {
    /// Returns current account information. Authentication is required.
    pub async fn account_status(&self) -> Result<AccountStatus> {
        self.get("account/status", &()).await
    }
}
