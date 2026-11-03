const configured = import.meta.env.VITE_API_URL?.trim().replace(/\/$/, "");
export const apiBase = configured ? configured : "";

function socketUrl(path: string): URL {
  const base = apiBase || window.location.origin;
  const url = new URL(path, base);
  url.protocol = base.startsWith("https:") ? "wss:" : "ws:";
  return url;
}

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
  const url = socketUrl("/api/ws");
  url.searchParams.set("device_id", deviceId);
  withSession(url);
  return new WebSocket(url);
}

export function terminalSocket(): WebSocket {
  const url = socketUrl("/api/term");
  withSession(url);
  return new WebSocket(url);
}
