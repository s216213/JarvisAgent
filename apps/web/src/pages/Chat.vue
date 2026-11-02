<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import maplibregl from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { socket } from "../api";

type Line = { role: "ja" | "jarvis"; text: string };
type Widget = {
  type: string;
  title?: string;
  body?: string;
  lat?: number;
  lon?: number;
  center?: [number, number];
  zoom?: number;
};
type Card = { id: string; summary: string; body: string };

const router = useRouter();
const lines = ref<Line[]>([]);
const draft = ref("");
const state = ref("idle");
const widget = ref<Widget | null>(null);
const card = ref<Card | null>(null);
const error = ref("");
const mapHost = ref<HTMLElement | null>(null);
let map: maplibregl.Map | null = null;
let ws: WebSocket | null = null;
let buffer = "";

function paintMap(next: Widget) {
  nextTick(() => {
    const lon = next.lon ?? next.center?.[0];
    const lat = next.lat ?? next.center?.[1];
    if (!mapHost.value || lon == null || lat == null) return;
    map?.remove();
    map = new maplibregl.Map({
      container: mapHost.value,
      style: {
        version: 8,
        sources: {
          osm: {
            type: "raster",
            tiles: ["https://tile.openstreetmap.org/{z}/{x}/{y}.png"],
            tileSize: 256,
          },
        },
        layers: [{ id: "osm", type: "raster", source: "osm" }],
      },
      center: [lon, lat],
      zoom: next.zoom ?? 11,
    });
    new maplibregl.Marker({ color: "#7FDBFF" }).setLngLat([lon, lat]).addTo(map);
  });
}

function onMessage(raw: string) {
  const event = JSON.parse(raw);
  if (event.type === "token") {
    state.value = "working";
    buffer += `${event.text} `;
    const last = lines.value.at(-1);
    if (last?.role === "jarvis") last.text = buffer.trim();
    else lines.value.push({ role: "jarvis", text: buffer.trim() });
  }
  if (event.type === "done") {
    buffer = "";
    state.value = lines.value.length ? "idle" : "empty";
  }
  if (event.type === "widget") {
    widget.value = event.widget;
    if (event.widget?.type === "map") paintMap(event.widget);
  }
  if (event.type === "confirm_request") {
    state.value = "consent";
    card.value = { id: event.id, summary: event.summary, body: event.body };
  }
}

function send(payload: unknown) {
  ws?.send(JSON.stringify(payload));
}

function submit() {
  const text = draft.value.trim();
  if (!text) return;
  error.value = "";
  lines.value.push({ role: "ja", text });
  draft.value = "";
  state.value = "working";
  if (text.toLowerCase() === "tak" && card.value) {
    send({ type: "confirm", id: card.value.id, accept: true, via: "text" });
    card.value = null;
  }
  send({ type: "user_text", text });
}

function decide(accept: boolean) {
  if (!card.value) return;
  send({ type: "confirm", id: card.value.id, accept, via: "button" });
  card.value = null;
  state.value = "working";
}

onMounted(() => {
  if (!sessionStorage.getItem("jarvis-auth")) {
    router.push("/login");
    return;
  }
  ws = socket("web-chat");
  ws.onmessage = (message) => onMessage(String(message.data));
  ws.onerror = () => {
    error.value = "Połączenie z rdzeniem nie działa.";
    state.value = "error";
  };
  sendHeartbeat();
});

function sendHeartbeat() {
  if (navigator.geolocation) {
    navigator.geolocation.getCurrentPosition(
      (pos) => send({ type: "presence", speech: false, idle: false, away: false, telemetry: { lat: pos.coords.latitude, lon: pos.coords.longitude } }),
      () => send({ type: "presence", speech: false, idle: false, away: false }),
    );
  }
}

onBeforeUnmount(() => {
  map?.remove();
  ws?.close();
});
</script>

<template>
  <main class="shell" :data-state="state">
    <nav class="nav">
      <a href="/">Czat</a>
      <a href="/admin">Admin</a>
      <span class="muted">{{ state }}</span>
    </nav>
    <section class="panel" v-if="lines.length === 0">
      <p class="muted">Napisz tak, jakbyś mówił. Jarvis ma tu te same narzędzia co w głosie.</p>
    </section>
    <section class="panel" v-for="(line, index) in lines" :key="index">
      <p class="muted">{{ line.role }}</p>
      <p>{{ line.text }}</p>
    </section>
    <section class="panel" v-if="widget">
      <p class="muted">{{ widget.title }}</p>
      <p>{{ widget.body }}</p>
      <div v-if="widget.type === 'map'" ref="mapHost" style="height: 280px"></div>
    </section>
    <section class="panel" v-if="card">
      <p>{{ card.summary }}</p>
      <pre class="mono">{{ card.body }}</pre>
      <button class="warn" @click="decide(true)">Tak</button>
      <button class="danger" @click="decide(false)">Nie</button>
    </section>
    <p v-if="error" class="error">{{ error }}</p>
    <form class="panel" @submit.prevent="submit">
      <input v-model="draft" placeholder="Napisz do Jarvisa" />
      <button class="accent" type="submit">Wyślij</button>
    </form>
  </main>
</template>
