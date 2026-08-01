import { ArrowRight, Check, ShieldCheck, Sparkles, Zap } from "lucide-react";
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
  const [step, setStep] = useState(0);
  const [choice, setChoice] = useState<"owner" | "worker">("owner");
  const [invitation, setInvitation] = useState("");
  const [error, setError] = useState<string | null>(null);
  const item = steps[step];
  const Icon = item.icon;
  return (
    <div className="onboarding-backdrop" role="dialog" aria-modal="true" aria-labelledby="onboarding-title">
      <section className="onboarding">
        <div className="brand-mark large">R</div>
        <p className="eyebrow">RAMPAGE / FIRST RUN</p>
        <Icon className="onboarding-icon" size={40} />
        <h1 id="onboarding-title">{item.title}</h1>
        <p>{item.body}</p>
        {step === 0 && <>
          <div className="choice-row" role="group" aria-label="First run mode">
            <button className={choice === "owner" ? "choice active" : "choice"} onClick={() => setChoice("owner")}>Create my fabric</button>
            <button className={choice === "worker" ? "choice active" : "choice"} onClick={() => setChoice("worker")}>Join a fabric</button>
          </div>
          {choice === "owner"
            ? <input aria-label="Fabric name" defaultValue="My Rampage" autoFocus />
            : <textarea aria-label="Signed Rampage invite" value={invitation} onChange={(event) => setInvitation(event.target.value)} placeholder="Paste the complete signed invite from the owner device" autoFocus />}
          {error && <p className="form-error" role="alert">{error}</p>}
        </>}
        {step === 1 && <div className="reserve"><span><Check size={14} /> Adaptive reserve</span><strong>Recommended</strong></div>}
        {step === 2 && <div className="privacy-note">Local-only is already useful. You can skip invitations and add devices later.</div>}
        <div className="step-dots" aria-label={`Step ${step + 1} of ${steps.length}`}>{steps.map((_, index) => <i key={index} className={index === step ? "active" : ""} />)}</div>
        <button className="primary" onClick={() => {
          if (step === 0 && choice === "worker") {
            void joinFabric(invitation).catch((reason: unknown) => setError(reason instanceof Error ? reason.message : "Could not join this fabric."));
            return;
          }
          step < steps.length - 1 ? setStep(step + 1) : finish();
        }}>
          {step < steps.length - 1 ? "Continue" : "Enter Rampage"} <ArrowRight size={17} />
        </button>
      </section>
    </div>
  );
}
