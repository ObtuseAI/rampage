import { Billboard, Line, OrbitControls, Text } from "@react-three/drei";
import { Canvas } from "@react-three/fiber";
import { useMemo } from "react";
import { useRampage } from "../store";
import type { FabricNode } from "../types";

const colors = {
  ready: "#78f7c5",
  working: "#8da5ff",
  sleeping: "#e7a85a",
  offline: "#5b6170",
};

function Node({ node }: { node: FabricNode }) {
  const selected = useRampage((state) => state.selectedNode === node.id);
  const select = useRampage((state) => state.setSelectedNode);
  return (
    <group position={[node.x, node.y, node.z]}>
      <mesh
        onClick={(event) => {
          event.stopPropagation();
          select(node.id);
        }}
        scale={selected ? 1.22 : 1}
      >
        <icosahedronGeometry args={[0.52, 2]} />
        <meshStandardMaterial color={colors[node.state]} emissive={colors[node.state]} emissiveIntensity={selected ? 1.4 : 0.5} roughness={0.28} metalness={0.62} />
      </mesh>
      <mesh scale={selected ? 1.55 : 1.3}>
        <icosahedronGeometry args={[0.52, 1]} />
        <meshBasicMaterial color={colors[node.state]} wireframe transparent opacity={0.18} />
      </mesh>
      <Billboard position={[0, -0.86, 0]} follow lockX={false} lockY={false} lockZ={false}>
        <Text fontSize={0.21} color="#dce7ff" anchorX="center" anchorY="middle">
          {node.name}
        </Text>
      </Billboard>
    </group>
  );
}

function Fabric() {
  const nodes = useRampage((state) => state.nodes);
  const reducedMotion = useRampage((state) => state.reducedMotion);
  const links = useMemo(() => nodes.slice(1).map((node) => ({ from: nodes[0], to: node })), [nodes]);
  return (
    <>
      <ambientLight intensity={0.45} />
      <pointLight position={[0, 5, 1]} color="#7df9cb" intensity={22} />
      <pointLight position={[-4, -2, 3]} color="#6a76ff" intensity={15} />
      <gridHelper args={[16, 32, "#222b45", "#121827"]} position={[0, -1.4, 0]} />
      {links.map(({ from, to }) => (
        <Line key={to.id} points={[[from.x, from.y, from.z], [to.x, to.y, to.z]]} color={colors[to.state]} lineWidth={Math.max(0.7, Math.min(4, Math.log10((to.downlinkMbps ?? 1) + 1)))} transparent opacity={to.topologyConfidence === "measured" ? 0.62 : 0.28} dashed={to.state === "sleeping" || to.topologyConfidence === "unmeasured"} dashScale={2} />
      ))}
      {nodes.map((node) => <Node key={node.id} node={node} />)}
      <OrbitControls enablePan={false} minDistance={5.5} maxDistance={11} autoRotate={!reducedMotion} autoRotateSpeed={0.32} />
    </>
  );
}

export function Arena() {
  return (
    <div className="arena" aria-label="Spatial fabric view. Use the Ops Grid for keyboard-accessible node controls.">
      <Canvas camera={{ position: [0, 4.8, 7.8], fov: 48 }} dpr={[1, 1.7]} frameloop="always">
        <color attach="background" args={["#07090f"]} />
        <fog attach="fog" args={["#07090f", 7, 16]} />
        <Fabric />
      </Canvas>
      <div className="gravity-lens" aria-hidden="true"><span>GRAVITY LENS</span><strong>WHOLE-WORKLOAD FIT</strong></div>
    </div>
  );
}
