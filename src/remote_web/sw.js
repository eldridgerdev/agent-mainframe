// Caches the app shell so an installed AMF Remote opens even when AMF is
// unreachable (it then shows "Can't reach AMF" instead of a browser error
// page). API routes are never cached — status is live or nothing.
"use strict";

const CACHE = "amf-remote-v4";
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

  // Network first, so a newer AMF build's shell replaces the cached one;
  // the cache is only the offline fallback.
  event.respondWith(
    fetch(event.request)
      .then((response) => {
        const copy = response.clone();
        caches.open(CACHE).then((cache) => cache.put(key, copy));
        return response;
      })
      .catch(() => caches.match(key)),
  );
});

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
