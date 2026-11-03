export const apiBase = import.meta.env.VITE_API_URL ?? "http://localhost:8080";

export type LoginResult = "ok" | "bad" | "empty";

export async function login(password: string): Promise<LoginResult> {
  const response = await fetch(`${apiBase}/api/login`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password }),
  });
  if (!response.ok) return "bad";
  const body = (await response.json().catch(() => ({}))) as { session?: string };
  if (!body.session) return "empty";
  sessionStorage.setItem("jarvis-session", body.session);
  return "ok";
}

export function authed(init: RequestInit = {}): RequestInit {
  const headers = new Headers(init.headers);
  const session = sessionStorage.getItem("jarvis-session");
  if (session) headers.set("x-jarvis-session", session);
  return { ...init, credentials: "include", headers };
}

function withSession(url: URL) {
  const session = sessionStorage.getItem("jarvis-session");
  if (session) url.searchParams.set("session", session);
}

export function socket(deviceId: string): WebSocket {
  const url = new URL("/api/ws", apiBase);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.searchParams.set("device_id", deviceId);
  withSession(url);
  return new WebSocket(url);
}

export function terminalSocket(): WebSocket {
  const url = new URL("/api/term", apiBase);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  withSession(url);
  return new WebSocket(url);
}
