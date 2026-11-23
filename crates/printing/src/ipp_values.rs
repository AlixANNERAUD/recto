//! Reading and writing IPP values, for the CUPS clients and the virtual printer alike.

use ipp::value::IppValue;

/// A set of values: one bare value when there is one, and none at all when empty, since an empty
/// `IppValue::Array` encodes without a value length and desyncs the reader's parser.
pub(crate) fn set(values: impl IntoIterator<Item = IppValue>) -> Option<IppValue> {
    let mut values: Vec<IppValue> = values.into_iter().collect();
    match values.len() {
        0 => None,
        1 => values.pop(),
        _ => Some(IppValue::Array(values)),
    }
}

/// The values of an attribute, the members of a set one by one.
pub(crate) fn each(value: &IppValue) -> std::slice::Iter<'_, IppValue> {
    match value {
        IppValue::Array(values) => values.iter(),
        one => std::slice::from_ref(one).iter(),
    }
}

/// The text of a name, text or keyword value. Out-of-band values (`unknown`, `no-value`, ...)
/// are not text: CUPS hides an attribute with them.
pub(crate) fn text(value: &IppValue) -> Option<&str> {
    match value {
        IppValue::NameWithoutLanguage(name) | IppValue::NameWithLanguage { name, .. } => {
            Some(name.as_str())
        }
        IppValue::TextWithoutLanguage(text) | IppValue::TextWithLanguage { text, .. } => {
            Some(text.as_ref())
        }
        IppValue::Keyword(keyword) => Some(keyword.as_str()),
        _ => None,
    }
}
