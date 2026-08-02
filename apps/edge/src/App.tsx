import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BatteryCharging, BatteryMedium, Cpu, Flame, Network, ShieldCheck, Square, Zap } from "lucide-react";

type NativeStatus = {
  platform: string;
  deviceKind: string;
  foreground: boolean;
  donationRequested: boolean;
  batteryPercent: number;
  onExternalPower: boolean;
  lowPowerMode: boolean;
  thermalHeadroomPercent: number;
  screenKeptAwake: boolean;
};

type Session = {
  nodeId: string;
  controllerEndpointId: string;
  enrolled: boolean;
  eligible: boolean;
  offerExpiresAt?: string;
  receiptsSubmitted: number;
  lastResult: string;
};

type EdgeView = { native: NativeStatus; session?: Session; message: string };

const blankNative: NativeStatus = {
  platform: "mobile",
  deviceKind: "phone",
  foreground: false,
  donationRequested: false,
  batteryPercent: 0,
  onExternalPower: false,
  lowPowerMode: false,
  thermalHeadroomPercent: 0,
  screenKeptAwake: false
};

export default function App() {
  const [view, setView] = useState<EdgeView>({ native: blankNative, message: "Native telemetry is starting." });
  const [displayName, setDisplayName] = useState("My Edge Device");
  const [invitation, setInvitation] = useState("");
  const [busy, setBusy] = useState(false);
  const active = Boolean(view.session?.eligible && view.native.donationRequested);

  const pulse = useCallback(async () => {
    try {
      setView(await invoke<EdgeView>("edge_pulse"));
    } catch (error) {
      const reason = String(error);
      try {
        const stopped = await invoke<EdgeView>("edge_stop");
        setView({ ...stopped, message: `Donation stopped after a failed lease pulse: ${reason}` });
      } catch {
        setView((current) => ({
          ...current,
          native: { ...current.native, donationRequested: false, screenKeptAwake: false },
          session: current.session ? { ...current.session, eligible: false } : undefined,
          message: `Donation revoked after a failed lease pulse: ${reason}`
        }));
      }
    }
  }, []);

  useEffect(() => {
    void invoke<EdgeView>("edge_status").then(setView).catch((error: unknown) => {
      setView((current) => ({ ...current, message: String(error) }));
    });
  }, []);

  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => void pulse(), 5_000);
    return () => window.clearInterval(timer);
  }, [active, pulse]);

  useEffect(() => {
    const stopWhenHidden = () => {
      if (document.visibilityState !== "visible" && active) {
        void invoke<EdgeView>("edge_stop").then(setView).catch(() => undefined);
      }
    };
    document.addEventListener("visibilitychange", stopWhenHidden);
    return () => document.removeEventListener("visibilitychange", stopWhenHidden);
  }, [active]);

  const start = async () => {
    setBusy(true);
    try {
      setView(await invoke<EdgeView>("edge_start", {
        invitation: invitation.trim() || null,
        displayName: displayName.trim()
      }));
      setInvitation("");
    } catch (error) {
      setView((current) => ({ ...current, message: String(error) }));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try { setView(await invoke<EdgeView>("edge_stop")); }
    finally { setBusy(false); }
  };

  const stateLabel = useMemo(() => {
    if (active) return "CONTRIBUTING";
    if (view.native.lowPowerMode) return "LOW POWER — PAUSED";
    if (view.native.thermalHeadroomPercent < 35) return "THERMAL — PAUSED";
    return "OWNER CONTROLLED";
  }, [active, view.native.lowPowerMode, view.native.thermalHeadroomPercent]);

  return <main>
    <header><div className="mark">R</div><div><strong>RAMPAGE EDGE</strong><span>FOREGROUND CONTRIBUTOR</span></div><i className={active ? "live" : ""} /></header>
    <section className="hero">
      <p className="eyebrow">{stateLabel}</p>
      <h1>Useful compute.<br/><em>No pretending.</em></h1>
      <p>Your phone contributes small, restart-safe work while this screen is open and native safety signals agree. It never becomes remote RAM, protected storage, or an always-on GPU.</p>
    </section>

    <section className="telemetry" aria-label="Native device safety telemetry">
      <article><BatteryMedium/><small>BATTERY</small><b>{view.native.batteryPercent}%</b><span>{view.native.onExternalPower ? "external power" : "reserve protected"}</span></article>
      <article><Flame/><small>THERMAL</small><b>{view.native.thermalHeadroomPercent}%</b><span>minimum 35%</span></article>
      <article><Cpu/><small>DEVICE</small><b>{view.native.deviceKind}</b><span>{view.native.platform}</span></article>
      <article><Network/><small>RECEIPTS</small><b>{view.session?.receiptsSubmitted ?? 0}</b><span>signed results</span></article>
    </section>

    {!active && <section className="enroll">
      <label>DEVICE NAME<input value={displayName} maxLength={80} onChange={(event) => setDisplayName(event.target.value)} /></label>
      <label>SIGNED INVITATION <span>only required the first time</span><textarea value={invitation} maxLength={262144} onChange={(event) => setInvitation(event.target.value)} placeholder="Paste the complete Rampage invitation from your owner PC" /></label>
      <button disabled={busy || !displayName.trim()} onClick={() => void start()}><Zap/> Start foreground donation</button>
    </section>}

    {active && <section className="session">
      <div className="pulse"><span/><span/><span/><b>LEASE PULSE</b></div>
      <p>{view.session?.lastResult}</p>
      <dl><div><dt>NODE</dt><dd>{view.session?.nodeId.slice(0, 18)}…</dd></div><div><dt>OFFER EXPIRES</dt><dd>{view.session?.offerExpiresAt ? new Date(view.session.offerExpiresAt).toLocaleTimeString() : "refreshing"}</dd></div></dl>
      <button className="stop" disabled={busy} onClick={() => void stop()}><Square/> Stop and release this device</button>
    </section>}

    <section className="boundary">
      <ShieldCheck/><div><strong>THE HARD BOUNDARY</strong><p>Only allowlisted hash and evaluation shards. Twenty-second offers. Durable one-shot nonces. No model server, shell, public marketplace, protected replicas, or authority to widen policy.</p></div>
    </section>
    <footer>{view.message}<span>{view.native.screenKeptAwake ? <><BatteryCharging/> screen held awake</> : "session idle"}</span></footer>
  </main>;
}
