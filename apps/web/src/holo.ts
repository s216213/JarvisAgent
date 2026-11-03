import * as THREE from "three";

export type HoloNode = { id: string; label: string; group?: string };
export type HoloEdge = { from: string; to: string };
export type HoloScene = {
  nodes?: HoloNode[];
  edges?: HoloEdge[];
  ghost?: HoloNode[];
  text?: string;
  map?: { lat?: number; lon?: number; zoom?: number };
};

const tint: Record<string, string> = {
  core: "#7fdbff",
  memory: "#7fdbff",
  tool: "#8aa0b2",
  plugin: "#e7f6ff",
  history: "#e8a54b",
  ghost: "#e8a54b",
};

function sprite(text: string, color: string) {
  const canvas = document.createElement("canvas");
  canvas.width = 512;
  canvas.height = 96;
  const ctx = canvas.getContext("2d");
  if (!ctx) return new THREE.Sprite();
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = color;
  ctx.font = "28px Consolas, monospace";
  ctx.fillText(text.slice(0, 32), 12, 58);
  const material = new THREE.SpriteMaterial({ map: new THREE.CanvasTexture(canvas), transparent: true });
  const item = new THREE.Sprite(material);
  item.scale.set(3.2, 0.6, 1);
  return item;
}

export function paintHolo(scene: THREE.Scene, data: HoloScene) {
  scene.clear();
  scene.add(new THREE.AmbientLight(0xffffff, 1));
  const nodes = [...(data.nodes ?? []), ...(data.ghost ?? []).map((node) => ({ ...node, group: "ghost" }))];
  const placed = new Map<string, THREE.Vector3>();
  nodes.forEach((node, index) => {
    const ring = node.group === "ghost" ? 6.5 : 3.2 + (index % 4);
    const angle = (index / Math.max(nodes.length, 1)) * Math.PI * 2;
    const position = new THREE.Vector3(Math.cos(angle) * ring, (index % 5) * 0.35 - 0.7, Math.sin(angle) * ring);
    placed.set(node.id, position);
    const color = tint[node.group ?? "core"] ?? "#7fdbff";
    const mesh = new THREE.Mesh(
      new THREE.SphereGeometry(node.group === "ghost" ? 0.08 : 0.12, 16, 16),
      new THREE.MeshBasicMaterial({ color, transparent: true, opacity: node.group === "ghost" ? 0.55 : 1 }),
    );
    mesh.position.copy(position);
    scene.add(mesh);
    const tag = sprite(node.label, color);
    tag.position.copy(position);
    tag.position.y += 0.35;
    scene.add(tag);
  });
  const lines: number[] = [];
  for (const edge of data.edges ?? []) {
    const from = placed.get(edge.from);
    const to = placed.get(edge.to);
    if (!from || !to) continue;
    lines.push(from.x, from.y, from.z, to.x, to.y, to.z);
  }
  if (lines.length) {
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute("position", new THREE.Float32BufferAttribute(lines, 3));
    scene.add(new THREE.LineSegments(geometry, new THREE.LineBasicMaterial({ color: 0x1e3344 })));
  }
}
