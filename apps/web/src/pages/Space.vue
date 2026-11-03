<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import maplibregl from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { apiBase, authed, socket } from "../api";
import { paintHolo } from "../holo";

type SpaceNode = { id: string; label: string; group?: string };
type SpaceEdge = { from: string; to: string };
type Scene = {
  text?: string;
  nodes?: SpaceNode[];
  edges?: SpaceEdge[];
  ghost?: SpaceNode[];
  map?: { lat?: number; lon?: number; zoom?: number };
};

const router = useRouter();
const host = ref<HTMLElement | null>(null);
const mapHost = ref<HTMLElement | null>(null);
const caption = ref("Składam mapę siebie.");
const error = ref("");
let renderer: THREE.WebGLRenderer | null = null;
let scene: THREE.Scene | null = null;
let camera: THREE.PerspectiveCamera | null = null;
let controls: OrbitControls | null = null;
let frame = 0;
let map: maplibregl.Map | null = null;
let ws: WebSocket | null = null;

function paint(data: Scene) {
  if (!scene) return;
  paintHolo(scene, data);
  if (data.text) caption.value = data.text;
  const lat = data.map?.lat;
  const lon = data.map?.lon;
  if (map && lat != null && lon != null) map.setCenter([lon, lat]);
}

function bootMap(data: Scene) {
  if (!mapHost.value) return;
  const lat = data.map?.lat ?? 52.23;
  const lon = data.map?.lon ?? 21.01;
  map?.remove();
  map = new maplibregl.Map({
    container: mapHost.value,
    style: {
      version: 8,
      sources: {
        osm: { type: "raster", tiles: ["https://tile.openstreetmap.org/{z}/{x}/{y}.png"], tileSize: 256 },
      },
      layers: [{ id: "osm", type: "raster", source: "osm" }],
    },
    center: [lon, lat],
    zoom: data.map?.zoom ?? 11,
  });
  new maplibregl.Marker({ color: "#7FDBFF" }).setLngLat([lon, lat]).addTo(map);
}

function loop() {
  frame = requestAnimationFrame(loop);
  controls?.update();
  if (renderer && scene && camera) renderer.render(scene, camera);
}

onMounted(async () => {
  if (!sessionStorage.getItem("jarvis-auth") || !sessionStorage.getItem("jarvis-session")) {
    router.push("/login");
    return;
  }
  if (!host.value) return;
  scene = new THREE.Scene();
  scene.background = new THREE.Color(0x070b10);
  camera = new THREE.PerspectiveCamera(50, host.value.clientWidth / Math.max(host.value.clientHeight, 1), 0.1, 100);
  camera.position.set(0, 2.2, 8);
  renderer = new THREE.WebGLRenderer({ antialias: true });
  renderer.setSize(host.value.clientWidth, host.value.clientHeight);
  host.value.appendChild(renderer.domElement);
  controls = new OrbitControls(camera, renderer.domElement);
  controls.enablePan = false;
  loop();
  try {
    const response = await fetch(`${apiBase}/api/selfmap`, authed());
    if (response.status === 401) {
      router.push("/login");
      return;
    }
    const data = (await response.json()) as Scene;
    paint(data);
    bootMap(data);
  } catch {
    error.value = "Nie mam mapy siebie.";
  }
  ws = socket("web-space");
  ws.onmessage = (message) => {
    const event = JSON.parse(String(message.data));
    if (event.type === "space_scene") paint(event as Scene);
  };
});

onBeforeUnmount(() => {
  cancelAnimationFrame(frame);
  ws?.close();
  map?.remove();
  controls?.dispose();
  renderer?.dispose();
});
</script>

<template>
  <main class="shell space-page">
    <header class="topbar">
      <span class="mark">Jarvis</span>
      <nav class="nav">
        <a href="/">Pulpit</a>
        <a href="/chat">Czat</a>
        <a href="/space" aria-current="page">Przestrzeń</a>
        <a href="/admin">Admin</a>
      </nav>
    </header>
    <div class="state-line"></div>
    <p class="muted space-caption">{{ caption }}</p>
    <p v-if="error" class="error">{{ error }}</p>
    <div class="space-stage">
      <div ref="host" class="space-holo"></div>
      <div ref="mapHost" class="space-world"></div>
    </div>
  </main>
</template>
