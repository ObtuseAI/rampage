import { ArrowRight, Check, Laptop, Radar, ShieldCheck, Sparkles, X, Zap } from "lucide-react";
import { useState } from "react";
import { useRampage } from "../store";

const steps = [
  { icon: Sparkles, title: "Name your fabric", body: "Rampage discovers this machine first. Nothing is shared publicly." },
  { icon: ShieldCheck, title: "Your machine stays yours", body: "Rampage keeps at least one CPU core, 512 MiB memory, and a 10% safety guardband outside the shared pool." },
  { icon: Zap, title: "Invite when ready", body: "Add trusted devices with a short-lived invite and local confirmation on both screens." },
];

export function Onboarding() {
  const finish = useRampage((state) => state.finishOnboarding);
  const joinFabric = useRampage((state) => state.joinFabric);
  const beginPairing = useRampage((state) => state.beginPairing);
  const cancelPairing = useRampage((state) => state.cancelPairing);
  const pairing = useRampage((state) => state.workerPairing);
  const [step, setStep] = useState(0);
  const [choice, setChoice] = useState<"owner" | "worker">("owner");
  const [invitation, setInvitation] = useState("");
  const [error, setError] = useState<string | null>(null);
  const item = steps[step];
  const joining = step === 0 && choice === "worker";
  const Icon = joining ? Laptop : item.icon;
  const title = joining ? "Join your fabric" : item.title;
  const body = joining
    ? "Rampage finds your main PC securely over your private network."
    : item.body;
  return (
    <div className="onboarding-backdrop" role="dialog" aria-modal="true" aria-labelledby="onboarding-title">
      <section className="onboarding">
        <div className="brand-mark large">R</div>
        <p className="eyebrow">RAMPAGE / FIRST RUN</p>
        <Icon className="onboarding-icon" size={40} />
        <h1 id="onboarding-title">{title}</h1>
        <p>{body}</p>
        {step === 0 && <>
          <div className="choice-row" role="group" aria-label="First run mode">
            <button className={choice === "owner" ? "choice active" : "choice"} onClick={() => setChoice("owner")}>Create my fabric</button>
            <button className={choice === "worker" ? "choice active" : "choice"} onClick={() => setChoice("worker")}>Join my fabric</button>
          </div>
          {choice === "owner"
            ? <input aria-label="Fabric name" defaultValue="My Rampage" autoFocus />
            : <div className="join-flow">
              {pairing.state === "idle" && <div className="join-state"><Laptop size={24} /><strong>This laptop will wait for your main PC.</strong><span>Nothing needs to be copied, typed, or configured. Keep both machines on the same private network.</span></div>}
              {pairing.state === "searching" && <div className="join-state active"><Radar size={24} /><strong>Looking for your main PC…</strong><span>On the main PC open Rampage and choose <b>Add machine</b>. If Windows asks, allow Rampage on private networks only.</span><i className="pairing-pulse" aria-hidden="true" /></div>}
              {pairing.state === "waiting_approval" && <div className="join-state active"><ShieldCheck size={24} /><strong>Compare this code on the main PC</strong><output aria-label={`Verification code ${pairing.verification_code.split("").join(" ")}`}>{pairing.verification_code}</output><span>Leave this laptop here. Approve only when both screens show the same digits.</span></div>}
              {pairing.state === "approved" && <div className="join-state success"><Check size={24} /><strong>Approved</strong><span>Rampage is securely enrolling this laptop and will restart automatically.</span></div>}
              {pairing.state === "failed" && <div className="join-state failed"><X size={24} /><strong>Pairing needs another try</strong><span>{pairing.message}</span><ul><li>Keep both machines on the same Wi-Fi or Ethernet network.</li><li>Choose <b>Add machine</b> on the main PC.</li><li>Allow Rampage on private networks if Windows asks.</li></ul></div>}
              {(pairing.state === "searching" || pairing.state === "waiting_approval") && <button type="button" className="cancel-join" onClick={() => void cancelPairing()}>Cancel</button>}
              <details className="manual-invite">
                <summary>Advanced: use a complete invite</summary>
                <textarea aria-label="Signed Rampage invite" value={invitation} onChange={(event) => setInvitation(event.target.value)} placeholder="Paste a complete signed invite" />
                <button type="button" onClick={() => void joinFabric(invitation).catch((reason: unknown) => setError(reason instanceof Error ? reason.message : "Could not join this fabric."))}>Use complete invite</button>
              </details>
            </div>}
          {error && <p className="form-error" role="alert">{error}</p>}
        </>}
        {step === 1 && <div className="reserve"><span><Check size={14} /> Adaptive reserve</span><strong>Recommended</strong></div>}
        {step === 2 && <div className="privacy-note">Local-only is already useful. You can skip invitations and add devices later.</div>}
        <div className="step-dots" aria-label={`Step ${step + 1} of ${steps.length}`}>{steps.map((_, index) => <i key={index} className={index === step ? "active" : ""} />)}</div>
        <button className="primary" disabled={step === 0 && choice === "worker" && pairing.state !== "idle" && pairing.state !== "failed"} onClick={() => {
          if (step === 0 && choice === "worker") {
            if (pairing.state === "idle" || pairing.state === "failed") {
              setError(null);
              void beginPairing().catch((reason: unknown) => setError(reason instanceof Error ? reason.message : "Could not start nearby pairing."));
            }
            return;
          }
          step < steps.length - 1 ? setStep(step + 1) : finish();
        }}>
          {step === 0 && choice === "worker"
            ? pairing.state === "idle" || pairing.state === "failed" ? "Find my fabric" : pairing.state === "approved" ? "Finishing securely…" : "Waiting for main PC…"
            : step < steps.length - 1 ? "Continue" : "Enter Rampage"} <ArrowRight size={17} />
        </button>
      </section>
    </div>
  );
}
