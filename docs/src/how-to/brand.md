# Brand recto

recto can carry your name, your color, your logo and your icon, for example to look like part of
a family or company portal. The header, the page titles, the messages that name the app and the
app installed on a phone or a computer all follow.

Every setting is optional. Without them recto looks as it always has.

## 1. Choose a name and a color

Add the settings to `.env`, next to `compose.yml`:

```sh
RECTO_BRAND_NAME="Maison Dupont"
RECTO_BRAND_SHORT_NAME=Maison
RECTO_BRAND_COLOR="#8a2be2"
```

The quotes keep the space and the `#`, which would otherwise start a comment.

Then pass them to the `recto` service in `compose.yml`, under `environment`:

```yaml
      RECTO_BRAND_NAME: ${RECTO_BRAND_NAME:-recto}
      RECTO_BRAND_SHORT_NAME: ${RECTO_BRAND_SHORT_NAME:-}
      RECTO_BRAND_COLOR: "${RECTO_BRAND_COLOR:-#0f6cbd}"
```

- The name, up to 64 characters, shows in the header, the browser tab and the installed app.
- The short name, up to 32 characters, is the label under the icon on a home screen. It defaults
  to the name.
- The color is written `#rrggbb`. Buttons, links, the current page and the focus ring take it, in
  the light and the dark theme. In the dark theme, links and the current page use a lighter shade
  so they stay readable on dark surfaces.

## 2. Add a logo and an icon

Put the files in a folder next to `compose.yml`, such as `./brand`, mount it read-only, and point
the settings at the files inside the container:

```yaml
    environment:
      RECTO_BRAND_LOGO: /brand/logo.svg
      RECTO_BRAND_ICON: /brand/icon.png
    volumes:
      - ./brand:/brand:ro
```

The files must be readable by user 65532, which recto runs as: `chmod a+r brand/*` is enough.

- The **logo** replaces the printer glyph before the name, in the header and on the sign-in page.
  It is shown 28 pixels high. Use a PNG, a JPEG or an SVG of at most 1 MiB; a wide logo is fine.
- The **icon** is the app's icon on a home screen, in the task bar and in the browser tab. Use a
  square PNG or JPEG of at least 512 by 512 pixels and at most 5 MiB. recto makes every size
  from it, and a version for devices that crop icons to a circle or a rounded shape: your image,
  shrunk to the middle 80 %, on your color. Keep the important part of the picture away from the
  edges.

Without an icon, recto draws its own in your color.

recto reads the files once, at startup: after replacing one, restart recto.

## 3. Restart

```sh
docker compose up -d recto
```

## What refuses to start

recto checks the settings when it starts and stops with a message naming the setting if one is
wrong, rather than showing a broken page. The most common is a color that is too light:

```text
recto: RECTO_BRAND_COLOR #ffff00 gives white button text a contrast of 1.07:1; it needs 4.5:1. Choose a darker colour.
```

Buttons have white text on the color, and text needs a contrast of at least 4.5:1 with its
background to be readable by people with low vision (the WCAG 2 level AA). Pick a darker shade of
the same hue. The others are a name that is blank or too long, a file that cannot be read, is too
large or is not one of the accepted types, and an icon that is not square or is too small.

## Check

Reload recto in the browser: the header shows your name and logo, and the buttons your color.
Browsers keep icons for a while, and an installed app keeps the name and icon it was installed
with: to see the new ones, remove the app and install it again.
