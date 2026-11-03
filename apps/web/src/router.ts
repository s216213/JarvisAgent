import { createRouter, createWebHistory } from "vue-router";
import Login from "./pages/Login.vue";
import Chat from "./pages/Chat.vue";
import Admin from "./pages/Admin.vue";
import Space from "./pages/Space.vue";
import CommandCenter from "./pages/CommandCenter.vue";

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: "/login", component: Login },
    { path: "/", component: CommandCenter },
    { path: "/chat", component: Chat },
    { path: "/space", component: Space },
    { path: "/admin", component: Admin },
  ],
});
