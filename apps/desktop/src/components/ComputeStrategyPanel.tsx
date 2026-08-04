import { Activity, Bot, Gamepad2, Gauge, Layers3, Network, Sparkles, Zap } from "lucide-react";
import { useRampage } from "../store";
import type { ComputeStrategy } from "../types";

const strategies: Array<{
  id: ComputeStrategy;
  label: string;
  short: string;
  description: string;
  icon: typeof Layers3;
}> = [
  {
    id: "autonomous_balanced",
    label: "Automatic",
    short: "RAMPAGE CHOOSES",
    description: "Measure every device and path, then continuously choose the safest high-value role without technical setup.",
    icon: Sparkles,
  },
  {
    id: "maximum_model_size",
    label: "Biggest AI",
    short: "BIGGEST AI",
    description: "Combine only compatible, qualified model memory to fit the largest possible local model.",
    icon: Layers3,
  },
  {
    id: "speed_boost",
    label: "Fastest AI",
    short: "FASTEST AI",
    description: "Use tensor peers only when measured links predict faster tokens; otherwise select the fastest whole-model node.",
    icon: Zap,
  },
  {
    id: "maximum_throughput",
    label: "More Work",
    short: "MOST WORK",
    description: "Replicate the model across capable nodes for more simultaneous requests and agent work.",
    icon: Network,
  },
  {
    id: "efficiency",
    label: "Protect This PC",
    short: "GAMING-SAFE",
    description: "Keep foreground CPU, memory, GPU, and network reserves on this PC while moving eligible background work elsewhere.",
    icon: Gamepad2,
  },
];

const gib = 1024 ** 3;
const formatGiB = (bytes: number) => `${(bytes / gib).toFixed(bytes >= 10 * gib ? 0 : 1)} GiB`;

export function ComputeStrategyPanel() {
  const state = useRampage();
  const selected = strategies.find((strategy) => strategy.id === state.computeStrategy)!;
  const plan = state.modelPlan;
  return (
    <section className="compute-deck" aria-label="Compute strategy">
      <header>
        <div>
          <p className="eyebrow">COMPUTE STRATEGY</p>
          <h2>{selected.short}</h2>
        </div>
        <span className={`plan-state ${plan?.state ?? "preview"}`}>
          {state.modelPlanPending ? "MEASURING" : plan?.state.replaceAll("_", " ") ?? "PREVIEW"}
        </span>
      </header>
      <div className="strategy-tabs" role="group" aria-label="Additional compute objective">
        {strategies.map((strategy) => {
          const Icon = strategy.icon;
          return (
            <button
              key={strategy.id}
              className={state.computeStrategy === strategy.id ? "active" : ""}
              aria-pressed={state.computeStrategy === strategy.id}
              onClick={() => state.setComputeStrategy(strategy.id)}
            >
              <Icon size={15} />
              <span>{strategy.label}</span>
              {strategy.id === "autonomous_balanced" && <em>RECOMMENDED</em>}
            </button>
          );
        })}
      </div>
      <div className="strategy-body">
        <div className="strategy-copy">
          <p>{selected.description}</p>
          <span><Gauge size={14} /> {plan?.reason ?? "Connect the controller to calculate a signed-resource placement preview."}</span>
        </div>
        <details className="model-advanced">
          <summary>Advanced model target</summary>
          <div className="model-targets">
            <label>
              Model
              <input list="rampage-installed-models" value={state.targetModelId} onChange={(event) => state.setTargetModelId(event.target.value)} />
              <datalist id="rampage-installed-models">
                {state.gatewayModels.map((model) => <option key={model} value={model} />)}
              </datalist>
            </label>
            <label>
              Weights GiB
              <input type="number" min="1" max="16000" value={state.targetModelGiB} onChange={(event) => state.setTargetModelGiB(event.target.valueAsNumber)} />
            </label>
            <label>
              KV reserve GiB
              <input type="number" min="0" max="1000" value={state.kvCacheGiB} onChange={(event) => state.setKvCacheGiB(event.target.valueAsNumber)} />
            </label>
            <button onClick={() => void state.planModelSession()} disabled={state.modelPlanPending}>
              {state.modelPlanPending ? "Profiling…" : "Plan fabric"}
            </button>
          </div>
        </details>
        <div className="model-metrics" aria-live="polite">
          <div><span>Requested</span><strong>{formatGiB((state.targetModelGiB + state.kvCacheGiB) * gib)}</strong></div>
          <div><span>Visible memory</span><strong>{plan ? formatGiB(plan.observed_fabric_bytes) : "—"}</strong></div>
          <div><span>Compatible max</span><strong>{plan ? formatGiB(plan.maximum_supported_bytes) : "—"}</strong></div>
          <div><span>{state.computeStrategy === "speed_boost" ? "Predicted speed" : "Placement"}</span><strong>{plan ? state.computeStrategy === "speed_boost" ? `${(plan.predicted_speedup_milli / 1000).toFixed(2)}×` : `${plan.placements.length} rank${plan.placements.length === 1 ? "" : "s"}` : "—"}</strong></div>
        </div>
      </div>
      {(plan?.blockers[0] || plan?.warnings[0]) && (
        <div className="strategy-gate"><strong>{plan.blockers.length ? "FENCED" : "NOTE"}</strong><span>{plan.blockers[0] ?? plan.warnings[0]}</span></div>
      )}
      <div className={`local-ai-autopilot ${state.localAiRuntime.state}`} aria-live="polite">
        <Bot size={14} />
        <span>
          <strong>LOCAL AI AUTOPILOT · {state.localAiRuntime.state.replaceAll("_", " ")}</strong>
          {state.localAiRuntime.message}
        </span>
        <small>{state.localAiRuntime.runtimeVersion ? `OLLAMA ${state.localAiRuntime.runtimeVersion}` : state.localAiRuntime.modelId}</small>
      </div>
      {state.fabricRole === "owner" && (
        <div className={`fabric-proof ${state.fabricBenchmark ? "complete" : ""}`} aria-live="polite">
          <Activity size={14} />
          <span>
            <strong>SUSTAINED FABRIC PROOF</strong>
            {state.fabricBenchmark
              ? `${state.fabricBenchmark.nodes.length} signed machine receipt${state.fabricBenchmark.nodes.length === 1 ? "" : "s"} · ${(state.fabricBenchmark.fabric_hashes_per_second / 1_000_000).toFixed(2)} MH/s · ${state.fabricBenchmark.effective_scale_over_fastest_node.toFixed(2)}× fastest node`
              : "Run real bounded CPU work on every connected machine and compare their combined measured rate."}
          </span>
          <button onClick={() => void state.runFabricBenchmark()} disabled={state.fabricBenchmarkPending || !state.connected}>
            {state.fabricBenchmarkPending ? "Working…" : state.fabricBenchmark ? "Run again" : "Prove the fabric"}
          </button>
        </div>
      )}
      <div className={`gateway-ready ${state.gatewayModels.length ? "online" : ""}`}>
        <span><strong>UNIVERSAL AI GATEWAY</strong>{state.gatewayModels.length ? `${state.gatewayModels.length} consistent installed model${state.gatewayModels.length === 1 ? "" : "s"} · OpenAI · Anthropic · OpenRouter ready` : "No eligible whole-model worker"}</span>
        <button onClick={() => void state.copyGatewayConfig()} disabled={!state.gatewayModels.length}>Copy API setup</button>
      </div>
      <footer>One signed execution lane now speaks OpenAI, Anthropic, and OpenRouter-compatible APIs. Cross-host tensor and pipeline launch remain evidence-gated.</footer>
    </section>
  );
}
