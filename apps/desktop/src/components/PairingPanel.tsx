import { Check, Laptop, Radar, ShieldCheck, X } from "lucide-react";
import { useRampage } from "../store";

export function PairingPanel() {
  const windowState = useRampage((state) => state.pairingWindow);
  const approve = useRampage((state) => state.approvePairing);
  const reject = useRampage((state) => state.rejectPairing);
  if (!windowState?.requests.length) return null;
  return (
    <section className="pairing-panel" aria-live="polite" aria-label="Nearby laptop pairing">
      <header>
        <Radar size={17} />
        <div><strong>NEW MACHINE FOUND</strong><span>Approve the device you expect</span></div>
      </header>
      {windowState.requests.map((request) => <article className="pairing-request" key={request.request_id}>
        <div className="pairing-device"><Laptop size={18} /><div><strong>{request.device_name}</strong><span>{request.device_kind} · nearby request</span></div></div>
        <p><ShieldCheck size={14} /> This one-time request expires automatically. Approve only the machine you just told to join.</p>
        {request.state === "completed"
          ? <div className="pairing-approved"><Check size={15} /> Connected securely. The laptop is restarting into your fabric.</div>
          : request.state === "approved"
          ? <div className="pairing-approved"><Check size={15} /> Approval sent—finishing secure enrollment…</div>
          : <div className="pairing-actions">
            <button type="button" className="pairing-deny" onClick={() => void reject(request.request_id)}><X size={14} /> Not mine</button>
            <button type="button" className="pairing-approve" onClick={() => void approve(request.request_id)}><Check size={14} /> Approve this machine</button>
          </div>}
      </article>)}
    </section>
  );
}
