import { BatteryCharging, Cpu, Database, MemoryStick, Network } from "lucide-react";
import { useRampage } from "../store";

export function OpsGrid() {
  const nodes = useRampage((state) => state.nodes);
  const selected = useRampage((state) => state.selectedNode);
  const setSelected = useRampage((state) => state.setSelectedNode);
  return (
    <div className="ops-grid" aria-label="Fabric nodes">
      {nodes.map((node) => (
        <button key={node.id} className={`node-card ${selected === node.id ? "selected" : ""}`} onClick={() => setSelected(node.id)} aria-pressed={selected === node.id}>
          <span className={`state-dot ${node.state}`} />
          <span className="node-heading"><strong>{node.name}</strong><small>{node.kind.replace("_", " ")}{node.modelRuntimeCount ? ` · ${node.modelMemoryAvailableGb}G model` : ""}</small></span>
          <span className="meter-row"><Cpu size={15} /><span>CPU</span><meter min="0" max="100" value={node.cpu}>{node.cpu}%</meter><b>{node.cpu}%</b></span>
          <span className="meter-row"><MemoryStick size={15} /><span>RAM</span><meter min="0" max="100" value={node.memory}>{node.memory}%</meter><b>{node.memory}%</b></span>
          <span className="meter-row"><Database size={15} /><span>GPU</span><meter min="0" max="100" value={node.gpu}>{node.gpu}%</meter><b>{node.gpu}%</b></span>
          <span className="meter-row"><Database size={15} /><span>DISK</span><meter min="0" max="100" value={node.storage}>{node.storage}%</meter><b>{node.storageAvailableGb}G</b></span>
          <span className="meter-row"><Network size={15} /><span>LINK</span><span>{node.topologyConfidence === "measured" ? `${node.latencyMs} ms authenticated QUIC` : node.topologyConfidence === "controller_local" ? "local fabric" : "awaiting benchmark"}</span><b>{node.downlinkMbps ? `${node.downlinkMbps}M` : "—"}</b></span>
          <span className="node-foot"><BatteryCharging size={14} /> {node.state === "sleeping" ? "Charging-only worker" : "Owner policy active"}</span>
        </button>
      ))}
    </div>
  );
}
