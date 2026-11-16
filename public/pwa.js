// Registers the service worker that makes recto installable and shows an offline page.
if ("serviceWorker" in navigator) navigator.serviceWorker.register("/sw.js");
