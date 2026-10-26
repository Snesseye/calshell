import { useCallback, useEffect, useState } from "react";
import {
  connectGoogle,
  disconnectGoogle,
  errorMessage,
  getSettings,
  onStateChanged,
  openLink,
  saveSettings,
  syncNow,
  type Config,
  type SettingsView,
} from "../lib/ipc";

const SETUP_GUIDE = "https://github.com/Snesseye/calshell#1-create-a-google-cloud-oauth-client";
const DURATIONS = [15, 20, 30, 45, 60, 90, 120];
const SYNC_INTERVALS = [
  { value: 30, label: "30 seconds" },
  { value: 60, label: "1 minute" },
  { value: 120, label: "2 minutes" },
  { value: 300, label: "5 minutes" },
];

export default function Settings() {
  const [view, setView] = useState<SettingsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);

  const refresh = useCallback(() => {
    getSettings().then(setView, (err) => setError(errorMessage(err)));
  }, []);

  useEffect(() => {
    refresh();
    const unlisten = onStateChanged(refresh);
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const update = async (patch: Partial<Config>) => {
    if (!view) return;
    try {
      setView(await saveSettings({ ...view.config, ...patch }));
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
      refresh();
    }
  };

  const connect = async () => {
    setConnecting(true);
    setError(null);
    try {
      setView(await connectGoogle());
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setConnecting(false);
    }
  };

  const disconnect = async () => {
    try {
      setView(await disconnectGoogle());
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  if (!view) {
    return <main className="settings">{error && <p className="banner error">{error}</p>}</main>;
  }

  const { config } = view;
  const needsClient = !view.builtin_client && !config.google_client_id;
  const writable = view.calendars.filter((c) => c.writable);

  return (
    <main className="settings">
      <h1>Settings</h1>
      {error && (
        <p className="banner error" role="alert">
          {error}
        </p>
      )}

      <section className="card">
        <h2>Google Calendar</h2>
        <p className="muted">
          Powers the tray, quick add and notifications. The calendar window has its own Google sign-in.
        </p>
        <div className="row">
          <div>
            <div className="label">{view.connected ? "Connected" : "Not connected"}</div>
            {view.connected && view.account && <div className="muted">{view.account}</div>}
            {view.connected && (
              <div className="muted small">
                {view.last_error
                  ? `Last sync failed: ${view.last_error}`
                  : view.last_sync
                    ? `Synced ${view.last_sync}`
                    : "Syncing…"}
              </div>
            )}
          </div>
          {view.connected ? (
            <div className="actions">
              <button onClick={() => syncNow()}>Sync now</button>
              <button onClick={disconnect}>Disconnect</button>
            </div>
          ) : (
            <button className="primary" onClick={connect} disabled={connecting || needsClient}>
              {connecting ? "Waiting for browser…" : "Connect"}
            </button>
          )}
        </div>
        {connecting && <p className="muted small">Finish signing in in your browser, then come back here.</p>}
        {(needsClient || config.google_client_id) && (
          <OAuthClientFields config={config} onSave={update} required={needsClient} />
        )}
      </section>

      <section className="card">
        <h2>Quick add</h2>
        <Field label="Shortcut" hint="Opens the quick add box from anywhere.">
          <HotkeyRecorder value={config.hotkey} onChange={(hotkey) => update({ hotkey })} />
        </Field>
        {view.hotkey_error && <p className="banner error">{view.hotkey_error}</p>}
        <Field label="Add to">
          <select
            value={config.default_calendar_id}
            onChange={(e) => update({ default_calendar_id: e.target.value })}
            disabled={!view.connected}
          >
            <option value="primary">Primary calendar</option>
            {writable
              .filter((c) => !c.primary)
              .map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
          </select>
        </Field>
        <Field label="Default length" hint="Used when you don't type one.">
          <select
            value={config.default_event_duration_minutes}
            onChange={(e) => update({ default_event_duration_minutes: Number(e.target.value) })}
          >
            {withCurrent(DURATIONS, config.default_event_duration_minutes).map((m) => (
              <option key={m} value={m}>
                {m < 60 ? `${m} min` : m % 60 === 0 ? `${m / 60} h` : `${Math.floor(m / 60)} h ${m % 60} min`}
              </option>
            ))}
          </select>
        </Field>
      </section>

      {view.calendars.length > 0 && (
        <section className="card">
          <h2>Calendars</h2>
          <p className="muted">Shown in the tray and used for notifications.</p>
          <ul className="calendars">
            {view.calendars.map((c) => (
              <li key={c.id}>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={c.enabled}
                    onChange={(e) =>
                      update({
                        disabled_calendar_ids: e.target.checked
                          ? config.disabled_calendar_ids.filter((id) => id !== c.id)
                          : [...config.disabled_calendar_ids, c.id],
                      })
                    }
                  />
                  <span className="swatch" style={{ background: c.color ?? "var(--accent)" }} />
                  <span className="calendar-name">{c.name}</span>
                </label>
              </li>
            ))}
          </ul>
        </section>
      )}

      <section className="card">
        <h2>App</h2>
        <Toggle
          label="Launch on startup"
          checked={config.launch_on_startup}
          onChange={(v) => update({ launch_on_startup: v })}
        />
        <Toggle
          label="Start in the tray at login"
          checked={config.start_minimized}
          onChange={(v) => update({ start_minimized: v })}
        />
        <Toggle
          label="Closing the window keeps Calshell in the tray"
          checked={config.close_to_tray}
          onChange={(v) => update({ close_to_tray: v })}
        />
        <Toggle
          label="Event notifications"
          checked={config.notifications_enabled}
          onChange={(v) => update({ notifications_enabled: v })}
        />
        <Toggle label="24-hour clock" checked={config.use_24h_clock} onChange={(v) => update({ use_24h_clock: v })} />
        <Field label="Events in tray">
          <select value={config.tray_event_count} onChange={(e) => update({ tray_event_count: Number(e.target.value) })}>
            {Array.from({ length: 10 }, (_, i) => i + 1).map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Sync every">
          <select
            value={config.sync_interval_seconds}
            onChange={(e) => update({ sync_interval_seconds: Number(e.target.value) })}
          >
            {withCurrent(
              SYNC_INTERVALS.map((s) => s.value),
              config.sync_interval_seconds,
            ).map((value) => (
              <option key={value} value={value}>
                {SYNC_INTERVALS.find((s) => s.value === value)?.label ?? `${value} seconds`}
              </option>
            ))}
          </select>
        </Field>
      </section>

      <footer className="muted small">
        Calshell {view.version} · Settings file: <code>{view.config_path}</code>
      </footer>
    </main>
  );
}

/** Keeps a hand-edited config value selectable. */
function withCurrent(options: number[], current: number): number[] {
  return options.includes(current) ? options : [...options, current].sort((a, b) => a - b);
}

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="row">
      <div>
        <div className="label">{label}</div>
        {hint && <div className="muted small">{hint}</div>}
      </div>
      {children}
    </div>
  );
}

function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="row toggle-row">
      <span className="label">{label}</span>
      <input type="checkbox" role="switch" className="switch" checked={checked} onChange={(e) => onChange(e.target.checked)} />
    </label>
  );
}

function OAuthClientFields({
  config,
  onSave,
  required,
}: {
  config: Config;
  onSave: (patch: Partial<Config>) => void;
  required: boolean;
}) {
  const [id, setId] = useState(config.google_client_id ?? "");
  const [secret, setSecret] = useState(config.google_client_secret ?? "");

  const save = () => {
    if (id !== (config.google_client_id ?? "") || secret !== (config.google_client_secret ?? "")) {
      onSave({ google_client_id: id.trim() || null, google_client_secret: secret.trim() || null });
    }
  };

  return (
    <div className="oauth">
      <p className="muted small">
        {required ? "This build has no Google OAuth client. " : ""}
        Create a Desktop OAuth client in Google Cloud and paste it here.{" "}
        <button className="link" onClick={() => openLink(SETUP_GUIDE)}>
          Setup guide
        </button>
      </p>
      <input
        placeholder="Client ID"
        value={id}
        onChange={(e) => setId(e.target.value)}
        onBlur={save}
        spellCheck={false}
        aria-label="OAuth client ID"
      />
      <input
        placeholder="Client secret"
        type="password"
        value={secret}
        onChange={(e) => setSecret(e.target.value)}
        onBlur={save}
        spellCheck={false}
        aria-label="OAuth client secret"
      />
    </div>
  );
}

const MODIFIER_KEYS = new Set(["Control", "Shift", "Alt", "Meta", "OS"]);

/** Turns a key press into an accelerator string such as "Ctrl+Shift+Space". */
function toAccelerator(e: React.KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(e.key)) return null;
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean);
  const key = e.code.replace(/^Key/, "").replace(/^Digit/, "");
  const isFunctionKey = /^F\d{1,2}$/.test(key);
  if (mods.length === 0 && !isFunctionKey) return null;
  return [...mods, key].join("+");
}

function HotkeyRecorder({ value, onChange }: { value: string; onChange: (hotkey: string) => void }) {
  const [recording, setRecording] = useState(false);

  return (
    <button
      className={`hotkey ${recording ? "recording" : ""}`}
      onClick={() => setRecording(true)}
      onBlur={() => setRecording(false)}
      onKeyDown={(e) => {
        if (!recording) return;
        e.preventDefault();
        if (e.key === "Escape") {
          setRecording(false);
          return;
        }
        const accelerator = toAccelerator(e);
        if (accelerator) {
          setRecording(false);
          if (accelerator !== value) onChange(accelerator);
        }
      }}
    >
      {recording ? "Press keys…" : value.split("+").map((k, i) => <kbd key={i}>{k}</kbd>)}
    </button>
  );
}
