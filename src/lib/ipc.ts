import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Mirrors `config.rs`. Field names match config.json. */
export interface Config {
  hotkey: string;
  default_calendar_id: string;
  sync_interval_seconds: number;
  launch_on_startup: boolean;
  start_minimized: boolean;
  close_to_tray: boolean;
  tray_event_count: number;
  default_event_duration_minutes: number;
  notifications_enabled: boolean;
  use_24h_clock: boolean;
  disabled_calendar_ids: string[];
  user_agent?: string | null;
  google_client_id?: string | null;
  google_client_secret?: string | null;
}

export interface CalendarView {
  id: string;
  name: string;
  color: string | null;
  primary: boolean;
  writable: boolean;
  enabled: boolean;
}

export interface SettingsView {
  config: Config;
  connected: boolean;
  account: string | null;
  calendars: CalendarView[];
  builtin_client: boolean;
  hotkey_error: string | null;
  last_sync: string | null;
  last_error: string | null;
  config_path: string;
  version: string;
}

export interface QuickAddInfo {
  connected: boolean;
  calendar: string | null;
}

export interface QuickAddResult {
  title: string;
  when: string;
  html_link: string | null;
}

export const STATE_CHANGED = "calshell://state-changed";
export const QUICK_ADD_SHOWN = "calshell://quick-add-shown";

export const getSettings = () => invoke<SettingsView>("get_settings");
export const saveSettings = (config: Config) => invoke<SettingsView>("save_settings", { config });
export const connectGoogle = () => invoke<SettingsView>("connect_google");
export const disconnectGoogle = () => invoke<SettingsView>("disconnect_google");
export const quickAddInfo = () => invoke<QuickAddInfo>("quick_add_info");
export const quickAdd = (text: string) => invoke<QuickAddResult>("quick_add", { text });
export const hideQuickAdd = () => invoke<void>("hide_quick_add");
export const openSettings = () => invoke<void>("open_settings");
export const openEvent = (url: string | null) => invoke<void>("open_event", { url });
export const openLink = (url: string) => invoke<void>("open_link", { url });
export const syncNow = () => invoke<void>("sync_now");

export const onStateChanged = (handler: () => void): Promise<UnlistenFn> => listen(STATE_CHANGED, handler);
export const onQuickAddShown = (handler: () => void): Promise<UnlistenFn> => listen(QUICK_ADD_SHOWN, handler);

/** Commands reject with a plain string from Rust. */
export const errorMessage = (err: unknown): string =>
  typeof err === "string" ? err : err instanceof Error ? err.message : "Something went wrong.";
