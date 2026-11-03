export const apiBase = import.meta.env.VITE_API_URL ?? "http://localhost:8080";

export async function login(password: string): Promise<boolean> {
  const response = await fetch(`${apiBase}/api/login`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password }),
  });
  if (!response.ok) return false;
  const body = (await response.json().catch(() => ({}))) as { session?: string };
  if (body.session) sessionStorage.setItem("jarvis-session", body.session);
  return true;
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
