import { Activity, Boxes, BrainCircuit, CircleStop, Command, Eye, Grid2X2, MonitorUp, MousePointer2, Orbit, Play, RefreshCw, Rocket, ShieldCheck, Wrench } from "lucide-react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { lazy, Suspense, useEffect } from "react";
import { CommandPalette } from "./components/CommandPalette";
import { ComputeStrategyPanel } from "./components/ComputeStrategyPanel";
import { ArenaBoundary, ArenaLoading } from "./components/ArenaBoundary";
import { PairingPanel } from "./components/PairingPanel";
import { Onboarding } from "./components/Onboarding";
import { OpsGrid } from "./components/OpsGrid";
import { RemoteAssistPanel } from "./components/RemoteAssistPanel";
import { RecoveryCenter } from "./components/RecoveryCenter";
import { type PairingRequest, surfaceNativePairingRequest, useRampage } from "./store";

const Arena = lazy(() => import("./components/Arena").then((module) => ({ default: module.Arena })));

export default function App() {
  const state = useRampage();
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void listen<PairingRequest | null>("rampage://pairing-request", (event) => {
      surfaceNativePairingRequest(event.payload);
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  useEffect(() => {
    void state.refresh();
    const interval = window.setInterval(() => void useRampage.getState().refresh(), 8_000);
    return () => window.clearInterval(interval);
  }, []);
  useEffect(() => {
    const active = state.onboarding || state.fabricRole === "owner" || state.workerPairing.state !== "idle";
    if (!active) return;
    void state.refreshPairing();
    const interval = window.setInterval(() => void useRampage.getState().refreshPairing(), 1_000);
    return () => window.clearInterval(interval);
  }, [state.onboarding, state.fabricRole, state.workerPairing.state]);
  useEffect(() => {
    if (state.fabricRole !== "worker") return;
    void state.refreshRemoteAssistStatus();
    const interval = window.setInterval(() => void useRampage.getState().refreshRemoteAssistStatus(), 1_000);
    return () => window.clearInterval(interval);
  }, [state.fabricRole]);
  const selected = state.nodes.find((node) => node.id === state.selectedNode) ?? state.nodes[0];
  return (
    <div className="shell">
      {state.onboarding && <Onboarding />}
      <CommandPalette />
      <RemoteAssistPanel />
      <RecoveryCenter />
      <header className="topbar">
        <div className="identity"><div className="brand-mark">R</div><div><strong>RAMPAGE</strong><span>PERSONAL COMPUTE FABRIC</span></div></div>
        <div className={`status-ribbon ${state.remoteAssistStatus.active ? "remote-active" : ""}`} role="status"><i className={state.connected && !state.killLatch ? "online" : "reduced"} /><strong>{state.killLatch ? "OWNER STOPPED" : state.remoteAssistStatus.active ? "REMOTE CONTROL ACTIVE" : state.fabricRole === "worker" ? state.workerRuntime.state === "active" ? "WORKER ACTIVE" : state.workerRuntime.state === "starting" ? "WORKER CONNECTING" : "WORKER ATTENTION" : state.connected ? "FABRIC LIVE" : "LOCAL REDUCED"}</strong><span>{state.nodes.length} nodes</span><span>{state.meshMode.replace("_", " ")}</span><span>{state.diagnostic ? `self-scan ${state.diagnostic.health_score}/100` : state.capability.replaceAll("_", " ")}</span></div>
        <div className="header-actions">
          <button className="icon-button" onClick={() => void state.refresh()} aria-label="Refresh fabric"><RefreshCw size={17} /></button>
          <button className="icon-button" onClick={() => state.setRecoveryOpen(true)} aria-label="Open Recovery Center" title="Fix connection or start over"><Wrench size={17} /></button>
          <button className={`autostart-button ${state.runAtLogin ? "active" : ""}`} onClick={() => void state.toggleAutostart()} aria-label={state.runAtLogin ? "Stop launching Rampage with Windows" : "Start Rampage with Windows"} title="Keep your fabric available after sign-in"><Rocket size={15} /> {state.runAtLogin ? "AUTO-START ON" : "AUTO-START OFF"}</button>
          <button className="command-button" onClick={() => state.setCommandOpen(true)}><Command size={16} /> Command <kbd>Ctrl K</kbd></button>
          {state.killLatch
            ? <button className="resume-button" onClick={() => { if (window.confirm("Resume Rampage sharing under the current owner policy?")) void state.localResume().catch((error: unknown) => useRampage.setState({ lastAction: error instanceof Error ? error.message : "Resume failed." })); }}><Play size={17} /> RESUME</button>
            : <button className="stop-button" onClick={state.localStop}><CircleStop size={17} /> STOP</button>}
        </div>
      </header>
      <aside className="rail" aria-label="Primary navigation">
        <button className="active" aria-label="Fabric"><Boxes /></button>
        <button aria-label="Work"><Activity /></button>
        <button aria-label="Evolution"><BrainCircuit /></button>
        <button aria-label="Evidence"><ShieldCheck /></button>
        <span />
        <button onClick={() => state.setMode(state.mode === "arena" ? "grid" : "arena")} aria-label={`Switch to ${state.mode === "arena" ? "grid" : "arena"} view`}>
          {state.mode === "arena" ? <Grid2X2 /> : <Orbit />}
        </button>
      </aside>
      <main id="main">
        <section className="workspace">
          <div className="section-heading"><div><p className="eyebrow">FABRIC DECK</p><h1>Your machines, acting as one.</h1></div><div className="view-switch" role="group" aria-label="View"><button className={state.mode === "arena" ? "active" : ""} onClick={() => state.setMode("arena")}><Orbit size={15} /> Arena</button><button className={state.mode === "grid" ? "active" : ""} onClick={() => state.setMode("grid")}><Grid2X2 size={15} /> Grid</button></div></div>
          {state.fabricRole === "owner" && <ComputeStrategyPanel />}
          {state.mode === "arena" ? (
            <ArenaBoundary openGrid={() => state.setMode("grid")}>
              <Suspense fallback={<ArenaLoading openGrid={() => state.setMode("grid")} />}>
                <Arena openGrid={() => state.setMode("grid")} />
              </Suspense>
            </ArenaBoundary>
          ) : <OpsGrid />}
        </section>
        <aside className="inspector" aria-live="polite">
          <PairingPanel />
          {selected ? <>
          <p className="eyebrow">SELECTED NODE</p>
          <h2>{selected.name}</h2><span className={`badge ${selected.state}`}>{selected.state}</span>
          <div className="orbital-gauge" style={{ "--value": `${selected.cpu * 3.6}deg` } as React.CSSProperties}><strong>{selected.cpu}%</strong><span>CPU LOAD</span></div>
          <dl><div><dt>Memory</dt><dd>{selected.memory}%</dd></div><div><dt>GPU</dt><dd>{selected.gpu}%</dd></div><div><dt>Model lane</dt><dd>{selected.modelRuntimeCount ? `${selected.modelMemoryAvailableGb} GB · ${selected.modelRuntimeCount} runtime${selected.modelRuntimeCount === 1 ? "" : "s"}` : "Not qualified"}</dd></div><div><dt>Donated storage</dt><dd>{selected.storageAvailableGb} GB free</dd></div><div><dt>Fabric link</dt><dd>{selected.topologyConfidence === "measured" ? `${selected.latencyMs} ms / ${selected.downlinkMbps} Mbps down` : selected.topologyConfidence === "controller_local" ? "Controller local" : "Awaiting signed benchmark"}</dd></div><div><dt>Kind</dt><dd>{selected.kind.replace("_", " ")}</dd></div><div><dt>Policy</dt><dd>Owner protected</dd></div></dl>
          <div className="explanation"><ShieldCheck size={18} /><div><strong>Why this state?</strong><p>{selected.state === "sleeping" ? "Charging and foreground conditions are not currently satisfied." : "The node is inside owner reserves and its capability offer is admissible."}</p></div></div>
          {state.fabricRole === "worker" && <div className={`remote-permission ${state.remoteAssistStatus.active ? "active" : ""}`}>
            <div><MonitorUp size={18} /><div><strong>Owner Remote Assist</strong><p>{state.remoteAssistStatus.active ? `Your owner is ${state.remoteAssistStatus.mode === "control" ? "controlling" : "viewing"} this desktop now. Lock screen and admin prompts stay blocked.` : "Let your paired owner view or control this unlocked Windows desktop. Lock screen and admin prompts stay blocked."}</p></div></div>
            <label className="permission-switch"><span>{state.remoteAssistStatus.enabled ? "Allowed" : "Off"}</span><input aria-label="Allow owner remote control" type="checkbox" checked={state.remoteAssistStatus.enabled} disabled={!state.remoteAssistStatus.supported} onChange={(event) => void state.setRemoteAssistEnabled(event.currentTarget.checked)} /><i /></label>
          </div>}
          {state.diagnostic && <div className="explanation"><BrainCircuit size={18} /><div><strong>Autonomous self-scan · {state.diagnostic.status} · {state.diagnostic.health_score}/100</strong><p>{state.diagnostic.findings.find((finding) => finding.severity !== "info")?.evidence ?? "No warning or critical bottleneck is present in the latest evidence window."} No per-change approval is required inside the owner envelope.</p></div></div>}
          <button className="secondary">Open evidence trail</button>
          {state.fabricRole === "owner" && selected.remoteAssist && <div className="remote-launch-actions"><button disabled={state.remoteDesktopPending || state.killLatch} onClick={() => void state.openRemoteDesktop(selected.id, "view").catch((error: unknown) => useRampage.setState({ lastAction: error instanceof Error ? error.message : "Remote view failed." }))}><Eye size={16} /> View desktop</button><button className="control" disabled={state.remoteDesktopPending || state.killLatch} onClick={() => void state.openRemoteDesktop(selected.id, "control").catch((error: unknown) => useRampage.setState({ lastAction: error instanceof Error ? error.message : "Remote control failed." }))}><MousePointer2 size={16} /> Control desktop</button></div>}
          {state.fabricRole === "owner" && <label className="secondary file-action">{selected.artifactEndpoint ? "Encrypt + replicate file" : "Encrypt file locally"}<input type="file" onChange={(event) => { const file = event.currentTarget.files?.[0]; if (file) void state.storeFile(file, selected.id).catch((error: unknown) => useRampage.setState({ lastAction: error instanceof Error ? error.message : "Artifact transfer failed." })); event.currentTarget.value = ""; }} /></label>}
          {state.inviteCode && <div className="explanation"><ShieldCheck size={18} /><div><strong>One-time invite</strong><p>{state.inviteCode}</p>{state.inviteBundle && <button className="secondary" onClick={() => void navigator.clipboard.writeText(state.inviteBundle!)}>Copy complete invite</button>}</div></div>}
          </> : <div className="empty-fabric"><p className="eyebrow">NO ACTIVE NODES</p><h2>Ready for your next machine</h2><p>On another PC choose “Join my fabric.” Rampage detects it here automatically and asks once before admitting it.</p></div>}
        </aside>
      </main>
      <footer className="evidence-spine"><span><i /> EVIDENCE SPINE</span><strong>{state.lastAction ?? (state.events.length ? `${state.events.length} verified events` : "No controller evidence yet")}</strong><span>{state.lastSync ? `Synced ${state.lastSync.toLocaleTimeString()}` : "Discovering…"}</span><span className="push">THRESHOLDED AUTONOMY · NO PER-CHANGE APPROVAL</span></footer>
    </div>
  );
}
