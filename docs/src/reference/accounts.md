# Accounts

recto keeps its users in its [database](configuration.md#database). A user signs in with a
**local account**, a username and a password, or through **single sign-on** with an OpenID
Connect provider. Some users are **administrators**. Every signed-in user can use every printer
and scanner; being an administrator only adds the administration pages.

## Local accounts

Local accounts are on by default (`RECTO_LOCAL_ACCOUNTS=true`). People sign in at `/sign-in`
with their username or their email address, and their password. There is no self-registration
and no password reset by email: an administrator creates the account and sets its passwords.

- **Usernames** are 1 to 64 characters among lower-case letters, digits, `.`, `_` and `-`,
  starting with a letter or a digit. They never change.
- **Display names** are shown in the header. They default to the username.
- **Email addresses** are optional and not verified. They are another way to sign in, matched
  in any case, and each belongs to one user at most.
- **Passwords** have at least `RECTO_PASSWORD_MINIMUM_LENGTH` characters (12 by default) and at
  most 1,024. There are no composition rules. They are stored as Argon2id hashes.

With a provider configured, `/sign-in` sends people straight to single sign-on as long as no
account has a password, since the form could sign nobody in; the form appears once an account
has one. With `RECTO_LOCAL_ACCOUNTS=false`, the password form is gone for good and `/sign-in`
always sends people to single sign-on. Local accounts stay in the database and work again once
the setting is turned back on.

### Password sign-in limit

Password sign-in attempts are counted per client address: a burst of 5, then one more every 4
seconds. Beyond that, recto answers `429 Too Many Requests` with **Too many attempts. Wait a few
seconds, then try again.**, even to the right password. A password change on the Account page
checks the current password, so it counts as an attempt too, from the same budget.

An IPv6 client counts by its `/64` network rather than by its full address, since one home or
machine usually has the whole network to pick addresses from.

The client address is the address of the connection, so behind a reverse proxy every client
shares the proxy's budget. With `RECTO_TRUST_FORWARDED_HEADERS=true`, recto reads it from
`X-Forwarded-For`, `X-Real-IP` or `Forwarded` instead; set it only behind a proxy that replaces
those headers (see [Run behind a reverse proxy](../how-to/reverse-proxy.md#count-sign-in-attempts-per-person)).

A wrong username, a wrong password, an account without a password and a disabled account all
get the same answer, **The username or password is wrong.**, in the same time.

## Single sign-on users

With `RECTO_OIDC_ISSUER` and `RECTO_OIDC_CLIENT_ID` set, people can sign in through the
provider. A user is created at their first sign-in, and identified from then on by the
provider's **issuer and subject** (`iss` and `sub`), never by username or email address.

- The **username** is the first non-empty claim of `preferred_username`, `email` and `sub`, kept
  exactly as the provider sends it, with `-2`, `-3`… appended when another user has it. It is
  set at the first sign-in and never changes, even when the claim does.
- The **display name** (`name`, else the username) and the **email address** (unless another
  user has it) are updated at each sign-in.

The username is what CUPS records as the owner of print jobs, and what scan results belong to.

A local account and a single sign-on user are two users, even with the same email address;
recto does not link them.

## Administrators

Administrators reach the **Administration** pages, at `/administration/users` and
`/administration/groups`, where they:

- create local accounts, administrators or not;
- set a new password for another user who has one;
- disable and enable other users;
- make other users administrators, or remove their administration;
- see the groups and their members.

They cannot do any of this to their own account, so the last administrator cannot lock everyone
out from the page. For everyone else, the administration pages answer as if they did not exist.

An administrator is made in one of three ways:

| How | Where |
| --- | --- |
| On the Users page, by another administrator | [Manage users and administrators](../how-to/manage-users.md) |
| On the command line, `recto create-administrator` or `recto administrator` | [Command line](command-line.md#recto-create-administrator) |
| By the administrator claim of the identity provider | [Below](#administrator-claim) |

### Administrator claim

With `RECTO_OIDC_ADMINISTRATOR_CLAIM` set, a single sign-on user is an administrator when the
claim matches `RECTO_OIDC_ADMINISTRATOR_VALUE`: a string equal to it, a list containing it, or an
object with it as a key (Zitadel's roles). Without a value, the claim only has to be present and
not `false` or `null`. The name is looked up whole first, then as a path of names separated by
dots (`realm_access.roles`).

The claim is applied at each sign-in:

- It grants administration to a user who does not have it. The Users page then shows **From the
  identity provider**, and refuses to change it: change it in the provider, or on the server with
  `recto administrator <USERNAME> --remove`.
- It takes back only the administration it granted. An administrator made on the Users page or
  on the command line stays one, whatever the claim says.
- When the claim is missing from the token, recto asks the UserInfo endpoint. If that request
  fails, administration is left as it is. A provider without a UserInfo endpoint puts every claim
  in the token, so there a missing claim takes back the administration it granted.

## Groups

With `RECTO_OIDC_GROUPS_CLAIM` set, recto reads a single sign-on user's groups from that claim at
each sign-in: a string is one group, a list holds one group per string, and an object one group
per key. The user's memberships are replaced by the groups read, and groups seen for the first
time are created. Groups are never deleted, even without members.

Groups are read-only in recto. The Users page lists each user's groups, and the Groups page each
group's members. They do not change what anyone may do in recto. If the UserInfo request
fails, memberships are left as they are; with a provider without a UserInfo endpoint, a groups
claim missing from the token means no groups.

## Disabled users

A disabled user cannot sign in: the password form answers as for a wrong password, and single
sign-on ends on **No access to recto**. Their sessions end at their next request. Enabling them
again lets them sign in; it does not bring their sessions back.

## Sessions

A sign-in starts a session, stored in the database, so restarts and upgrades do not sign anyone
out. The `recto_session` cookie holds only the session's random identifier. It is `HttpOnly`,
`SameSite=Lax`, and `Secure` when `RECTO_PUBLIC_URL` starts with `https://`.

A session ends:

- after 12 hours without a request (10 minutes for a single sign-on that has not come back from
  the provider);
- when the user signs out;
- when their password is set or changed, except, for a change on the Account page, the session
  that made it;
- when they are disabled.

Expired sessions are deleted from the database every 5 minutes.
