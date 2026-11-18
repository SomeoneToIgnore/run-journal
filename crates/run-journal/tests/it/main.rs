#![expect(
    clippy::unwrap_used,
    reason = "a broken fixture fails its test where it breaks"
)]

mod resume;
mod support;
