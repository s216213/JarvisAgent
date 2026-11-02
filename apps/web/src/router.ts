import { createRouter, createWebHistory } from "vue-router";
import Login from "./pages/Login.vue";
import Chat from "./pages/Chat.vue";
import Admin from "./pages/Admin.vue";

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: "/login", component: Login },
    { path: "/", component: Chat },
    { path: "/admin", component: Admin },
  ],
});
