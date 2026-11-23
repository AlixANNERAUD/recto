//! Requests in, answers out.
//!
//! The `ipp` crate parses and encodes; this adds the request checks of RFC 8011 section 4.1 it
//! leaves to servers, and reads the attributes of each operation recto serves.

use std::io::Cursor;

use bytes::Bytes;
use ipp::attribute::{IppAttribute, IppAttributeGroup};
use ipp::model::{DelimiterTag, IppVersion, StatusCode};
use ipp::parser::IppParser;
use ipp::reader::IppReader;
use ipp::request::IppRequestResponse;
use ipp::value::IppValue;

use super::{Media, Requested};
use crate::{ColorMode, PageRanges, Sides};

/// The one operation answered without credentials: clients read `uri-authentication-supported`
/// before they know they need any.
pub const GET_PRINTER_ATTRIBUTES: u16 = 0x000B;

/// `server-error-too-many-jobs` (RFC 8011 section 4.1.6.3), missing from the `ipp` crate's
/// `StatusCode`, so statuses are written raw.
pub const TOO_MANY_JOBS: u16 = 0x050B;

/// The first eight bytes of a request (RFC 8010 section 3.1.1), read before the rest of the body
/// so a request can be refused before its document is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: (u8, u8),
    pub operation: u16,
    pub request_id: u32,
}

pub fn header(first_eight: &[u8; 8]) -> Header {
    let [major, minor, op_high, op_low, id @ ..] = *first_eight;
    Header {
        version: (major, minor),
        operation: u16::from_be_bytes([op_high, op_low]),
        request_id: u32::from_be_bytes(id),
    }
}

/// The `which-jobs` of Get-Jobs. Held jobs are reported completed, so the default lists only
/// Create-Jobs still waiting for their document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhichJobs {
    NotCompleted,
    Completed,
    All,
    /// A value recto does not know, already in [`Incoming::unsupported`]: refused with
    /// `client-error-attributes-or-values-not-supported`, which then names it (RFC 8011
    /// section 4.2.6.1), not ignored.
    Unsupported,
}

/// An operation and what recto reads of it. `requested` lists are never empty: an absent
/// `requested-attributes` becomes the operation's default (`all`, or `job-id` and `job-uri` for
/// Get-Jobs).
#[derive(Clone, Debug)]
pub enum Operation {
    GetPrinterAttributes {
        requested: Vec<String>,
    },
    PrintJob {
        name: Option<String>,
        /// The declared `document-format`, a hint only: recto detects the format from the content.
        format: Option<String>,
        requested: Requested,
        document: Bytes,
    },
    ValidateJob {
        format: Option<String>,
    },
    CreateJob {
        name: Option<String>,
        requested: Requested,
    },
    SendDocument {
        job_id: i32,
        format: Option<String>,
        last: bool,
        document: Bytes,
    },
    CloseJob {
        job_id: i32,
    },
    CancelJob {
        job_id: i32,
    },
    /// Empty `job_ids`: every job of the device.
    CancelMyJobs {
        job_ids: Vec<i32>,
    },
    GetJobAttributes {
        job_id: i32,
        requested: Vec<String>,
    },
    GetJobs {
        which: WhichJobs,
        limit: Option<u32>,
        /// PWG 5100.11 `job-ids`: when not empty, only these jobs, whatever `which` says.
        job_ids: Vec<i32>,
        requested: Vec<String>,
    },
    IdentifyPrinter,
    /// Answered with `server-error-operation-not-supported`.
    Unsupported(u16),
    /// A job refused for its attributes: compression, or a value recto does not offer with
    /// `ipp-attribute-fidelity`. Answered with this status and, through [`response`], the
    /// unsupported attributes, which RFC 8011 section 4.1.7 asks a refusal to name.
    Refused(StatusCode),
}

/// A decoded request.
#[derive(Clone, Debug)]
pub struct Incoming {
    pub version: IppVersion,
    pub request_id: u32,
    pub operation: Operation,
    /// Attributes whose values recto does not offer, dropped from the job and reported back by
    /// [`response`] with `successful-ok-ignored-or-substituted-attributes`.
    pub unsupported: Vec<IppAttribute>,
    /// The request's `attributes-natural-language`, lowercase: the language the client wants its
    /// messages in.
    pub language: String,
}

/// Decodes a whole request body; the document is a slice of `body`, never a copy. The error is
/// the status to answer with, through [`refusal`].
pub fn decode(body: Bytes) -> Result<Incoming, StatusCode> {
    use StatusCode::ClientErrorBadRequest as Bad;

    let first: &[u8; 8] = body.get(..8).and_then(|b| b.try_into().ok()).ok_or(Bad)?;
    let head = header(first);
    // The versions of `ipp-versions-supported`.
    if !matches!(head.version, (1, 0..=1) | (2, 0)) {
        return Err(StatusCode::ServerErrorVersionNotSupported);
    }
    if head.request_id == 0 || i32::try_from(head.request_id).is_err() {
        return Err(Bad);
    }
    check_shape(&body)?;

    let (parsed, attributes, reader) = IppParser::new(IppReader::new(Cursor::new(body.clone())))
        .parse_parts()
        .map_err(|_| Bad)?;
    let offset = usize::try_from(reader.into_inner().position()).map_err(|_| Bad)?;
    let document = body.slice(offset..);

    let groups = attributes.groups();
    let operation_group = groups
        .first()
        .filter(|group| group.tag() == DelimiterTag::OperationAttributes)
        .ok_or(Bad)?;
    if groups
        .windows(2)
        .any(|pair| (pair[1].tag() as u8) < (pair[0].tag() as u8))
    {
        return Err(Bad);
    }
    check_operation_group(operation_group)?;
    let language = operation_group
        .get("attributes-natural-language")
        .and_then(|attribute| attribute.value().as_natural_language())
        .ok_or(Bad)?
        .to_ascii_lowercase();

    let request = Request {
        operation: operation_group,
        job: attributes.first_of(DelimiterTag::JobAttributes),
    };
    let mut unsupported = Vec::new();
    let operation = match head.operation {
        0x02 => match request
            .compression(&mut unsupported)
            .and_then(|()| request.requested(&mut unsupported))
        {
            Ok(requested) => Operation::PrintJob {
                name: request.name(),
                format: request.format(),
                requested,
                document,
            },
            Err(status) => Operation::Refused(status),
        },
        0x04 => match request
            .compression(&mut unsupported)
            .and_then(|()| request.requested(&mut unsupported))
        {
            Ok(_) => Operation::ValidateJob {
                format: request.format(),
            },
            Err(status) => Operation::Refused(status),
        },
        0x05 => match request.requested(&mut unsupported) {
            Ok(requested) => Operation::CreateJob {
                name: request.name(),
                requested,
            },
            Err(status) => Operation::Refused(status),
        },
        0x06 => {
            let last = request.operation("last-document").ok_or(Bad)?;
            let last = *last.as_boolean().ok_or(Bad)?;
            let job_id = request.job_id()?;
            match request.compression(&mut unsupported) {
                Ok(()) => Operation::SendDocument {
                    job_id,
                    format: request.format(),
                    last,
                    document,
                },
                Err(status) => Operation::Refused(status),
            }
        }
        0x08 => Operation::CancelJob {
            job_id: request.job_id()?,
        },
        0x09 => Operation::GetJobAttributes {
            job_id: request.job_id()?,
            requested: request.requested_attributes(&["all"]),
        },
        0x0A => Operation::GetJobs {
            which: match request.keyword("which-jobs").as_deref() {
                None | Some("not-completed") => WhichJobs::NotCompleted,
                Some("completed") => WhichJobs::Completed,
                Some("all") => WhichJobs::All,
                Some(_) => {
                    unsupported.extend(request.operation.get("which-jobs").map(echoable));
                    WhichJobs::Unsupported
                }
            },
            limit: request
                .operation("limit")
                .and_then(IppValue::as_integer)
                .and_then(|&limit| u32::try_from(limit).ok())
                .filter(|&limit| limit > 0),
            job_ids: request.job_ids()?,
            requested: request.requested_attributes(&["job-id", "job-uri"]),
        },
        GET_PRINTER_ATTRIBUTES => Operation::GetPrinterAttributes {
            requested: request.requested_attributes(&["all"]),
        },
        0x39 => Operation::CancelMyJobs {
            job_ids: request.job_ids()?,
        },
        0x3B => Operation::CloseJob {
            job_id: request.job_id()?,
        },
        0x3C => Operation::IdentifyPrinter,
        other => Operation::Unsupported(other),
    };

    Ok(Incoming {
        version: parsed.version,
        request_id: head.request_id,
        operation,
        unsupported,
        language,
    })
}

/// How deep collections may nest. recto reads `media-col` and its `media-size`, two levels.
const MAX_DEPTH: usize = 4;

/// How many attributes a request may carry, collection members included.
const MAX_ATTRIBUTES: usize = 1000;

/// How many values one attribute or member may carry.
const MAX_VALUES: usize = 64;

/// Walks the attribute section before the `ipp` crate parses it, refusing nesting, attributes
/// or values past the bounds above, and collections left open or closed twice. That parser
/// deep-clones each collection as it closes, recursively: about 4,000 levels, which fit in a
/// request read without credentials, cost quadratic time and can overflow the stack, which
/// aborts recto. This pass is linear and uses no recursion.
fn check_shape(body: &[u8]) -> Result<(), StatusCode> {
    use StatusCode::ClientErrorBadRequest as Bad;

    /// A two-byte length, then that many bytes, skipped.
    fn skip(body: &[u8], at: &mut usize) -> Result<usize, StatusCode> {
        let length = body
            .get(*at..*at + 2)
            .map(|bytes| usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
            .ok_or(Bad)?;
        *at += 2 + length;
        Ok(length)
    }

    let mut at = 8;
    // The values of the current attribute or member at each open level, outermost first.
    let mut values: Vec<usize> = vec![0];
    let mut attributes = 0;
    loop {
        let &tag = body.get(at).ok_or(Bad)?;
        at += 1;
        match tag {
            0x03 if values.len() == 1 => return Ok(()),
            // A group starts, or the attributes end, inside a collection.
            0x00..=0x0F if values.len() > 1 => return Err(Bad),
            0x00..=0x0F => {
                values[0] = 0;
                continue;
            }
            _ => {}
        }
        let name = skip(body, &mut at)?;
        skip(body, &mut at)?;
        // endCollection.
        if tag == 0x37 {
            if values.len() == 1 {
                return Err(Bad);
            }
            values.pop();
            continue;
        }
        let current = values.last_mut().ok_or(Bad)?;
        match tag {
            // memberAttrName: a member starts, its first value comes next.
            0x4A => {
                attributes += 1;
                *current = 0;
            }
            _ => {
                if name > 0 {
                    attributes += 1;
                    *current = 0;
                }
                *current += 1;
                if *current > MAX_VALUES {
                    return Err(Bad);
                }
                // begCollection.
                if tag == 0x34 {
                    if values.len() > MAX_DEPTH {
                        return Err(Bad);
                    }
                    values.push(0);
                }
            }
        }
        if attributes > MAX_ATTRIBUTES {
            return Err(Bad);
        }
    }
}

/// The answer to `incoming`: `status` (raw, see [`TOO_MANY_JOBS`]), then one group per entry of
/// `groups`, as Get-Jobs needs one job group per job. Unsupported attributes go back in their
/// own group, and turn `successful-ok` into `successful-ok-ignored-or-substituted-attributes`.
pub fn response(
    incoming: &Incoming,
    language: &str,
    status: u16,
    groups: Vec<(DelimiterTag, Vec<IppAttribute>)>,
) -> Bytes {
    let ignored = !incoming.unsupported.is_empty();
    let status = if ignored && status == StatusCode::SuccessfulOk as u16 {
        StatusCode::SuccessfulOkIgnoredOrSubstitutedAttributes as u16
    } else {
        status
    };
    let unsupported = ignored.then(|| {
        (
            DelimiterTag::UnsupportedAttributes,
            incoming.unsupported.clone(),
        )
    });
    encode(
        incoming.version,
        incoming.request_id,
        language,
        status,
        unsupported.into_iter().chain(groups),
    )
}

/// An unsupported attribute as it can go back to the client: as sent, or with the out-of-band
/// `unsupported` value when it holds an empty set, which would encode without a value length
/// (a collection left unclosed parses as one) and desync the client's parser.
pub(super) fn echoable(attribute: &IppAttribute) -> IppAttribute {
    fn encodes(value: &IppValue) -> bool {
        match value {
            IppValue::Array(values) => !values.is_empty() && values.iter().all(encodes),
            IppValue::Collection(members) => members.values().all(encodes),
            _ => true,
        }
    }
    if encodes(attribute.value()) {
        attribute.clone()
    } else {
        IppAttribute::new(
            attribute.name().clone(),
            IppValue::Other {
                tag: 0x10,
                data: Bytes::new(),
            },
        )
    }
}

/// The answer to a request [`decode`] refused, from the header the route already read.
pub fn refusal(header: &Header, status: u16) -> Bytes {
    // Answer in the closest version recto speaks when the request's is not one of them.
    let version = match header.version {
        (1, 0..=1) | (2, 0..=2) => {
            IppVersion(u16::from_be_bytes([header.version.0, header.version.1]))
        }
        (0 | 1, _) => IppVersion::v1_1(),
        _ => IppVersion::v2_0(),
    };
    encode(
        version,
        header.request_id,
        DEFAULT_LANGUAGE,
        status,
        std::iter::empty(),
    )
}

/// The `attributes-natural-language` of an answer to a request recto could not read.
const DEFAULT_LANGUAGE: &str = "en";

/// `language` is the response's `attributes-natural-language`: RFC 8011 section 4.1.4.2 has
/// every text the response carries in it.
fn encode(
    version: IppVersion,
    request_id: u32,
    language: &str,
    status: u16,
    groups: impl IntoIterator<Item = (DelimiterTag, Vec<IppAttribute>)>,
) -> Bytes {
    let id = i32::try_from(request_id).unwrap_or(0);
    let mut response = IppRequestResponse::new_response(version, StatusCode::SuccessfulOk, id)
        .expect("a response header always builds");
    response.header_mut().operation_or_status = i16::from_be_bytes(status.to_be_bytes());
    if let Ok(language) = IppValue::new_natural_language(language)
        && let Some(attribute) = response
            .attributes_mut()
            .groups_mut()
            .iter_mut()
            .filter(|group| group.tag() == DelimiterTag::OperationAttributes)
            .flat_map(|group| group.attributes_mut().iter_mut())
            .find(|attribute| attribute.name().as_str() == "attributes-natural-language")
    {
        *attribute = IppAttribute::new(attribute.name().clone(), language);
    }
    for (tag, attributes) in groups {
        // The encoder writes only the first operation group: extra operation attributes, such
        // as a status message, join it.
        if tag == DelimiterTag::OperationAttributes
            && let Some(operation) = response
                .attributes_mut()
                .groups_mut()
                .iter_mut()
                .find(|group| group.tag() == DelimiterTag::OperationAttributes)
        {
            operation.attributes_mut().extend(attributes);
            continue;
        }
        let mut group = IppAttributeGroup::new(tag);
        group.attributes_mut().extend(attributes);
        // One group each: `IppAttributes::add` would merge every job into the first job group.
        response.attributes_mut().groups_mut().push(group);
    }
    response.to_bytes()
}

/// RFC 8011 section 4.1.4: `attributes-charset` first, `attributes-natural-language` second, and a
/// target.
fn check_operation_group(group: &IppAttributeGroup) -> Result<(), StatusCode> {
    use StatusCode::ClientErrorBadRequest as Bad;

    let [charset, language, ..] = group.attributes() else {
        return Err(Bad);
    };
    if charset.name().as_str() != "attributes-charset" {
        return Err(Bad);
    }
    let charset = charset.value().as_charset().ok_or(Bad)?;
    if !["utf-8", "us-ascii"].contains(&charset.as_str().to_ascii_lowercase().as_str()) {
        return Err(StatusCode::ClientErrorCharsetNotSupported);
    }
    if language.name().as_str() != "attributes-natural-language"
        || language.value().as_natural_language().is_none()
    {
        return Err(Bad);
    }
    let target = |name| group.get(name).and_then(|a| a.value().as_uri()).is_some();
    if !target("printer-uri") && !target("job-uri") {
        return Err(Bad);
    }
    Ok(())
}

/// The values of a one-or-many attribute. Not `IppValue`'s own iterator, which walks into a
/// collection's members.
fn each(value: &IppValue) -> std::slice::Iter<'_, IppValue> {
    match value {
        IppValue::Array(values) => values.iter(),
        one => std::slice::from_ref(one).iter(),
    }
}

/// A string value of any string syntax; anything else, a string that was not UTF-8 included,
/// counts as absent.
fn string(value: &IppValue) -> Option<String> {
    match value {
        IppValue::NameWithoutLanguage(text) => Some(text.as_str().to_owned()),
        IppValue::NameWithLanguage { name, .. } => Some(name.as_str().to_owned()),
        IppValue::TextWithoutLanguage(text) => Some(text.as_ref().to_owned()),
        IppValue::TextWithLanguage { text, .. } => Some(text.as_ref().to_owned()),
        IppValue::Keyword(keyword) => Some(keyword.as_str().to_owned()),
        IppValue::MimeMediaType(mime) => Some(mime.as_str().to_owned()),
        _ => None,
    }
}

struct Request<'a> {
    operation: &'a IppAttributeGroup,
    job: Option<&'a IppAttributeGroup>,
}

impl Request<'_> {
    fn operation(&self, name: &str) -> Option<&IppValue> {
        self.operation.get(name).map(IppAttribute::value)
    }

    fn keyword(&self, name: &str) -> Option<String> {
        self.operation(name)
            .and_then(IppValue::as_keyword)
            .map(|k| k.as_str().to_owned())
    }

    fn name(&self) -> Option<String> {
        self.operation("job-name").and_then(string)
    }

    fn format(&self) -> Option<String> {
        self.operation("document-format").and_then(string)
    }

    /// Only uncompressed documents: recto would otherwise inflate untrusted data before any
    /// limit. A refusal names the attribute in `unsupported`.
    fn compression(&self, unsupported: &mut Vec<IppAttribute>) -> Result<(), StatusCode> {
        match self.keyword("compression").as_deref() {
            None | Some("none") => Ok(()),
            Some(_) => {
                unsupported.extend(self.operation.get("compression").map(echoable));
                Err(StatusCode::ClientErrorCompressionNotSupported)
            }
        }
    }

    /// `job-id` with `printer-uri`, or the last segment of `job-uri`.
    /// `job-ids`, empty when absent; a value that is not a positive integer is a bad request.
    fn job_ids(&self) -> Result<Vec<i32>, StatusCode> {
        self.operation("job-ids")
            .map(|value| {
                each(value)
                    .map(|id| {
                        id.as_integer()
                            .copied()
                            .filter(|&id| id > 0)
                            .ok_or(StatusCode::ClientErrorBadRequest)
                    })
                    .collect()
            })
            .transpose()
            .map(Option::unwrap_or_default)
    }

    fn job_id(&self) -> Result<i32, StatusCode> {
        let from_id = self
            .operation("job-id")
            .and_then(IppValue::as_integer)
            .copied();
        let from_uri = || {
            let uri = self.operation("job-uri")?.as_uri()?;
            uri.as_str().rsplit('/').next()?.parse::<i32>().ok()
        };
        from_id
            .or_else(from_uri)
            .filter(|&id| id > 0)
            .ok_or(StatusCode::ClientErrorBadRequest)
    }

    fn requested_attributes(&self, default: &[&str]) -> Vec<String> {
        let asked: Vec<String> = self
            .operation("requested-attributes")
            .map(|value| {
                each(value)
                    .filter_map(IppValue::as_keyword)
                    .map(|k| k.as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        if asked.is_empty() {
            default.iter().map(|name| (*name).to_owned()).collect()
        } else {
            asked
        }
    }

    /// A job template attribute: in the job group, or in the operation group where some older
    /// clients put them.
    fn template(&self, name: &str) -> Option<&IppAttribute> {
        self.job
            .and_then(|group| group.get(name))
            .or_else(|| self.operation.get(name))
    }

    /// The job template attributes recto keeps. Values it does not offer go to `unsupported`,
    /// or refuse the job when the client asked for `ipp-attribute-fidelity`.
    fn requested(&self, unsupported: &mut Vec<IppAttribute>) -> Result<Requested, StatusCode> {
        let mut requested = Requested::default();
        let mut read = |name: &str, apply: &mut dyn FnMut(&IppValue) -> bool| {
            if let Some(attribute) = self.template(name)
                && !apply(attribute.value())
            {
                unsupported.push(echoable(attribute));
            }
        };

        read("copies", &mut |value| {
            requested.copies = value
                .as_integer()
                .and_then(|&copies| u32::try_from(copies).ok())
                .filter(|copies| (1..=99).contains(copies));
            requested.copies.is_some()
        });
        read("sides", &mut |value| {
            requested.sides = value
                .as_keyword()
                .and_then(|k| Sides::from_keyword(k.as_str()));
            requested.sides.is_some()
        });
        read("print-color-mode", &mut |value| {
            match value.as_keyword().map(|k| k.as_str()) {
                // No preference: the release form keeps its own default.
                Some("auto") => true,
                keyword => {
                    requested.color_mode = keyword.and_then(ColorMode::from_keyword);
                    requested.color_mode.is_some()
                }
            }
        });
        read("page-ranges", &mut |value| {
            let ranges: Option<Vec<(u32, u32)>> = each(value)
                .map(|range| match range {
                    IppValue::RangeOfInteger { min, max } => {
                        Some((u32::try_from(*min).ok()?, u32::try_from(*max).ok()?))
                    }
                    _ => None,
                })
                .collect();
            requested.page_ranges = ranges.and_then(PageRanges::from_ranges);
            requested.page_ranges.is_some()
        });
        read("media", &mut |value| {
            requested.media = value
                .as_keyword()
                .and_then(|k| k.as_str().parse::<Media>().ok());
            requested.media.is_some()
        });
        if requested.media.is_none() {
            read("media-col", &mut |value| match media_from_col(value) {
                Ok(media) => {
                    requested.media = media;
                    true
                }
                Err(()) => false,
            });
        }
        // Advertised in `job-creation-attributes-supported` with one or a few values, kept
        // nowhere since the document is already laid out, but checked like the others.
        read("orientation-requested", &mut |value| {
            matches!(value, IppValue::Enum(3..=6))
        });
        read("print-quality", &mut |value| {
            matches!(value, IppValue::Enum(4))
        });
        read("printer-resolution", &mut |value| {
            matches!(
                value,
                IppValue::Resolution {
                    cross_feed: 300,
                    feed: 300,
                    units: 3
                }
            )
        });

        let fidelity = self
            .operation("ipp-attribute-fidelity")
            .and_then(IppValue::as_boolean)
            .copied()
            .unwrap_or(false);
        if fidelity && !unsupported.is_empty() {
            return Err(StatusCode::ClientErrorAttributesOrValuesNotSupported);
        }
        Ok(requested)
    }
}

/// The medium a `media-col` names, by `media-size-name` or by its `media-size`. `Ok(None)` when
/// it names no size (only a source or a type, say), `Err` when it names one recto does not offer.
fn media_from_col(value: &IppValue) -> Result<Option<Media>, ()> {
    let IppValue::Collection(members) = value else {
        return Err(());
    };
    if let Some(name) = members
        .iter()
        .find(|(k, _)| k.as_str() == "media-size-name")
    {
        let keyword = name.1.as_keyword().ok_or(())?;
        return keyword.as_str().parse::<Media>().map(Some).map_err(|_| ());
    }
    let Some((_, size)) = members.iter().find(|(k, _)| k.as_str() == "media-size") else {
        return Ok(None);
    };
    let IppValue::Collection(size) = size else {
        return Err(());
    };
    let dimension = |name: &str| {
        size.iter()
            .find(|(k, _)| k.as_str() == name)
            .and_then(|(_, v)| v.as_integer().copied())
    };
    let (Some(x), Some(y)) = (dimension("x-dimension"), dimension("y-dimension")) else {
        return Err(());
    };
    // Within a millimetre: clients convert from points or inches and round either way.
    Media::ALL
        .into_iter()
        .find(|media| {
            let (width, height) = media.size();
            width.abs_diff(x) <= 100 && height.abs_diff(y) <= 100
        })
        .map(Some)
        .ok_or(())
}
