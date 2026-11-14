//! The interactive parts of the pages. Each island is rendered on the server like any
//! component, then takes over in the browser; the rest of the page stays plain HTML.

mod browser;
pub mod jobs;
pub mod print_form;
pub mod scan;
