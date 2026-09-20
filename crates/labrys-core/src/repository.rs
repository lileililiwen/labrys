use std::collections::HashMap;
use std::sync::RwLock;

use crate::application::Application;
use crate::error::{CoreError, Result};
use crate::ids::ApplicationId;

/// Persistence boundary for applications.
///
/// The platform database is authoritative; this trait lets the SQLx
/// implementation arrive in a later change while tests use the in-memory
/// implementation.
pub trait ApplicationRepository: Send + Sync {
    fn save(&self, app: Application) -> Result<()>;
    fn get(&self, id: &ApplicationId) -> Result<Application>;
    fn list(&self) -> Vec<Application>;
    fn delete(&self, id: &ApplicationId) -> Result<()>;
}

/// In-memory repository for tests and single-process use.
#[derive(Debug, Default)]
pub struct InMemoryApplicationRepository {
    inner: RwLock<HashMap<String, Application>>,
}

impl InMemoryApplicationRepository {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    fn key(id: &ApplicationId) -> String {
        id.to_string()
    }
}

impl ApplicationRepository for InMemoryApplicationRepository {
    fn save(&self, app: Application) -> Result<()> {
        let mut guard = self
            .inner
            .write()
            .map_err(|_| CoreError::InvalidManifest("repository lock poisoned".to_string()))?;
        guard.insert(Self::key(&app.id), app);
        Ok(())
    }

    fn get(&self, id: &ApplicationId) -> Result<Application> {
        let guard = self
            .inner
            .read()
            .map_err(|_| CoreError::InvalidManifest("repository lock poisoned".to_string()))?;
        guard
            .get(&Self::key(id))
            .cloned()
            .ok_or_else(|| CoreError::NotFound(id.to_string()))
    }

    fn list(&self) -> Vec<Application> {
        self.inner
            .read()
            .map(|guard| guard.values().cloned().collect())
            .unwrap_or_default()
    }

    fn delete(&self, id: &ApplicationId) -> Result<()> {
        let mut guard = self
            .inner
            .write()
            .map_err(|_| CoreError::InvalidManifest("repository lock poisoned".to_string()))?;
        guard
            .remove(&Self::key(id))
            .map(|_| ())
            .ok_or_else(|| CoreError::NotFound(id.to_string()))
    }
}
