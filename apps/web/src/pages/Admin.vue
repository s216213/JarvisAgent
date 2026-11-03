<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { apiBase, terminalSocket } from "../api";

type Flag = { name: string; enabled: boolean; description?: string };
type EnvVar = { name: string; secret: boolean; preview: string | null };
type Metrics = {
  presence: string;
  devices: string[];
  freeMb: number | null;
  desktop: boolean | null;
  turso: boolean | null;
  recentCount: number | null;
  extra: string;
};

const router = useRouter();
const flags = ref<Flag[]>([]);
const vars = ref<EnvVar[]>([]);
const metrics = ref<Metrics>({
  presence: "brak",
  devices: [],
  freeMb: null,
  desktop: null,
  turso: null,
  recentCount: null,
  extra: "",
});
const host = ref("");
const hostFree = ref<number | null>(null);
const hostDesktop = ref<boolean | null>(null);
const term = ref("");
const command = ref("");
let termSocket: WebSocket | null = null;
const name = ref("");
const value = ref("");
const error = ref("");

function asRecord(value: unknown): Record<string, unknown> | null {
  if (value && typeof value === "object" && !Array.isArray(value)) return value as Record<string, unknown>;
  return null;
}

function yn(value: boolean | null) {
  if (value == null) return "—";
  return value ? "tak" : "nie";
}

function readStatus(value: unknown) {
  const row = asRecord(value);
  const hostRow = asRecord(row?.host);
  const devices = Array.isArray(row?.devices) ? row.devices.filter((item): item is string => typeof item === "string") : [];
  const recent = row?.recent_429;
  const recentCount = typeof recent === "number" ? recent : Array.isArray(recent) ? recent.length : null;
  metrics.value = {
    presence: typeof row?.presence === "string" && row.presence ? row.presence : "brak",
    devices,
    freeMb: typeof hostRow?.free_mb === "number" ? hostRow.free_mb : null,
    desktop: typeof hostRow?.desktop === "boolean" ? hostRow.desktop : null,
    turso: typeof row?.turso === "boolean" ? row.turso : null,
    recentCount,
    extra: JSON.stringify({ providers: row?.providers ?? null, audit: row?.audit ?? null, recent_429: recent ?? null }, null, 2),
  };
}

function readHost(value: unknown) {
  const row = asRecord(value);
  const resources = asRecord(row?.resources);
  hostFree.value = typeof resources?.free_mb === "number" ? resources.free_mb : null;
  hostDesktop.value = typeof resources?.desktop === "boolean" ? resources.desktop : null;
  if (typeof row?.ps === "string") host.value = row.ps;
  else host.value = typeof value === "string" ? value : JSON.stringify(value, null, 2);
}

async function load() {
  const [flagRes, envRes, statusRes] = await Promise.all([
    fetch(`${apiBase}/api/flags`, { credentials: "include" }),
    fetch(`${apiBase}/api/env`, { credentials: "include" }),
    fetch(`${apiBase}/api/status`, { credentials: "include" }),
  ]);
  if (flagRes.status === 401) {
    router.push("/login");
    return;
  }
  const flagJson = await flagRes.json();
  flags.value = flagJson.flags ?? [];
  vars.value = (await envRes.json()).vars ?? [];
  readStatus(await statusRes.json());
  const hostRes = await fetch(`${apiBase}/api/host`, { credentials: "include" });
  readHost(await hostRes.json());
}

function openTerm() {
  term.value = "";
  termSocket = terminalSocket();
  termSocket.onmessage = (message) => {
    term.value += String(message.data);
  };
  termSocket.onerror = () => {
    term.value += "Połączenie terminala nie wstało. Wyjdź i zaloguj się jeszcze raz.\n";
  };
}

function sendTerm() {
  if (termSocket?.readyState !== WebSocket.OPEN) return;
  termSocket.send(command.value + "\n");
  command.value = "";
}

async function restart(service: string) {
  await fetch(`${apiBase}/api/host`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ service }),
  });
  await load();
}

async function desktop() {
  const response = await fetch(`${apiBase}/api/desktop`, { method: "POST", credentials: "include" });
  const text = await response.text();
  try {
    readHost(JSON.parse(text));
  } catch {
    host.value = text;
  }
}

async function toggle(flag: Flag) {
  flag.enabled = !flag.enabled;
  await fetch(`${apiBase}/api/flags`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name: flag.name, enabled: flag.enabled }),
  });
}

async function saveEnv(remove = false) {
  error.value = "";
  const response = await fetch(`${apiBase}/api/env`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name: name.value, value: value.value, remove }),
  });
  if (!response.ok) {
    error.value = "Nie zapisuję tej zmiennej.";
    return;
  }
  value.value = "";
  await load();
}

onMounted(() => {
  if (!sessionStorage.getItem("jarvis-auth")) {
    router.push("/login");
    return;
  }
  load().catch(() => {
    error.value = "Rdzeń nie odpowiada.";
  });
});
</script>

<template>
  <main class="shell">
    <header class="topbar">
      <span class="mark">Jarvis</span>
      <nav class="nav">
        <a href="/">Czat</a>
        <a href="/admin" aria-current="page">Admin</a>
      </nav>
    </header>
    <div class="state-line"></div>
    <div class="board">
      <section class="frame">
        <h2>Status</h2>
        <p class="muted">Adres nasłuchu zmienia się dopiero po restarcie kontenera. Wejście tutaj nie przenosi rozmowy.</p>
        <div class="metrics">
          <div class="metric">
            <p class="muted">obecność</p>
            <p class="mono">{{ metrics.presence }}</p>
          </div>
          <div class="metric">
            <p class="muted">urządzenia</p>
            <p class="mono">{{ metrics.devices.length }}</p>
            <p class="muted">{{ metrics.devices.join(", ") || "brak" }}</p>
          </div>
          <div class="metric">
            <p class="muted">wolny RAM</p>
            <p class="mono">{{ metrics.freeMb == null ? "—" : metrics.freeMb + " MB" }}</p>
          </div>
          <div class="metric">
            <p class="muted">pulpit</p>
            <p class="mono">{{ yn(metrics.desktop) }}</p>
          </div>
          <div class="metric">
            <p class="muted">Turso</p>
            <p class="mono">{{ yn(metrics.turso) }}</p>
          </div>
          <div class="metric">
            <p class="muted">ostatnie 429</p>
            <p class="mono">{{ metrics.recentCount == null ? "—" : metrics.recentCount }}</p>
          </div>
        </div>
        <pre class="mono well">{{ metrics.extra }}</pre>
      </section>
      <section class="frame">
        <h2>Flagi</h2>
        <article class="flag" v-for="flag in flags" :key="flag.name">
          <button :class="{ on: flag.enabled }" @click="toggle(flag)">{{ flag.enabled ? "włączona" : "wyłączona" }}</button>
          <div>
            <p class="mono">{{ flag.name }}</p>
            <p class="muted">{{ flag.description || "Brak opisu." }}</p>
          </div>
        </article>
      </section>
      <section class="frame">
        <h2>Zmienne</h2>
        <p class="muted" v-for="item in vars" :key="item.name">{{ item.name }}: {{ item.secret ? "ustawione" : item.preview }}</p>
        <div class="stack">
          <input v-model="name" placeholder="NAZWA" />
          <input v-model="value" placeholder="wartość, sekret nie wraca" />
          <div class="row">
            <button class="accent" @click="saveEnv(false)">Zapisz</button>
            <button class="danger" @click="saveEnv(true)">Usuń</button>
          </div>
        </div>
      </section>
      <section class="frame">
        <h2>Maszyna</h2>
        <div class="metrics">
          <div class="metric">
            <p class="muted">wolny RAM</p>
            <p class="mono">{{ hostFree == null ? "—" : hostFree + " MB" }}</p>
          </div>
          <div class="metric">
            <p class="muted">pulpit</p>
            <p class="mono">{{ yn(hostDesktop) }}</p>
          </div>
        </div>
        <pre class="mono well">{{ host }}</pre>
        <div class="row">
          <button @click="restart('searxng')">Restart SearXNG</button>
          <button @click="desktop">Pulpit, gdy starczy RAM</button>
        </div>
      </section>
      <section class="frame wide">
        <h2>Terminal</h2>
        <div class="stack">
          <button @click="openTerm">Otwórz PTY</button>
          <pre class="mono well">{{ term }}</pre>
          <input v-model="command" placeholder="polecenie" @keydown.enter.prevent="sendTerm" />
        </div>
      </section>
    </div>
    <p v-if="error" class="error dock">{{ error }}</p>
  </main>
</template>
