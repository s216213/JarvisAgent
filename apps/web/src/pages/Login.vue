<script setup lang="ts">
import { ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { login } from "../api";

const password = ref("");
const error = ref("");
const router = useRouter();
const route = useRoute();

async function submit() {
  error.value = "";
  const result = await login(password.value).catch(() => "bad" as const);
  if (result === "bad") {
    error.value = "Hasło nie pasuje.";
    return;
  }
  if (result === "empty") {
    error.value = "Hasło przeszło, ale rdzeń nie oddał sesji czatu.";
    return;
  }
  sessionStorage.setItem("jarvis-auth", "1");
  router.push("/");
}
</script>

<template>
  <main class="shell gate">
    <section class="frame">
      <form class="stack" @submit.prevent="submit">
        <h1>Jarvis</h1>
        <p class="muted">Jedno hasło. Nic więcej.</p>
        <input v-model="password" type="password" autocomplete="current-password" placeholder="Hasło" />
        <p v-if="route.query.again === '1'" class="muted">Poprzednia sesja nie doszła do czatu. Wpisz hasło jeszcze raz.</p>
        <p v-if="error" class="error">{{ error }}</p>
        <button class="accent" type="submit">Wejdź</button>
      </form>
    </section>
  </main>
</template>
