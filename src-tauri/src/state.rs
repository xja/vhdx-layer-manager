use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};

use crate::{
    db::{AppSettings, Database},
    error::{AppError, Result},
    logging::init_tracing,
    paths::{validate_workspace_root, AppPaths},
};

#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<StateInner>>,
}

#[derive(Default)]
struct StateInner {
    paths: Option<AppPaths>,
    db: Option<Arc<Database>>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(StateInner::default())),
        }
    }
}

impl SharedState {
    pub fn initialize(&self, root: PathBuf, locale: Option<String>) -> Result<AppSettings> {
        validate_workspace_root(&root)?;
        let paths = AppPaths::new(root);
        paths.ensure_layout()?;
        init_tracing(paths.ops_log_path().as_path())?;

        let db = Arc::new(Database::open(&paths)?);
        db.update_root_path(paths.root())?;
        if let Some(locale) = locale {
            db.update_locale(&locale)?;
        }
        let settings = db.get_settings()?;

        {
            let mut inner = self.inner.write().expect("state lock poisoned");
            inner.paths = Some(paths);
            inner.db = Some(db.clone());
        }

        Ok(settings)
    }

    pub fn get_settings(&self) -> Result<Option<AppSettings>> {
        if let Some(db) = self.db_opt() {
            let settings = db.get_settings()?;
            if let Err(err) = validate_workspace_root(std::path::Path::new(&settings.root_path)) {
                // Reject previously saved drive-root workspaces instead of partially restoring them.
                return Err(err);
            }
            Ok(Some(settings))
        } else {
            Ok(None)
        }
    }

    pub fn paths(&self) -> Result<AppPaths> {
        self.inner
            .read()
            .expect("state lock poisoned")
            .paths
            .clone()
            .ok_or(AppError::RootNotInitialized)
    }

    pub fn db(&self) -> Result<Arc<Database>> {
        self.db_opt().ok_or(AppError::RootNotInitialized)
    }

    fn db_opt(&self) -> Option<Arc<Database>> {
        self.inner.read().expect("state lock poisoned").db.clone()
    }
}
