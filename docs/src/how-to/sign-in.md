# Set up sign-in

recto signs people in with an OpenID Connect provider. This guide creates the application in
your provider and gives its details to recto.

## What every provider needs

Create a **public client**: no client secret, the **authorization code** flow, and **PKCE** with
`S256`. Register two addresses, where `{RECTO_PUBLIC_URL}` is the address people open:

| Setting | Value |
| --- | --- |
| Redirect URI | `{RECTO_PUBLIC_URL}/auth/callback` |
| Post-logout redirect URI | `{RECTO_PUBLIC_URL}/auth/signed-out` |

Then set, in `.env` or under `environment`:

```sh
RECTO_OIDC_ISSUER=https://id.example.org
RECTO_OIDC_CLIENT_ID=the-client-id
```

The issuer is the URL under which your provider serves
`/.well-known/openid-configuration`. recto asks for the `openid`, `profile` and `email` scopes.

From then on, everyone your provider signs in can use recto. To let in only some people, see
[Let only some people in](restrict-access.md).

## Zitadel

1. In a project, create an application of type **User Agent**, with authentication method
   **PKCE**.
2. Add the two URIs above. For an `http://` address, such as on a test machine, turn on
   **Development mode**.
3. Copy the **Client ID** into `RECTO_OIDC_CLIENT_ID`, and set `RECTO_OIDC_ISSUER` to your
   instance's address, such as `https://id.example.org`.
4. Optionally, in the application's **Token settings**, turn on **User Info inside ID Token**.
   Without it, recto reads the username and display name from the UserInfo endpoint at each
   sign-in, which costs one more request.

## Keycloak

1. In your realm, create a client of type **OpenID Connect**.
2. Turn **Client authentication** off, and keep only **Standard flow**.
3. Set **Valid redirect URIs** and **Valid post logout redirect URIs** to the two URIs above, and
   **Web origins** to `{RECTO_PUBLIC_URL}`.
4. Under **Advanced**, set **Proof Key for Code Exchange Code Challenge Method** to `S256`.
5. Set `RECTO_OIDC_ISSUER` to `https://keycloak.example.org/realms/<realm>` and
   `RECTO_OIDC_CLIENT_ID` to the client id.

## Authentik

1. Create a provider of type **OAuth2/OpenID Provider** with **Client type** set to **Public**.
2. Add the redirect URI above as a **Strict** redirect URI.
3. Create an **Application** that uses this provider. Its slug is part of the issuer.
4. Set `RECTO_OIDC_ISSUER` to `https://authentik.example.org/application/o/<slug>/` and
   `RECTO_OIDC_CLIENT_ID` to the provider's client id.

## Check

Open recto: it sends you to your provider, and back to the Print page with your name at the top.
If you land on **Sign-in failed** instead, or your provider shows an error about the redirect
URI, see [Troubleshoot](troubleshoot.md#sign-in).
