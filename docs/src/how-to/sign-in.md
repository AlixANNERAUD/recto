# Set up single sign-on

recto can sign people in with an OpenID Connect provider, such as Zitadel, Keycloak or Authentik,
next to its local accounts or instead of them. This guide creates the application in your
provider, gives its details to recto, and optionally lets the provider name the administrators
and send the groups.

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

Set both, or neither: recto refuses to start with only one.

From then on, everyone your provider signs in can use recto. Each person gets a user at their
first sign-in, named after their `preferred_username` (see
[Single sign-on users](../reference/accounts.md#single-sign-on-users)). To let in only some
people, see [Let only some people in](restrict-access.md).

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

## Make administrators from the provider

Rather than appointing administrators in recto, you can give them a role or a group in the
provider. Name the claim that carries it, and the value that makes an administrator:

- `RECTO_OIDC_ADMINISTRATOR_CLAIM`: the claim, such as `groups`.
- `RECTO_OIDC_ADMINISTRATOR_VALUE`: the role or group, such as `recto-admins`. Without it, the
  claim only has to be present and not `false`.

The claim is matched like the required claim of [Let only some people in](restrict-access.md),
and applied at each sign-in (see
[Administrator claim](../reference/accounts.md#administrator-claim)). Administrators appointed in
recto stay administrators whatever the claim says.

**Zitadel**: create a project role `recto-admin`, turn on **Assert roles on authentication** in
the project, grant the role, and set:

```yaml
RECTO_OIDC_SCOPES: urn:zitadel:iam:org:projects:roles
RECTO_OIDC_ADMINISTRATOR_CLAIM: urn:zitadel:iam:org:project:roles
RECTO_OIDC_ADMINISTRATOR_VALUE: recto-admin
```

**Keycloak**: create a realm role `recto-admin`, assign it, turn on **Add to ID token** in the
**realm roles** mapper of the `roles` client scope, and set:

```yaml
RECTO_OIDC_ADMINISTRATOR_CLAIM: realm_access.roles
RECTO_OIDC_ADMINISTRATOR_VALUE: recto-admin
```

**Authentik**: create a group `recto-admins`, add the administrators, and set:

```yaml
RECTO_OIDC_ADMINISTRATOR_CLAIM: groups
RECTO_OIDC_ADMINISTRATOR_VALUE: recto-admins
```

## Show the groups from the provider

With `RECTO_OIDC_GROUPS_CLAIM`, recto reads each person's groups at sign-in and shows them on the
**Users** and **Groups** tabs of the Administration page. They are read-only in recto, and do not
change what anyone may do.

**Zitadel** sends no groups, but its project roles can stand in for them; each role becomes a
group:

```yaml
RECTO_OIDC_SCOPES: urn:zitadel:iam:org:projects:roles
RECTO_OIDC_GROUPS_CLAIM: urn:zitadel:iam:org:project:roles
```

**Keycloak**: in the client's **Client scopes** tab, open `<client id>-dedicated` and add a
mapper, **By configuration**, of type **Group Membership**, with **Token Claim Name** `groups`,
**Full group path** off and **Add to ID token** on. Then set:

```yaml
RECTO_OIDC_GROUPS_CLAIM: groups
```

**Authentik** puts the `groups` claim in the `profile` scope, which recto always asks for:

```yaml
RECTO_OIDC_GROUPS_CLAIM: groups
```

## Use single sign-on only

Local accounts stay on next to single sign-on: the sign-in page shows the password form and a
**Sign in with single sign-on** button. To send everyone straight to the provider instead, as
recto did before it had local accounts, set:

```yaml
RECTO_LOCAL_ACCOUNTS: "false"
```

Local accounts already created are kept, and work again if you turn the setting back on.

## Check

Open recto and press **Sign in with single sign-on** (or nothing, with local accounts off): recto
sends you to your provider, and back to the Print page with your name at the top. If you land on
**Sign-in failed** instead, or your provider shows an error about the redirect URI, see
[Troubleshoot](troubleshoot.md#sign-in).
