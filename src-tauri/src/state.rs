use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, RwLock};
use std::time::Duration;
use tokio::sync::Notify;

use crate::auth::Auth;
use crate::config::Config;
use crate::gcal::GcalClient;
use crate::reminders::FiredReminders;
use crate::sync::SyncStore;
use crate::tray::TrayModel;

pub struct AppState {
    config: RwLock<Config>,
    pub config_path: PathBuf,
    pub auth: Auth,
    pub gcal: GcalClient,
    store: Mutex<SyncStore>,
    reminders: Mutex<FiredReminders>,
    tray: Mutex<TrayModel>,
    /// Wakes the sync loop early (settings saved, event added, "Sync now").
    pub sync_now: Notify,
    pub hotkey_error: Mutex<Option<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AppState {
    pub fn new(config: Config, config_path: PathBuf, reminders_path: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Calshell/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("http client");
        Self {
            config: RwLock::new(config),
            config_path,
            auth: Auth::new(http.clone()),
            gcal: GcalClient::new(http),
            store: Mutex::new(SyncStore::default()),
            reminders: Mutex::new(FiredReminders::load(reminders_path)),
            tray: Mutex::new(TrayModel::default()),
            sync_now: Notify::new(),
            hotkey_error: Mutex::new(None),
        }
    }

    pub fn config(&self) -> Config {
        self.config.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn set_config(&self, config: Config) -> std::io::Result<()> {
        config.save(&self.config_path)?;
        *self.config.write().unwrap_or_else(|p| p.into_inner()) = config;
        Ok(())
    }

    pub fn store(&self) -> MutexGuard<'_, SyncStore> {
        lock(&self.store)
    }

    pub fn reminders(&self) -> MutexGuard<'_, FiredReminders> {
        lock(&self.reminders)
    }

    pub fn tray(&self) -> MutexGuard<'_, TrayModel> {
        lock(&self.tray)
    }

    pub fn hotkey_error(&self) -> MutexGuard<'_, Option<String>> {
        lock(&self.hotkey_error)
    }
}
