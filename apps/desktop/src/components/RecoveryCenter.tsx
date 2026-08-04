import { AlertTriangle, Check, Clipboard, HardDrive, RefreshCw, RotateCcw, ShieldX, Trash2, Unplug, X } from "lucide-react";
import { useMemo, useState } from "react";
import { useRampage } from "../store";

type DestructiveAction =
  | { kind: "leave"; phrase: "LEAVE FABRIC"; requiresTyping: false; title: string; detail: string }
  | { kind: "reset"; phrase: "RESET RAMPAGE"; requiresTyping: true; title: string; detail: string }
  | { kind: "forget"; phrase: string; requiresTyping: false; title: string; detail: string; nodeId: string };

export function RecoveryCenter() {
  const open = useRampage((state) => state.recoveryOpen);
  const status = useRampage((state) => state.recoveryStatus);
  const setOpen = useRampage((state) => state.setRecoveryOpen);
  const refresh = useRampage((state) => state.refreshRecovery);
  const repair = useRampage((state) => state.repairConnection);
  const leave = useRampage((state) => state.leaveFabric);
  const reset = useRampage((state) => state.factoryReset);
  const forget = useRampage((state) => state.forgetNode);
  const [action, setAction] = useState<DestructiveAction | null>(null);
  const [confirmation, setConfirmation] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const canConfirm = action !== null && (!action.requiresTyping || confirmation === action.phrase);
  const diagnostics = useMemo(() => status ? JSON.stringify(status, null, 2) : "", [status]);

  if (!open) return null;
  const begin = (next: DestructiveAction) => {
    setAction(next);
    setConfirmation("");
    setError(null);
  };
  const execute = async () => {
    if (!action || (action.requiresTyping && confirmation !== action.phrase)) return;
    setPending(true);
    setError(null);
    try {
      if (action.kind === "leave") await leave(action.phrase);
      if (action.kind === "reset") await reset(confirmation);
      if (action.kind === "forget") {
        await forget(action.nodeId, action.phrase);
        setAction(null);
        setConfirmation("");
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Recovery action failed.");
    } finally {
      setPending(false);
    }
  };

  return <div className="recovery-backdrop" role="dialog" aria-modal="true" aria-labelledby="recovery-title">
    <section className="recovery-center">
      <header>
        <div><RotateCcw size={20} /><div><p className="eyebrow">RECOVERY CENTER</p><h1 id="recovery-title">Repair, leave, or start clean.</h1></div></div>
        <button aria-label="Close Recovery Center" onClick={() => setOpen(false)}><X size={18} /></button>
      </header>
      {!status ? <div className="recovery-loading"><RefreshCw className="spin" /><span>Inspecting local identity, sidecars, and enrollment…</span></div> : <>
        <div className={`recovery-health ${status.healthy ? "healthy" : "attention"}`}>
          {status.healthy ? <Check size={18} /> : <AlertTriangle size={18} />}
          <div><strong>{status.healthy ? "Local lifecycle is consistent" : "Recovery attention required"}</strong><span>Rampage {status.version} · {status.role} · {status.state.replaceAll("_", " ")}</span></div>
          <button onClick={() => void refresh()}><RefreshCw size={14} /> Run checks</button>
        </div>
        {status.issues.length > 0 && <ul className="recovery-issues">{status.issues.map((issue) => <li key={issue}>{issue}</li>)}</ul>}

        <div className="recovery-simple-actions">
          <button className="recovery-primary" onClick={() => void repair().catch((reason: unknown) => setError(reason instanceof Error ? reason.message : "Restart failed."))}><RefreshCw size={18} /><span><strong>Fix Rampage</strong><small>Restart safely without losing anything</small></span></button>
          {status.canLeaveFabric && <button onClick={() => begin({ kind: "leave", phrase: "LEAVE FABRIC", requiresTyping: false, title: "Start pairing over?", detail: "Rampage will stop sharing, remove this laptop’s old pairing, and return to the two-button setup screen. The app and installed AI models stay installed." })}><Unplug size={18} /><span><strong>Pair again</strong><small>Remove this laptop’s old connection</small></span></button>}
        </div>

        <details className="recovery-advanced">
          <summary>Advanced recovery and enrolled devices</summary>
          {status.role === "owner" && <section className="recovery-devices">
          <div><HardDrive size={17} /><div><strong>Enrolled machines</strong><span>Forget stale devices so old identities cannot reconnect.</span></div></div>
          {status.nodes.map((node) => <article key={node.nodeId}>
            <i className={node.live ? "live" : "offline"} />
            <div><strong>{node.displayName}</strong><span>{node.platform} · {node.live ? "live signed offer" : "offline"}</span></div>
            {node.local ? <small>OWNER LOCAL</small> : <button onClick={() => begin({ kind: "forget", nodeId: node.nodeId, phrase: `FORGET ${node.nodeId}`, requiresTyping: false, title: `Forget ${node.displayName}?`, detail: "Rampage will revoke its offers, outstanding work, Remote Assist sessions, artifact locations, and future access under this identity." })}><ShieldX size={14} /> Forget</button>}
          </article>)}
          </section>}
          <div className="recovery-actions">
            <article><Trash2 size={19} /><div><strong>Factory reset</strong><p>Erase this device’s Rampage runtime, identities, local evidence, encrypted caches, and auto-start setting. Installed models outside Rampage are untouched.</p></div><button className="danger" onClick={() => begin({ kind: "reset", phrase: "RESET RAMPAGE", requiresTyping: true, title: "Reset this Rampage device?", detail: "This is the full local factory reset. On an owner PC it also erases the fabric ledger and enrolled-device records." })}>Factory reset</button></article>
          </div>
          <button className="copy-diagnostics" onClick={() => void navigator.clipboard.writeText(diagnostics)}><Clipboard size={14} /> Copy redacted recovery receipt</button>
        </details>
      </>}
      {action && <div className="recovery-confirm">
        <ShieldX size={26} />
        <h2>{action.title}</h2>
        <p>{action.detail}</p>
        {action.requiresTyping && <label>Type <strong>{action.phrase}</strong> to continue<input autoFocus value={confirmation} onChange={(event) => setConfirmation(event.currentTarget.value)} /></label>}
        {error && <p className="form-error" role="alert">{error}</p>}
        <div><button onClick={() => { setAction(null); setConfirmation(""); setError(null); }}>Cancel</button><button className="danger" disabled={!canConfirm || pending} onClick={() => void execute()}>{pending ? "Applying…" : action.kind === "forget" ? "Forget machine" : action.kind === "leave" ? "Leave fabric" : "Reset Rampage"}</button></div>
      </div>}
      {!action && error && <p className="recovery-error" role="alert">{error}</p>}
    </section>
  </div>;
}
