import { Activity, ArrowUpRight, BrainCircuit, CheckCircle2, Gauge, RefreshCw, ShieldCheck, Sparkles, Zap } from "lucide-react";
import { ComputeStrategyPanel } from "./ComputeStrategyPanel";
import { useRampage } from "../store";

function heading(id: string, eyebrow: string, title: string, copy: string) {
  return <div className="surface-heading"><div><p className="eyebrow">{eyebrow}</p><h1 id={id}>{title}</h1><p>{copy}</p></div></div>;
}

function compactEventName(value: string) {
  return value.replaceAll(".", " / ").replaceAll("_", " ");
}

export function WorkSurface() {
  const state = useRampage();
  const dividend = state.fabricBenchmark;
  const working = state.nodes.filter((node) => node.state === "working").length;
  const ready = state.nodes.filter((node) => node.state === "ready").length;
  const measured = state.nodes.filter((node) => node.topologyConfidence === "measured").length;
  const recentWork = state.events.filter((event) => event.event_type.startsWith("job.") || event.event_type.includes("benchmark")).slice(-6).reverse();
  return <section className="product-surface" aria-labelledby="work-title">
    {heading("work-title", "WORK ORCHESTRATOR", "Put the whole fabric to work.", "Rampage selects qualified machines, preserves owner reserves, and records every admitted result.")}
    <div className="metric-strip" aria-label="Work readiness">
      <article><span>Ready now</span><strong>{ready}</strong><small>verified nodes</small></article>
      <article><span>Working</span><strong>{working}</strong><small>active nodes</small></article>
      <article><span>Measured links</span><strong>{measured}/{state.nodes.length}</strong><small>placement confidence</small></article>
      <article><span>Time returned</span><strong>{dividend ? `${dividend.time_returned_hours_per_100.toFixed(1)}h` : "—"}</strong><small>per 100h of matched work</small></article>
    </div>
    {dividend && <article className="compute-dividend" aria-label="Verified compute dividend">
      <div className="dividend-lead"><div className="card-icon"><Gauge /></div><div><p className="eyebrow">VERIFIED COMPUTE DIVIDEND</p><h2>{dividend.time_returned_hours_per_100.toFixed(1)} hours returned per 100</h2><p>For fully divisible CPU work matching this proof, your fabric finished the same work in an estimated {dividend.estimated_time_saved_percent.toFixed(1)}% less time than its fastest machine alone.</p></div></div>
      <div className="dividend-metrics">
        <div><span>Extra capacity</span><strong>+{dividend.verified_extra_capacity_percent.toFixed(1)}%</strong></div>
        <div><span>Effective scale</span><strong>{dividend.effective_scale_over_fastest_node.toFixed(2)}×</strong></div>
        <div><span>Signed machines</span><strong>{dividend.nodes.length}</strong></div>
        <div><span>Proof rate</span><strong>{(dividend.fabric_hashes_per_second / 1_000_000).toFixed(2)} MH/s</strong></div>
      </div>
      <small><ShieldCheck /> Derived only from concurrent signed sustained CPU receipts. It is not a claim that every workload or the PC itself becomes this much faster.</small>
    </article>}
    {state.fabricRole === "owner" && <ComputeStrategyPanel />}
    <div className="surface-grid two-column">
      <article className="surface-card featured">
        <div className="card-icon"><Zap /></div><div><p className="eyebrow">LIVE FABRIC PROOF</p><h2>Make every active machine prove useful work.</h2><p>A sustained benchmark dispatches governed shards, verifies signed receipts, and reports the effective scale over the fastest individual node.</p></div>
        <button disabled={!state.connected || state.fabricBenchmarkPending} onClick={() => void state.runFabricBenchmark()}>{state.fabricBenchmarkPending ? <RefreshCw className="spin" /> : <Activity />} {state.fabricBenchmarkPending ? "Proving…" : "Run sustained benchmark"}</button>
      </article>
      <article className="surface-card">
        <div className="card-title"><ShieldCheck /><div><p className="eyebrow">ADMISSION QUEUE</p><h2>Recent governed work</h2></div></div>
        <div className="activity-list">
          {recentWork.length ? recentWork.map((event) => <div key={event.sequence}><i /><span><strong>{compactEventName(event.event_type)}</strong><small>{new Date(event.recorded_at).toLocaleTimeString()} · #{event.sequence}</small></span></div>) : <div className="empty-state"><CheckCircle2 /><span><strong>Queue clear</strong><small>No admitted work is waiting.</small></span></div>}
        </div>
      </article>
    </div>
  </section>;
}

export function EvolutionSurface() {
  const state = useRampage();
  const diagnostic = state.diagnostic;
  const warnings = diagnostic?.findings.filter((finding) => finding.severity !== "info") ?? [];
  return <section className="product-surface" aria-labelledby="evolution-title">
    {heading("evolution-title", "AUTONOMOUS EVOLUTION", "A fabric that finds its own limits.", "Continuous evidence detects bottlenecks and applies only changes inside the owner-defined authority envelope.")}
    <div className="evolution-hero">
      <div className="health-orbit" style={{ "--health": `${(diagnostic?.health_score ?? 0) * 3.6}deg` } as React.CSSProperties}><strong>{diagnostic?.health_score ?? "—"}</strong><span>FABRIC HEALTH</span></div>
      <div><p className="eyebrow">SELF-SCAN STATE</p><h2>{diagnostic ? diagnostic.status.replaceAll("_", " ") : "Awaiting verified evidence"}</h2><p>{warnings[0]?.evidence ?? diagnostic?.findings[0]?.evidence ?? "Rampage is collecting controller evidence before changing placement or optimization behavior."}</p><div className="authority-chip"><ShieldCheck /> Authority expansion is always denied</div></div>
      <button onClick={() => void state.refresh()}><RefreshCw /> Scan now</button>
    </div>
    <div className="surface-grid three-column">
      <article className="surface-card"><div className="card-title"><BrainCircuit /><div><p className="eyebrow">LOCAL AI</p><h2>{state.localAiRuntime.state.replaceAll("_", " ")}</h2></div></div><p>{state.localAiRuntime.message}</p><span className={`state-line ${state.localAiRuntime.state}`}>{state.localAiRuntime.modelId}</span></article>
      <article className="surface-card"><div className="card-title"><Gauge /><div><p className="eyebrow">PLACEMENT BRAIN</p><h2>{state.computeStrategy.replaceAll("_", " ")}</h2></div></div><p>{state.modelPlan?.reason ?? "The next model plan will be generated from fresh signed capacity and measured topology."}</p><span className="state-line">{state.modelPlan ? `${state.modelPlan.placements.length} placements · ${state.modelPlan.parallelism?.replaceAll("_", " ") ?? "preview"}` : "Evidence pending"}</span></article>
      <article className="surface-card"><div className="card-title"><Sparkles /><div><p className="eyebrow">FINDINGS</p><h2>{diagnostic?.findings.length ?? 0} active observations</h2></div></div><div className="finding-stack">{diagnostic?.findings.slice(0, 3).map((finding) => <div key={finding.code} className={finding.severity}><strong>{finding.code.replaceAll("_", " ")}</strong><span>{finding.scope}</span></div>) ?? <p>No verified findings yet.</p>}</div></article>
    </div>
  </section>;
}

export function EvidenceSurface() {
  const state = useRampage();
  const events = state.events.slice(-18).reverse();
  return <section className="product-surface evidence-surface" aria-labelledby="evidence-title">
    {heading("evidence-title", "EVIDENCE SPINE", "Every useful action leaves a receipt.", "Inspect the local append-only record behind enrollment, compute placement, resource offers, diagnostics, and encrypted artifacts.")}
    <div className="evidence-summary">
      <article><ShieldCheck /><div><span>Controller state</span><strong>{state.connected ? "Verified live" : "Reduced"}</strong></div></article>
      <article><Activity /><div><span>Loaded events</span><strong>{state.events.length}</strong></div></article>
      <article><BrainCircuit /><div><span>Self-scan digest</span><strong>{state.diagnostic?.evidence_digest.slice(0, 20) ?? "Pending"}</strong></div></article>
      <button onClick={() => void state.refresh()}><RefreshCw /> Refresh evidence</button>
    </div>
    <div className="ledger" aria-label="Verified evidence events">
      <header><span>Sequence</span><span>Event</span><span>Subject</span><span>Recorded</span><span>Integrity</span></header>
      {events.length ? events.map((event) => <article key={event.sequence}><code>#{event.sequence}</code><strong>{compactEventName(event.event_type)}</strong><span>{event.subject_id.length > 24 ? `${event.subject_id.slice(0, 20)}…` : event.subject_id}</span><time>{new Date(event.recorded_at).toLocaleString()}</time><span className="verified"><CheckCircle2 /> chained</span></article>) : <div className="ledger-empty"><ShieldCheck /><h2>No evidence loaded</h2><p>The controller must be live before local evidence can be displayed.</p></div>}
    </div>
    <button className="evidence-export" onClick={() => void navigator.clipboard.writeText(JSON.stringify(state.events, null, 2))}><ArrowUpRight /> Copy loaded evidence JSON</button>
  </section>;
}
