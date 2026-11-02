<script setup lang="ts">
import { ref } from "vue";
import { useRouter } from "vue-router";
import { login } from "../api";

const password = ref("");
const error = ref("");
const router = useRouter();

async function submit() {
  error.value = "";
  const ok = await login(password.value).catch(() => false);
  if (!ok) {
    error.value = "Hasło nie pasuje.";
    return;
  }
  sessionStorage.setItem("jarvis-auth", "1");
  router.push("/");
}
</script>

<template>
  <main class="shell">
    <section class="panel">
      <h1>Jarvis</h1>
      <p class="muted">Jedno hasło. Nic więcej.</p>
      <form @submit.prevent="submit">
        <input v-model="password" type="password" autocomplete="current-password" placeholder="Hasło" />
        <p v-if="error" class="error">{{ error }}</p>
        <button class="accent" type="submit">Wejdź</button>
      </form>
    </section>
  </main>
</template>
