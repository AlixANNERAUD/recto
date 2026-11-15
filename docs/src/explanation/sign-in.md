# Sign-in and sessions

## Why OpenID Connect only

A home server often already has an identity provider, for its other self-hosted apps. Using it
means one account per person across all of them, two-factor authentication and passkeys for free,
and no passwords stored in recto. recto has no local accounts at all, so there is nothing in it to
leak or to reset.

## The flow

recto is a *public client*: it has no client secret, because a secret in a self-hosted app is
one more thing to generate, store and rotate, and adds little when the redirect URIs are fixed.
It uses the authorization code flow with **PKCE**, which protects the code on its way back:

1. Someone opens recto without a session. recto creates a random `state`, a `nonce` and a PKCE
   verifier, keeps them in a short-lived pending session (10 minutes), and redirects to the
   provider.
2. The person signs in at the provider, which redirects back to `/auth/callback` with a code.
3. recto checks the `state`, exchanges the code with the verifier, and checks the ID token's
   signature, issuer, audience, expiry and nonce.
4. When the token lacks the profile claims or the required claim, recto asks the UserInfo
   endpoint for them.
5. recto checks the required claim, if one is configured, then starts a fresh session with a new
   id, so an id seen before sign-in is worthless after it.

The provider's details come from its discovery document, fetched at the first sign-in rather than
at startup: recto starts even when the provider is down, and recovers without a restart when it
comes back. When the provider rotates its signing keys, recto fetches them again on the first
token signed with an unknown key.

## Who the person is

recto shows a display name (`name`) and keeps a username, taken from the first claim present:
`preferred_username`, then `email`, then `sub`. The username is what CUPS sees as the owner of
print jobs, and what scan results are attached to.

## Sessions

A session is a random id in a cookie (`HttpOnly`, `SameSite=Lax`, and `Secure` when
`RECTO_PUBLIC_URL` is HTTPS), pointing to data kept in recto's memory. It lasts 12 hours after
the last request, so a session in daily use does not expire in the middle of a scan.

Sessions are not stored on disk. A restart signs everyone out, and it is the only cost: signing
back in is one redirect when the provider's own session is still valid.

Signing out ends the session in recto, then sends the browser to the provider's end-session
endpoint, when it has one, so the provider's session ends too.
