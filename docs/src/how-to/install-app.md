# Install recto as an app

recto can be installed on a phone, tablet or computer. It then opens in its own window, with its
own icon, under the name and colors you set in [Brand recto](brand.md).

Browsers install apps only from pages served over HTTPS, or from `localhost`. Put recto behind
a reverse proxy with a certificate first (see [Run behind a reverse proxy](reverse-proxy.md)).

## Chrome and Edge, on a computer or Android

1. Open recto and sign in.
2. Click the install button in the address bar, or open the browser menu and choose
   **Install recto** (Edge: **Apps**, then **Install this site as an app**). The name is the one
   you configured.
3. Confirm. recto opens in its own window.

## Safari on iPhone and iPad

1. Open recto in Safari.
2. Tap **Share**, then **Add to Home Screen**.
3. Tap **Add**.

## Safari on macOS

1. Open recto in Safari.
2. Choose **File**, then **Add to Dock**.

## What works offline

Only a page saying you are offline, with a link to try again. Printing and scanning need your
server, so recto shows no pages from memory. Behind a reverse proxy, when recto itself is down,
the proxy's error page shows rather than the offline page.

## Updates

There is nothing to update by hand. The app checks for a new version each time it loads, and a
new version of recto takes over on the next load.
