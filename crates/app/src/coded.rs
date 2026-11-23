//! Enums named in the address by a short code, such as why a form was refused or what it did.
//! Only codes travel there, so a crafted link cannot put arbitrary text on the page.

/// Defines an enum whose variants each have a code: `code` gives it and `from_code` reads it
/// back, from `ALL`, which lists every variant so that no code is left unreadable.
macro_rules! coded {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident = $code:literal,)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        $vis enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            const ALL: &[$name] = &[$($name::$variant),+];

            /// The code that names it in the address.
            pub fn code(self) -> &'static str {
                match self {
                    $($name::$variant => $code,)+
                }
            }

            /// What `code` names, if anything.
            pub fn from_code(code: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.code() == code)
            }
        }
    };
}

pub(crate) use coded;
