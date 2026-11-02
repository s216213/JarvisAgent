export const apiBase = import.meta.env.VITE_API_URL ?? "http://localhost:8080";

export async function login(password: string): Promise<boolean> {
  const response = await fetch(`${apiBase}/api/login`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password }),
  });
  return response.ok;
}

export function socket(deviceId: string): WebSocket {
  const url = new URL("/api/ws", apiBase);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.searchParams.set("device_id", deviceId);
  return new WebSocket(url);
}
