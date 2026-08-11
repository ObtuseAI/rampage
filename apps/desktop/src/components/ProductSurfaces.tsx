import { Activity, ArrowUpRight, BrainCircuit, CheckCircle2, Gauge, Network, RefreshCw, ShieldCheck, Sparkles, Timer, TrendingUp, Zap } from "lucide-react";
import { ComputeStrategyPanel } from "./ComputeStrategyPanel";
import { useRampage } from "../store";

function heading(id: string, eyebrow: string, title: string, copy: string) {
  return <div className="surface-heading"><div><p className="eyebrow">{eyebrow}</p><h1 id={id}>{title}</h1><p>{copy}</p></div></div>;
}

function compactEventName(value: string) {
  return value.replaceAll(".", " / ").replaceAll("_", " ");
}

function dividendPoints(scales: number[]) {
  if (!scales.length) return "";
  const minimum = Math.min(...scales);
  const maximum = Math.max(...scales);
  const span = Math.max(maximum - minimum, 0.05);
  return scales.map((scale, index) => {
    const x = scales.length === 1 ? 50 : (index / (scales.length - 1)) * 100;
    const y = 34 - ((scale - minimum) / span) * 28;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(" ");
}

function duration(ms: number | null) {
  if (ms === null) return "—";
  if (ms < 1_000) return `${ms} ms`;
  return `${(ms / 1_000).toFixed(ms < 10_000 ? 1 : 0)} s`;
}

export function WorkSurface() {
  const state = useRampage();
  const dividend = state.fabricBenchmark;
  const working = state.nodes.filter((node) => node.state === "working").length;
  const ready = state.nodes.filter((node) => node.state === "ready").length;
  const measured = state.nodes.filter((node) => node.topologyConfidence === "measured").length;
  const historyScales = state.dividendHistory.map((record) => record.result.effective_scale_over_fastest_node);
  const historyPoints = dividendPoints(historyScales);
  const planner = state.breakEvenPlan;
  const network = state.networkAutopilot;
  const remotePaths = network?.nodes.filter((node) => node.preferred_path !== "controller_local") ?? [];
  const directPaths = remotePaths.filter((node) => node.preferred_path === "direct_measured" || node.preferred_path === "direct_candidate").length;
  const relayPaths = remotePaths.filter((node) => node.preferred_path === "owner_relay_measured" || node.preferred_path === "owner_relay_bootstrap").length;
  const interactiveReady = remotePaths.filter((node) => node.traffic.some((traffic) => traffic.traffic_class === "interactive_ai" && traffic.admitted)).length;
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
      <div className="dividend-history">
        <span><TrendingUp /> Durable history</span>
        <svg viewBox="0 0 100 40" role="img" aria-label={`${historyScales.length} durable Compute Dividend measurement${historyScales.length === 1 ? "" : "s"}`} preserveAspectRatio="none">
          <polyline points={historyPoints} />
          {historyScales.map((_, index) => {
            const [x, y] = historyPoints.split(" ")[index]?.split(",") ?? ["0", "34"];
            return <circle key={state.dividendHistory[index]?.ledger_sequence ?? index} cx={x} cy={y} r="1.8" />;
          })}
        </svg>
        <strong>{state.dividendHistory.length ? `${state.dividendHistory.length} chained` : "Current session"}</strong>
      </div>
      <small><ShieldCheck /> Derived only from concurrent signed sustained CPU receipts. It is not a claim that every workload or the PC itself becomes this much faster.</small>
    </article>}
    <div className="autopilot-grid" aria-label="Automatic placement and network decisions">
      <article className={`autopilot-card ${planner?.decision ?? "insufficient_evidence"}`}>
        <div className="card-title"><Timer /><div><p className="eyebrow">P90 BREAK-EVEN BRAIN</p><h2>{planner?.decision === "use_fabric" ? "Fabric clears the gate" : planner?.decision === "stay_on_fastest_node" ? "Fastest node wins" : "Waiting for proof"}</h2></div></div>
        <p>{planner?.reason ?? "Run a signed sustained benchmark to unlock evidence-gated placement."}</p>
        <div className="autopilot-metrics"><span>Fastest node <strong>{duration(planner?.p90_baseline_ms ?? null)}</strong></span><span>Fabric <strong>{duration(planner?.p90_fabric_ms ?? null)}</strong></span><span>Required gain <strong>{planner ? `${planner.required_gain_percent.toFixed(0)}%` : "—"}</strong></span></div>
        <small>Conservative proof-target projection; slower or unmeasured distributed plans are refused automatically.</small>
      </article>
      <article className="autopilot-card network-autopilot">
        <div className="card-title"><Network /><div><p className="eyebrow">NETWORK AUTOPILOT</p><h2>{network ? `${directPaths} direct · ${relayPaths} relay` : "Collecting paths"}</h2></div></div>
        <p>{network ? `${interactiveReady}/${remotePaths.length} remote path${remotePaths.length === 1 ? "" : "s"} currently clear the interactive traffic gate.` : "Rampage will retain an authenticated owner relay for recovery and upgrade only from measured path evidence."}</p>
        <div className="path-chips">
          {remotePaths.length ? remotePaths.map((node) => <span key={node.node_id} className={node.preferred_path}>{state.nodes.find((candidate) => candidate.id === node.node_id)?.name ?? node.node_id.slice(0, 8)} · {node.preferred_path.replaceAll("_", " ")}</span>) : <span className="recovering">No remote path evidence yet</span>}
        </div>
        <small>Control stays available on authenticated fallback paths; AI, remote media, artifacts, and bulk work each have separate measured thresholds.</small>
      </article>
    </div>
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
