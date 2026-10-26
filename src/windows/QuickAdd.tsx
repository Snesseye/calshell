import { useCallback, useEffect, useRef, useState } from "react";
import {
  errorMessage,
  hideQuickAdd,
  onQuickAddShown,
  openSettings,
  quickAdd,
  quickAddInfo,
  type QuickAddInfo,
} from "../lib/ipc";

type Status =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "done"; title: string; when: string }
  | { kind: "error"; message: string };

const CLOSE_AFTER_SUCCESS_MS = 1200;

export default function QuickAdd() {
  const [text, setText] = useState("");
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [info, setInfo] = useState<QuickAddInfo | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const closeTimer = useRef<number | undefined>(undefined);

  const loadInfo = useCallback(() => {
    quickAddInfo().then(setInfo, () => setInfo(null));
    // Focus after the window is actually visible.
    requestAnimationFrame(() => inputRef.current?.focus());
  }, []);

  const reset = useCallback(() => {
    window.clearTimeout(closeTimer.current);
    setText("");
    setStatus({ kind: "idle" });
    loadInfo();
  }, [loadInfo]);

  useEffect(() => {
    loadInfo();
    const unlisten = onQuickAddShown(reset);
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [loadInfo, reset]);

  const submit = async () => {
    if (!text.trim() || status.kind === "saving") return;
    setStatus({ kind: "saving" });
    try {
      const result = await quickAdd(text);
      setStatus({ kind: "done", title: result.title, when: result.when });
      closeTimer.current = window.setTimeout(() => hideQuickAdd(), CLOSE_AFTER_SUCCESS_MS);
    } catch (err) {
      setStatus({ kind: "error", message: errorMessage(err) });
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      hideQuickAdd();
    }
  };

  const disconnected = info !== null && !info.connected;

  return (
    <div className="qa">
      <div className="qa-row">
        <CalendarGlyph />
        <input
          ref={inputRef}
          className="qa-input"
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            if (status.kind === "error") setStatus({ kind: "idle" });
          }}
          onKeyDown={onKeyDown}
          placeholder="call Sam tomorrow 3pm 30min"
          spellCheck={false}
          autoComplete="off"
          disabled={status.kind === "saving" || status.kind === "done"}
          aria-label="New event"
        />
        {status.kind === "saving" && <span className="spinner" aria-label="Adding" />}
      </div>
      <div className="qa-footer">
        {status.kind === "done" ? (
          <span className="qa-ok">
            Added <strong>{status.title}</strong>
            {status.when && <> · {status.when}</>}
          </span>
        ) : status.kind === "error" ? (
          <span className="qa-error">{status.message}</span>
        ) : disconnected ? (
          <span className="qa-error">
            Google Calendar is not connected.{" "}
            <button className="link" onClick={() => openSettings()}>
              Open settings
            </button>
          </span>
        ) : (
          <>
            <span>
              <kbd>Enter</kbd> add · <kbd>Esc</kbd> close
            </span>
            {info?.calendar && <span className="qa-calendar">{info.calendar}</span>}
          </>
        )}
      </div>
    </div>
  );
}

function CalendarGlyph() {
  return (
    <svg className="qa-glyph" viewBox="0 0 24 24" aria-hidden="true">
      <rect x="3.5" y="5" width="17" height="15.5" rx="3" fill="none" stroke="currentColor" strokeWidth="1.6" />
      <path d="M3.5 9.5h17M8 3v4M16 3v4" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
      <path d="M12 12.5v5M9.5 15h5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}
