// recto's service worker. It exists so browsers offer to install recto, and to show a page
// when the server cannot be reached. It never stores pages or anything personal: only the
// offline page, scripts, styles and brand images.
const CACHE = "recto-__VERSION__-__BRAND__";
const OFFLINE = "/offline";

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) =>
        // Without credentials the offline page renders for no user and the System theme, so
        // nothing personal is stored. The stylesheet is kept so the page is styled offline.
        Promise.all([
          cache.add(new Request(OFFLINE, { credentials: "omit" })),
          cache.add("/pkg/recto.css"),
        ]),
      )
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) =>
        Promise.all(names.filter((name) => name !== CACHE).map((name) => caches.delete(name))),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (request.method !== "GET") return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  if (request.mode === "navigate") {
    event.respondWith(fetch(request).catch(() => caches.match(OFFLINE)));
    return;
  }

  if (url.pathname.startsWith("/pkg/")) {
    // Network first: a new release must not run old scripts against new pages. The cache is
    // only for when the server cannot be reached.
    event.respondWith(
      fetch(request)
        .then((response) => {
          if (response.ok) {
            const copy = response.clone();
            event.waitUntil(caches.open(CACHE).then((cache) => cache.put(request, copy)));
          }
          return response;
        })
        .catch(() => caches.match(request).then((cached) => cached || Response.error())),
    );
    return;
  }

  if (url.pathname.startsWith("/brand/")) {
    // Stale while revalidate: a brand image rarely changes and the next visit shows the new one.
    event.respondWith(
      caches.open(CACHE).then((cache) =>
        cache.match(request).then((cached) => {
          const fresh = fetch(request).then((response) => {
            if (response.ok) cache.put(request, response.clone());
            return response;
          });
          if (cached) {
            event.waitUntil(fresh.catch(() => {}));
            return cached;
          }
          return fresh;
        }),
      ),
    );
  }
  // Anything else is left to the browser, as if no worker existed.
});
