import { Check, Laptop, Radar, ShieldCheck, X } from "lucide-react";
import { useRampage } from "../store";

export function PairingPanel() {
  const windowState = useRampage((state) => state.pairingWindow);
  const approve = useRampage((state) => state.approvePairing);
  const reject = useRampage((state) => state.rejectPairing);
  if (!windowState?.open && !windowState?.requests.length) return null;
  return (
    <section className="pairing-panel" aria-live="polite" aria-label="Nearby laptop pairing">
      <header>
        <Radar size={17} />
        <div><strong>NEARBY PAIRING</strong><span>{windowState.requests.length ? "Laptop found" : "Listening on this private network"}</span></div>
      </header>
      {!windowState.requests.length && <div className="pairing-searching">
        <span className="pairing-pulse" aria-hidden="true" />
        <div><strong>Waiting for the laptop…</strong><p>On the laptop choose “Join my fabric.” No address, account, or invite needs to be copied.</p><small>If Windows asks, allow Rampage on private networks only.</small></div>
      </div>}
      {windowState.requests.map((request) => <article className="pairing-request" key={request.request_id}>
        <div className="pairing-device"><Laptop size={18} /><div><strong>{request.device_name}</strong><span>{request.device_kind} · nearby request</span></div></div>
        <div className="pairing-code" aria-label={`Verification code ${request.verification_code.split("").join(" ")}`}>
          {request.verification_code}
        </div>
        <p><ShieldCheck size={14} /> Confirm these four digits are also visible on the laptop. The real enrollment secret stays encrypted.</p>
        {request.state === "completed"
          ? <div className="pairing-approved"><Check size={15} /> Connected securely. The laptop is restarting into your fabric.</div>
          : request.state === "approved"
          ? <div className="pairing-approved"><Check size={15} /> Approval sent—finishing secure enrollment…</div>
          : <div className="pairing-actions">
            <button type="button" className="pairing-deny" onClick={() => void reject(request.request_id)}><X size={14} /> Not mine</button>
            <button type="button" className="pairing-approve" onClick={() => void approve(request.request_id)}><Check size={14} /> Codes match—approve</button>
          </div>}
      </article>)}
    </section>
  );
}
