// recto's service worker. It exists so browsers offer to install recto, and to show a page
// when the server cannot be reached. It never stores pages or anything personal: only the
// offline page, scripts, styles and brand images.
const CACHE = "recto-__VERSION__-__BRAND__";
const OFFLINE = "/offline";

// Keeps the offline page in `language` (else the browser's), replacing the one kept before.
// Without credentials it renders for no user and the System theme, so nothing personal is
// stored; the language then goes as `Accept-Language`, since the locale cookie is not sent.
// Pages vary on that header, so lookups ignore `Vary`.
async function keepOffline(language) {
  const cache = await caches.open(CACHE);
  const kept = await cache.match(OFFLINE, { ignoreVary: true });
  if (language && kept && kept.headers.get("Content-Language") === language) return;
  const headers = language ? { "Accept-Language": language } : {};
  const response = await fetch(new Request(OFFLINE, { credentials: "omit", headers }));
  if (!response.ok) return;
  await cache.delete(OFFLINE, { ignoreVary: true });
  await cache.put(OFFLINE, response);
}

self.addEventListener("install", (event) => {
  event.waitUntil(
    // The stylesheet is kept so the offline page is styled.
    Promise.all([keepOffline(), caches.open(CACHE).then((cache) => cache.add("/pkg/recto.css"))])
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
    event.respondWith(
      fetch(request).then(
        (response) => {
          // Follow the language of the pages, which the person may have changed.
          const language = response.headers.get("Content-Language");
          if (language) event.waitUntil(keepOffline(language).catch(() => {}));
          return response;
        },
        () => caches.match(OFFLINE, { ignoreVary: true }),
      ),
    );
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
