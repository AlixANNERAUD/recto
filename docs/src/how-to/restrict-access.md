# Let only some people in

By default, everyone your identity provider signs in can use recto. To limit it to some people,
give them a role or a group in your provider, and tell recto which claim of the sign-in token
carries it.

This only concerns single sign-on. Local accounts exist only when an administrator creates them,
and any account can be disabled on the Users page (see
[Manage users and administrators](manage-users.md#disable-a-user)).

Two settings do it:

- `RECTO_OIDC_REQUIRED_CLAIM`: the claim that must be in the token.
- `RECTO_OIDC_REQUIRED_VALUE`: the value the claim must hold. Without it, the claim only has to be
  present.

How the value is matched depends on the claim (see
[Configuration](../reference/configuration.md#required-claim) for the exact rules): a string must
equal it, a list must contain it, an object must have it as a key. A dotted name, such as
`realm_access.roles`, reaches into nested objects.

People who sign in without it see **No access to recto**, with a button to sign out and use
another account.

## Zitadel: a project role

1. In the project of recto's application, create a role with the key `recto`.
2. In the project's settings, turn on **Assert roles on authentication**.
3. Grant the `recto` role to the people who may use recto (**Authorizations**).
4. Set:

```yaml
RECTO_OIDC_SCOPES: urn:zitadel:iam:org:projects:roles
RECTO_OIDC_REQUIRED_CLAIM: urn:zitadel:iam:org:project:roles
RECTO_OIDC_REQUIRED_VALUE: recto
```

Zitadel sends the roles as an object keyed by role, which recto matches on the key.

## Keycloak: a realm role

1. Create a realm role named `recto`, and assign it to the people who may use recto.
2. In **Client scopes**, open `roles`, then its **realm roles** mapper, and turn on **Add to ID
   token**.
3. Set:

```yaml
RECTO_OIDC_REQUIRED_CLAIM: realm_access.roles
RECTO_OIDC_REQUIRED_VALUE: recto
```

For a client role, use `resource_access.<client id>.roles` instead.

## Authentik: a group

1. Create a group named `recto`, and add the people who may use recto.
2. Set:

```yaml
RECTO_OIDC_REQUIRED_CLAIM: groups
RECTO_OIDC_REQUIRED_VALUE: recto
```

Authentik puts the `groups` claim in the `profile` scope, which recto always asks for.

You can also bind the group to the application in Authentik itself (**Policy / Group / User
Bindings**): Authentik then refuses others before they reach recto.

## Check

Sign in with an account that has the role: you reach the Print page. Sign in with one that does
not: you see **No access to recto**. If everyone sees it, the claim is probably missing from the
token; recto also reads the UserInfo endpoint when it is, so check the claim's name and that the
provider sends it to this client.

## Keep jobs private

By default everyone signed in sees every print job and can cancel it. To keep each person's jobs
to themselves, set:

```yaml
RECTO_JOB_VISIBILITY: own
```

Then:

- People see, and can cancel, only the jobs they sent through recto. A job someone else sent is
  reported as missing.
- Administrators see and cancel every job.
- Jobs recto did not send (from another computer printing straight to CUPS) or sent before you
  turned the setting on are not recorded, so only administrators see them.

If recto cannot read its records of who sent what, it shows people no jobs and refuses their
cancellations rather than guess. Administrators are not affected.

Anyone who reaches CUPS directly still sees every job there: this setting limits recto only.

## Limit printers and scanners to groups

By default everyone signed in can use every printer and scanner. To limit one to some groups:

1. Send groups from your provider (see [Groups](../reference/accounts.md#groups)). Groups come
   from the provider's groups claim at each sign-in, and recto cannot edit them. Local accounts
   belong to no group: they cannot use a limited device unless they are administrators.
2. Open **Administration**, then **Printers** or **Scanners**.
3. Press **Change who may use it** on the device's row (**Change access and prices** or
   **Manage** for a printer), choose **These groups**, tick the groups and press **Save**.

Then:

- People outside the groups do not see the device on the Print, Printers or Scan pages, and
  recto refuses it if they ask for it anyway.
- Administrators can always use every device.
- Choosing **These groups** with none ticked leaves the device to administrators only.
- Choosing **Everyone** removes the rule.
- With `RECTO_JOB_VISIBILITY=all`, the Jobs page still lists the jobs of a printer someone
  cannot use: the limit is on printing, not on seeing the queue.
- A CUPS class is a device of its own. A class that holds a limited printer needs its own rule,
  or people print to the printer through the class.
- Anyone who reaches CUPS or SANE directly is not limited: the rules apply in recto only.

Rules are kept under the device's name. When a device is removed from CUPS or SANE, its rule
stays, and the row is marked **Not found**: if the device comes back, it is limited again. Press
**Forget** to drop the rule. The page lists the rules even when CUPS or SANE does not answer.
