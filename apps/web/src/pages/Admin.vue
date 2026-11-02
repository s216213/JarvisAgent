<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { apiBase } from "../api";

type Flag = { name: string; enabled: boolean };
type EnvVar = { name: string; secret: boolean; preview: string | null };

const router = useRouter();
const flags = ref<Flag[]>([]);
const vars = ref<EnvVar[]>([]);
const status = ref("");
const host = ref("");
const term = ref("");
const command = ref("");
let termSocket: WebSocket | null = null;
const name = ref("");
const value = ref("");
const error = ref("");

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
  const statusJson = await statusRes.json();
  status.value = JSON.stringify(statusJson, null, 2);
  const hostRes = await fetch(`${apiBase}/api/host`, { credentials: "include" });
  host.value = JSON.stringify(await hostRes.json(), null, 2);
}

function openTerm() {
  const url = new URL("/api/term", apiBase);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  termSocket = new WebSocket(url);
  termSocket.onmessage = (message) => {
    term.value += String(message.data);
  };
}

function sendTerm() {
  termSocket?.send(command.value + "\n");
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
  host.value = await response.text();
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
    <nav class="nav">
      <a href="/">Czat</a>
      <a href="/admin">Admin</a>
    </nav>
    <section class="panel">
      <h2>Flagi</h2>
      <p v-for="flag in flags" :key="flag.name">
        <button @click="toggle(flag)">{{ flag.enabled ? "włączona" : "wyłączona" }}</button>
        {{ flag.name }}
      </p>
    </section>
    <section class="panel">
      <h2>Zmienne</h2>
      <p class="muted" v-for="item in vars" :key="item.name">{{ item.name }}: {{ item.secret ? "ustawione" : item.preview }}</p>
      <input v-model="name" placeholder="NAZWA" />
      <input v-model="value" placeholder="wartość, sekret nie wraca" />
      <button class="accent" @click="saveEnv(false)">Zapisz</button>
      <button class="danger" @click="saveEnv(true)">Usuń</button>
    </section>
    <section class="panel">
      <h2>Status</h2>
      <p class="muted">Adres nasłuchu zmienia się dopiero po restarcie kontenera. Wejście tutaj nie przenosi rozmowy.</p>
      <pre class="mono">{{ status }}</pre>
    </section>
    <section class="panel">
      <h2>Maszyna</h2>
      <pre class="mono">{{ host }}</pre>
      <button @click="restart('searxng')">Restart SearXNG</button>
      <button @click="desktop">Pulpit, gdy starczy RAM</button>
    </section>
    <section class="panel">
      <h2>Terminal</h2>
      <button @click="openTerm">Otwórz PTY</button>
      <pre class="mono">{{ term }}</pre>
      <input v-model="command" placeholder="polecenie" @keydown.enter.prevent="sendTerm" />
    </section>
    <p v-if="error" class="error">{{ error }}</p>
  </main>
</template>
