import { Eye, Maximize2, MonitorUp, MousePointer2, ShieldCheck, X } from "lucide-react";
import { useEffect, useRef } from "react";
import type { RemoteInputEvent } from "../types";
import { useRampage } from "../store";

const keyCodes: Record<string, number> = {
  Backspace: 0x08,
  Tab: 0x09,
  Enter: 0x0d,
  Shift: 0x10,
  Control: 0x11,
  Alt: 0x12,
  Pause: 0x13,
  CapsLock: 0x14,
  Escape: 0x1b,
  Space: 0x20,
  PageUp: 0x21,
  PageDown: 0x22,
  End: 0x23,
  Home: 0x24,
  ArrowLeft: 0x25,
  ArrowUp: 0x26,
  ArrowRight: 0x27,
  ArrowDown: 0x28,
  Insert: 0x2d,
  Delete: 0x2e,
  Meta: 0x5b,
  ContextMenu: 0x5d,
};

function virtualKey(event: React.KeyboardEvent): number | null {
  if (/^F(?:[1-9]|1[0-2])$/.test(event.key)) return 0x70 + Number(event.key.slice(1)) - 1;
  if (keyCodes[event.key]) return keyCodes[event.key];
  if (event.key.length === 1) {
    const code = event.key.toUpperCase().charCodeAt(0);
    if ((code >= 0x30 && code <= 0x39) || (code >= 0x41 && code <= 0x5a)) return code;
  }
  const nativeCode = (event.nativeEvent as KeyboardEvent).keyCode;
  return nativeCode >= 1 && nativeCode <= 254 ? nativeCode : null;
}

function pointerCoordinates(event: React.PointerEvent<HTMLImageElement>) {
  const rect = event.currentTarget.getBoundingClientRect();
  return {
    x: Math.max(0, Math.min(65_535, Math.round(((event.clientX - rect.left) / rect.width) * 65_535))),
    y: Math.max(0, Math.min(65_535, Math.round(((event.clientY - rect.top) / rect.height) * 65_535))),
  };
}

function pointerButton(button: number): "left" | "right" | "middle" | null {
  if (button === 0) return "left";
  if (button === 1) return "middle";
  if (button === 2) return "right";
  return null;
}

export function RemoteAssistPanel() {
  const session = useRampage((state) => state.remoteDesktopSession);
  const frame = useRampage((state) => state.remoteDesktopFrame);
  const pending = useRampage((state) => state.remoteDesktopPending);
  const close = useRampage((state) => state.closeRemoteDesktop);
  const send = useRampage((state) => state.sendRemoteDesktopInput);
  const viewer = useRef<HTMLDivElement>(null);
  const heldKeys = useRef(new Set<number>());
  const lastPointerSent = useRef(0);

  useEffect(() => {
    if (!session) return;
    void useRampage.getState().refreshRemoteDesktopFrame();
    const interval = window.setInterval(() => {
      void useRampage.getState().refreshRemoteDesktopFrame();
    }, 250);
    return () => window.clearInterval(interval);
  }, [session?.session_id]);

  useEffect(() => () => {
    const releases: RemoteInputEvent[] = [...heldKeys.current].map((virtual_key) => ({
      kind: "key",
      virtual_key,
      pressed: false,
    }));
    if (releases.length) void useRampage.getState().sendRemoteDesktopInput(releases);
    heldKeys.current.clear();
  }, []);

  if (!session) return null;
  const control = session.mode === "control";
  const source = frame ? `data:image/jpeg;base64,${frame.data_base64}` : null;
  const node = useRampage.getState().nodes.find((candidate) => candidate.id === session.node_id);

  const sendKey = (event: React.KeyboardEvent, pressed: boolean) => {
    if (!control) return;
    const key = virtualKey(event);
    if (!key) return;
    event.preventDefault();
    event.stopPropagation();
    if (pressed) heldKeys.current.add(key);
    else heldKeys.current.delete(key);
    void send([{ kind: "key", virtual_key: key, pressed }]);
  };

  return (
    <div className="remote-assist-backdrop" role="dialog" aria-modal="true" aria-label={`Remote desktop ${node?.name ?? session.node_id}`}>
      <div className="remote-assist-window" ref={viewer}>
        <header>
          <div className="remote-assist-title">
            <span className="live-dot" />
            <div><strong>{node?.name ?? "Paired worker"}</strong><span>{control ? "REMOTE CONTROL ACTIVE" : "LIVE VIEW"}</span></div>
          </div>
          <div className="remote-assist-tools">
            <span><ShieldCheck size={14} /> signed lease · {session.max_fps} fps ceiling</span>
            <button aria-label="Enter full screen" onClick={() => void viewer.current?.requestFullscreen()}><Maximize2 size={17} /></button>
            <button aria-label="Close Remote Assist" onClick={() => void close()}><X size={18} /></button>
          </div>
        </header>
        <div className={`remote-assist-stage ${control ? "control" : "view"}`}>
          {source ? (
            <img
              src={source}
              alt="Live paired worker desktop"
              draggable={false}
              tabIndex={control ? 0 : -1}
              onContextMenu={(event) => control && event.preventDefault()}
              onKeyDown={(event) => sendKey(event, true)}
              onKeyUp={(event) => sendKey(event, false)}
              onBlur={() => {
                const releases: RemoteInputEvent[] = [...heldKeys.current].map((virtual_key) => ({ kind: "key", virtual_key, pressed: false }));
                heldKeys.current.clear();
                if (releases.length) void send(releases);
              }}
              onPointerMove={(event) => {
                if (!control || performance.now() - lastPointerSent.current < 40) return;
                lastPointerSent.current = performance.now();
                void send([{ kind: "mouse_move", ...pointerCoordinates(event) }]);
              }}
              onPointerDown={(event) => {
                if (!control) return;
                event.currentTarget.focus();
                event.currentTarget.setPointerCapture(event.pointerId);
                const button = pointerButton(event.button);
                const events: RemoteInputEvent[] = [{ kind: "mouse_move", ...pointerCoordinates(event) }];
                if (button) events.push({ kind: "mouse_button", button, pressed: true });
                void send(events);
              }}
              onPointerUp={(event) => {
                if (!control) return;
                const button = pointerButton(event.button);
                if (button) void send([{ kind: "mouse_button", button, pressed: false }]);
              }}
              onWheel={(event) => {
                if (!control || event.deltaY === 0) return;
                event.preventDefault();
                const delta = Math.max(-1_920, Math.min(1_920, Math.round(-event.deltaY)));
                void send([{ kind: "mouse_wheel", delta }]);
              }}
            />
          ) : (
            <div className="remote-assist-loading"><MonitorUp size={36} /><strong>Opening the encrypted desktop stream…</strong><span>The worker must be unlocked and on its normal Windows desktop.</span></div>
          )}
          {pending && source && <span className="remote-frame-pulse">FRAME SYNC</span>}
        </div>
        <footer>
          <span>{control ? <MousePointer2 size={14} /> : <Eye size={14} />}{control ? "Mouse + keyboard routed through the paired mesh" : "View-only session — input authority not granted"}</span>
          <span>{frame ? `${frame.frame.width}×${frame.frame.height} · frame ${frame.frame.sequence}` : "Waiting for first frame"}</span>
          <strong>STOP on either machine revokes access</strong>
        </footer>
      </div>
    </div>
  );
}
