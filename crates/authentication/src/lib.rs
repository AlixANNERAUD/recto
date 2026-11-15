//! OpenID Connect sign-in and the cross-site request check.
//!
//! The server redirects the browser with [`OpenId::start`], keeps the [`PendingSignIn`] in the
//! session, then calls [`OpenId::finish`] from the callback and signs in the user of the
//! identity it gets.

pub mod claims;
pub mod csrf;
pub mod openid;

pub use openid::{OpenId, OpenIdError, OpenIdSettings, PendingSignIn, SignedIn};
