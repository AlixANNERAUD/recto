//! OpenID Connect sign-in and the cross-site request check.
//!
//! The server redirects the browser with [`OpenId::start`], keeps the [`PendingSignIn`] in the
//! session, then calls [`OpenId::finish`] from the callback and signs in the user of the
//! identity it gets. [`User`] is the signed-in user of a request, as the server hands it to the
//! interface.

pub mod claims;
pub mod csrf;
pub mod openid;
mod user;

pub use openid::{OpenId, OpenIdError, OpenIdSettings, PendingSignIn, SignedIn};
pub use user::User;
