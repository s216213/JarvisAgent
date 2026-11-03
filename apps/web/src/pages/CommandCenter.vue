<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import Holo from "../components/Holo.vue";
import { apiBase, authed, socket } from "../api";
import type { HoloScene } from "../holo";

type Line = { role: "ja" | "jarvis"; text: string };
type Card = { id: string; summary: string; body: string };
type AgentCard = { id: string; label: string; detail: string; status: string };
type Job = { id: string; kind: string; status: string; detail: string; pinned?: boolean };
type Schedule = { name: string; at: string; instruction: string; last_day?: string };
type Provider = { name: string; set: boolean };
type Device = { id: string; app?: string; window?: string; telemetry?: { cpu?: number; battery?: number; ram?: number } };
type Board = {
  scene?: HoloScene;
  agents?: AgentCard[];
  jobs?: Job[];
  schedules?: Schedule[];
  evolution?: { created: number; kind: string; status: string; summary: string }[];
  providers?: Provider[];
  recent_429?: string[];
  host?: { free_mb?: number };
  devices?: Device[];
  feed?: { kind: string; text: string }[];
  memory?: { chunks: number; facts: number; edges: number; messages: number; lexical?: boolean };
  presence?: string | null;
  proposals?: { id?: string; summary?: string; status?: string }[];
  plugins?: { id: string; enabled: boolean; description: string }[];
};

const nav = [
  ["core", "Command Center"],
  ["agents", "AI Core"],
  ["tasks", "Agents"],
  ["tasks", "Tasks"],
  ["timeline", "Calendar"],
  ["memory", "Memory"],
  ["talk", "Conversations"],
  ["memory", "Knowledge"],
  ["tools", "Tools"],
  ["timeline", "Workflows"],
] as const;

const router = useRouter();
const board = ref<Board | null>(null);
const query = ref("");
const draft = ref("");
const lines = ref<Line[]>([]);
const card = ref<Card | null>(null);
const error = ref("");
const clock = ref("");
const workflow = ref("");
const transcript = ref<HTMLElement | null>(null);
let ws: WebSocket | null = null;
let buffer = "";
let timer = 0;

const needle = computed(() => query.value.trim().toLowerCase());
function hit(text: string) {
  return !needle.value || text.toLowerCase().includes(needle.value);
}

const agents = computed(() => (board.value?.agents ?? []).filter((item) => hit(`${item.label} ${item.detail} ${item.status}`)));
const jobs = computed(() => (board.value?.jobs ?? []).filter((item) => hit(`${item.kind} ${item.status} ${item.detail}`)));
const feed = computed(() => (board.value?.feed ?? []).filter((item) => hit(item.text)));
const schedules = computed(() => (board.value?.schedules ?? []).filter((item) => hit(`${item.name} ${item.at} ${item.instruction}`)));
const plugins = computed(() => (board.value?.plugins ?? []).filter((item) => hit(`${item.id} ${item.description}`)));

const desktop = computed(() => board.value?.devices?.find((item) => item.id.includes("desktop")) ?? board.value?.devices?.[0]);
const cpu = computed(() => {
  const value = desktop.value?.telemetry?.cpu;
  return typeof value === "number" && !Number.isNaN(value) ? Math.max(0, Math.min(100, value)) : null;
});
const ram = computed(() => {
  const free = board.value?.host?.free_mb;
  if (typeof free !== "number") return null;
  return Math.max(0, Math.min(100, Math.round((free / 4096) * 100)));
});

function tick() {
  clock.value = new Intl.DateTimeFormat("pl-PL", {
    weekday: "long",
    day: "numeric",
    month: "long",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date());
}

function providerState(item: Provider) {
  const paused = (board.value?.recent_429 ?? []).some((line) => line.toLowerCase().includes(item.name));
  if (paused) return "przerwa 429";
  return item.set ? "klucz jest" : "brak klucza";
}

function go(id: string) {
  document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" });
}

function applyBoard(next: Board) {
  board.value = next;
  if (!workflow.value && next.schedules?.length) workflow.value = next.schedules[0].name;
}

async function load() {
  const response = await fetch(`${apiBase}/api/board`, authed());
  if (response.status === 401) {
    router.push("/login");
    return;
  }
  if (!response.ok) {
    error.value = "Nie mam pulpitu.";
    return;
  }
  applyBoard((await response.json()) as Board);
}

function send(payload: unknown) {
  if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(payload));
}

function stick() {
  nextTick(() => {
    const pane = transcript.value;
    if (pane) pane.scrollTop = pane.scrollHeight;
  });
}

function onSocket(raw: string) {
  const event = JSON.parse(raw);
  if (event.type === "board") applyBoard(event as Board);
  if (event.type === "token") {
    buffer += `${event.text} `;
    const last = lines.value.at(-1);
    if (last?.role === "jarvis") last.text = buffer.trim();
    else lines.value.push({ role: "jarvis", text: buffer.trim() });
    stick();
  }
  if (event.type === "done") buffer = "";
  if (event.type === "confirm_request") {
    card.value = { id: event.id, summary: event.summary, body: event.body };
  }
}

function submit() {
  const text = draft.value.trim();
  if (!text) return;
  lines.value.push({ role: "ja", text });
  draft.value = "";
  stick();
  const word = text.toLowerCase();
  if (card.value && (word === "tak" || word === "nie")) {
    send({ type: "confirm", id: card.value.id, accept: word === "tak", via: "text" });
    card.value = null;
    return;
  }
  send({ type: "user_text", text });
}

function decide(accept: boolean) {
  if (!card.value) return;
  send({ type: "confirm", id: card.value.id, accept, via: "button" });
  card.value = null;
}

function runWorkflow() {
  const item = schedules.value.find((row) => row.name === workflow.value) ?? schedules.value[0];
  if (!item?.instruction) return;
  draft.value = "";
  lines.value.push({ role: "ja", text: item.instruction });
  send({ type: "user_text", text: item.instruction });
  go("talk");
}

onMounted(() => {
  if (!sessionStorage.getItem("jarvis-auth") || !sessionStorage.getItem("jarvis-session")) {
    router.push("/login");
    return;
  }
  tick();
  timer = window.setInterval(tick, 1000);
  load().catch(() => {
    error.value = "Rdzeń nie odpowiada.";
  });
  ws = socket("web-chat");
  ws.onmessage = (message) => onSocket(String(message.data));
  ws.onopen = () => {
    navigator.geolocation?.getCurrentPosition(
      (pos) => send({ type: "presence", speech: false, idle: false, away: false, telemetry: { lat: pos.coords.latitude, lon: pos.coords.longitude } }),
      () => send({ type: "presence", speech: false, idle: false, away: false }),
    );
  };
});

onBeforeUnmount(() => {
  window.clearInterval(timer);
  ws?.close();
});
</script>

<template>
  <main class="cc">
    <aside class="cc-side">
      <p class="mark">JARVIS</p>
      <p class="muted">Command Center</p>
      <button v-for="(item, index) in nav" :key="index" type="button" @click="go(item[0])">{{ item[1] }}</button>
      <a href="/chat">Czat</a>
      <a href="/space">Przestrzeń</a>
      <a href="/admin">Admin</a>
    </aside>
    <section class="cc-main">
      <header class="cc-top">
        <div>
          <p class="muted">{{ clock }}</p>
          <p class="mono">{{ board?.presence ? `obecność ${board.presence}` : "brak obecności" }}</p>
        </div>
        <input v-model="query" placeholder="Szukaj na pulpicie" />
      </header>
      <p v-if="error" class="error">{{ error }}</p>
      <div class="cc-grid">
        <section id="core" class="frame cc-core">
          <h2>AI Core</h2>
          <Holo :scene="board?.scene" />
          <p class="muted">{{ board?.scene?.text || "Czekam na mapę siebie." }}</p>
        </section>
        <section id="feed" class="frame">
          <h2>Live feed</h2>
          <p v-if="feed.length === 0" class="muted">Pusto. Audit i odpowiedzi pojawią się tutaj.</p>
          <ul class="cc-list">
            <li v-for="(item, index) in feed" :key="index"><span class="mono">{{ item.kind }}</span> {{ item.text }}</li>
          </ul>
        </section>
        <section id="agents" class="frame">
          <h2>Agents</h2>
          <p v-if="agents.length === 0" class="muted">Brak kart.</p>
          <div class="cc-cards">
            <article v-for="item in agents" :key="item.id">
              <p>{{ item.label }}</p>
              <p class="mono">{{ item.status }}</p>
              <p class="muted">{{ item.detail }}</p>
            </article>
          </div>
        </section>
        <section id="monitor" class="frame">
          <h2>System</h2>
          <p class="mono">CPU {{ cpu == null ? "—" : `${Math.round(cpu)}%` }}</p>
          <div class="ring"><span :style="{ width: `${cpu ?? 0}%` }"></span></div>
          <p class="mono">RAM wolne {{ board?.host?.free_mb ?? "—" }} MB</p>
          <div class="ring"><span :style="{ width: `${ram ?? 0}%` }"></span></div>
          <p class="muted">{{ desktop?.window || desktop?.app || "Brak telemetrii komputera." }}</p>
        </section>
        <section id="memory" class="frame">
          <h2>Memory</h2>
          <p v-if="!board?.memory || (board.memory.facts === 0 && board.memory.chunks === 0)" class="muted">Zero notatek i faktów.</p>
          <p class="mono">fakty {{ board?.memory?.facts ?? 0 }} · fragmenty {{ board?.memory?.chunks ?? 0 }}</p>
          <p class="mono">krawędzie {{ board?.memory?.edges ?? 0 }} · wiadomości {{ board?.memory?.messages ?? 0 }}</p>
          <p class="muted">Pamięć leksykalna, bez modelu embeddingowego.</p>
        </section>
        <section id="timeline" class="frame">
          <h2>Timeline</h2>
          <p v-if="schedules.length === 0 && !(board?.evolution?.length)" class="muted">Brak harmonogramu i historii zmian.</p>
          <ul class="cc-list">
            <li v-for="item in schedules" :key="item.name"><span class="mono">{{ item.at }}</span> {{ item.name }}</li>
            <li v-for="item in board?.evolution ?? []" :key="item.created"><span class="mono">{{ item.status }}</span> {{ item.summary }}</li>
          </ul>
          <div class="row" v-if="schedules.length">
            <select v-model="workflow">
              <option v-for="item in schedules" :key="item.name" :value="item.name">{{ item.name }}</option>
            </select>
            <button type="button" @click="runWorkflow">Run Workflow</button>
          </div>
        </section>
        <section id="tools" class="frame">
          <h2>Tools</h2>
          <p v-if="plugins.length === 0" class="muted">Brak pluginów w katalogu.</p>
          <ul class="cc-list">
            <li v-for="item in plugins" :key="item.id"><span class="mono">{{ item.enabled ? "włączony" : "wyłączony" }}</span> {{ item.id }}</li>
          </ul>
          <p v-if="board?.proposals?.length" class="muted">Propozycje: {{ board.proposals.length }}. Zastosowanie zostaje w adminie.</p>
        </section>
        <section id="llm" class="frame cc-wide">
          <h2>LLM</h2>
          <div class="cc-cards">
            <article v-for="item in board?.providers ?? []" :key="item.name">
              <p>{{ item.name }}</p>
              <p class="mono">{{ providerState(item) }}</p>
            </article>
          </div>
        </section>
        <section id="tasks" class="frame cc-wide">
          <h2>Tasks</h2>
          <p v-if="jobs.length === 0" class="muted">Zero zadań.</p>
          <ul class="cc-list">
            <li v-for="item in jobs" :key="item.id"><span class="mono">{{ item.status }}</span> {{ item.kind }} <span class="muted">{{ item.detail }}</span></li>
          </ul>
        </section>
      </div>
      <footer id="talk" class="cc-talk">
        <div class="frame cc-lines" ref="transcript">
          <p v-if="lines.length === 0" class="muted">Talk to Jarvis</p>
          <p v-for="(line, index) in lines" :key="index" :class="line.role === 'jarvis' ? 'line-jarvis' : ''">{{ line.text }}</p>
        </div>
        <section class="frame consent" v-if="card">
          <p>{{ card.summary }}</p>
          <pre class="mono">{{ card.body }}</pre>
          <div class="row">
            <button class="warn" type="button" @click="decide(true)">Tak</button>
            <button class="danger" type="button" @click="decide(false)">Nie</button>
          </div>
        </section>
        <form class="composer" @submit.prevent="submit">
          <button type="button" @click="go('talk')">Start New Task</button>
          <button type="button" @click="go('timeline')">Open Calendar</button>
          <input v-model="draft" placeholder="Talk to Jarvis" />
          <button class="accent" type="submit">Wyślij</button>
        </form>
      </footer>
    </section>
  </main>
</template>
