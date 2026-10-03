// Caches the app shell so an installed AMF Remote opens even when AMF is
// unreachable (it then shows "Can't reach AMF" instead of a browser error
// page). API routes are never cached — status is live or nothing.
"use strict";

// v6: drops any tunnel error page an earlier worker cached as the shell.
const CACHE = "amf-remote-v6";
// How long a page load waits on AMF before an installed app opens from its
// cached shell instead.
const NETWORK_TIMEOUT_MS = 4000;
const SHELL = ["/", "/app.js", "/app.css", "/manifest.webmanifest", "/icon-192.png", "/icon-512.png"];

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(SHELL)));
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== "GET" || url.origin !== location.origin) return;
  // Navigations may carry ?code=…; serve the shell for any of them.
  const key = event.request.mode === "navigate" ? "/" : url.pathname;
  if (!SHELL.includes(key)) return;

  event.respondWith(networkFirst(event.request, key));
});

// Network first, so a newer AMF build's shell replaces the cached one; the
// cache is only the fallback. A fallback that waited for the network to
// *fail* would rarely come: an AMF whose tailnet peer is offline doesn't
// refuse, it just never answers, and the installed app sat on its splash
// screen until the OS gave up minutes later. So the network gets
// NETWORK_TIMEOUT_MS before a cached shell is served instead — which then
// says "Can't reach AMF" itself. A tunnel's error page (`tailscale serve`
// answers 502 for an AMF that is down) is likewise passed over for the
// cache, and never cached.
async function networkFirst(request, key) {
  const network = fetch(request).then((response) => {
    if (response.ok) {
      const copy = response.clone();
      caches.open(CACHE).then((cache) => cache.put(key, copy));
    }
    return response;
  });
  // Lost the race: a late failure has nobody left to report to.
  network.catch(() => {});

  let answer = null;
  try {
    answer = await Promise.race([
      network,
      new Promise((resolve) => setTimeout(resolve, NETWORK_TIMEOUT_MS, null)),
    ]);
  } catch { /* failed outright: fall back below */ }
  if (answer?.ok) return answer;

  const cached = await caches.match(key);
  if (cached) return cached;
  // Nothing cached yet (first visit): the network is all there is, so keep
  // waiting on it, error page or not.
  return answer ?? network;
}

// Payload from `crate::remote_push::PushMessage`: { title, body, tag }.
self.addEventListener("push", (event) => {
  let message = { title: "AMF", body: "An agent needs attention.", tag: "amf" };
  try {
    message = { ...message, ...event.data.json() };
  } catch { /* keep the generic text */ }
  event.waitUntil(
    self.registration.showNotification(message.title, {
      body: message.body,
      tag: message.tag,
      // A repeat for the same feature replaces the old one, but should
      // still buzz: it's a new state.
      renotify: true,
      icon: "/icon-192.png",
      badge: "/icon-192.png",
      data: { url: message.url || "/" },
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = new URL(event.notification.data?.url || "/", location.origin).href;
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then(async (windows) => {
      const open = windows.find((w) => new URL(w.url).origin === location.origin);
      if (!open) return self.clients.openWindow(url);
      await open.focus();
      // The page routes on its hash; navigating an open client keeps it.
      return open.navigate ? open.navigate(url) : open;
    }),
  );
});
