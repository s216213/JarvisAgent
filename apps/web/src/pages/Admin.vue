<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { apiBase, authed, terminalSocket } from "../api";

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
type PluginRow = { id: string; description: string; capabilities: string[]; trust: string; enabled: boolean };
type ProposalRow = { id: string; kind: string; summary: string; status: string; branch?: string };
const plugins = ref<PluginRow[]>([]);
const proposals = ref<ProposalRow[]>([]);
const proposalNote = ref("");

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
    fetch(`${apiBase}/api/flags`, authed()),
    fetch(`${apiBase}/api/env`, authed()),
    fetch(`${apiBase}/api/status`, authed()),
  ]);
  if (flagRes.status === 401) {
    router.push("/login");
    return;
  }
  const flagJson = await flagRes.json();
  flags.value = flagJson.flags ?? [];
  vars.value = (await envRes.json()).vars ?? [];
  readStatus(await statusRes.json());
  const hostRes = await fetch(`${apiBase}/api/host`, authed());
  readHost(await hostRes.json());
  const pluginRes = await fetch(`${apiBase}/api/plugins`, authed());
  plugins.value = (await pluginRes.json()).plugins ?? [];
  const proposalRes = await fetch(`${apiBase}/api/proposals`, authed());
  proposals.value = (await proposalRes.json()).proposals ?? [];
}

async function pluginAction(id: string, body: Record<string, unknown>) {
  await fetch(
    `${apiBase}/api/plugins`,
    authed({
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ id, ...body }),
    }),
  );
  await load();
}

async function proposalAction(id: string, action: string, target?: string) {
  proposalNote.value = "";
  const response = await fetch(
    `${apiBase}/api/proposals`,
    authed({
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ id, action, target }),
    }),
  );
  const text = await response.text();
  try {
    proposalNote.value = JSON.parse(text).text ?? text;
  } catch {
    proposalNote.value = text;
  }
  await load();
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
  await fetch(
    `${apiBase}/api/host`,
    authed({
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ service }),
    }),
  );
  await load();
}

async function desktop() {
  const response = await fetch(`${apiBase}/api/desktop`, authed({ method: "POST" }));
  const text = await response.text();
  try {
    readHost(JSON.parse(text));
  } catch {
    host.value = text;
  }
}

async function toggle(flag: Flag) {
  flag.enabled = !flag.enabled;
  await fetch(
    `${apiBase}/api/flags`,
    authed({
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: flag.name, enabled: flag.enabled }),
    }),
  );
}

async function saveEnv(remove = false) {
  error.value = "";
  const response = await fetch(
    `${apiBase}/api/env`,
    authed({
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: name.value, value: value.value, remove }),
    }),
  );
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
  if (!sessionStorage.getItem("jarvis-session")) {
    sessionStorage.removeItem("jarvis-auth");
    router.push("/login?again=1");
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
        <a href="/">Pulpit</a>
        <a href="/chat">Czat</a>
        <a href="/space">Przestrzeń</a>
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
        <h2>Pluginy</h2>
        <p v-if="plugins.length === 0" class="muted">Brak pluginów w katalogu na OCI.</p>
        <article class="flag" v-for="plugin in plugins" :key="plugin.id">
          <div class="row">
            <button :class="{ on: plugin.enabled }" @click="pluginAction(plugin.id, { enabled: !plugin.enabled })">{{ plugin.enabled ? "włączony" : "wyłączony" }}</button>
            <button class="danger" @click="pluginAction(plugin.id, { remove: true })">Usuń</button>
          </div>
          <div>
            <p class="mono">{{ plugin.id }}</p>
            <p class="muted">{{ plugin.description }} · {{ plugin.capabilities.join(", ") }} · {{ plugin.trust }}</p>
          </div>
        </article>
      </section>
      <section class="frame wide">
        <h2>Propozycje</h2>
        <p v-if="proposals.length === 0" class="muted">Brak propozycji kodu.</p>
        <article class="flag" v-for="item in proposals" :key="item.id">
          <div>
            <p class="mono">{{ item.id }} · {{ item.status }}</p>
            <p class="muted">{{ item.summary }}</p>
            <div class="row">
              <button @click="proposalAction(item.id, 'apply', 'oci')">OCI</button>
              <button @click="proposalAction(item.id, 'apply', 'pc')">Komputer</button>
              <button @click="proposalAction(item.id, 'apply', 'both')">Oba</button>
              <button @click="proposalAction(item.id, 'build')">Zbuduj</button>
              <button @click="proposalAction(item.id, 'push')">Wypchnij</button>
            </div>
          </div>
        </article>
        <p v-if="proposalNote" class="muted">{{ proposalNote }}</p>
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
