#![no_main]

//! Decode arbitrary fuzz-supplied bytes through `parse_wbmp`. The
//! decoder must always return a `Result` and never panic / abort /
//! OOM, regardless of how malformed the input is.
//!
//! The contract under test is purely that the call *returns*: a
//! malformed stream yields `Err(WbmpError::…)`, a well-formed one
//! yields `Ok(WbmpImage)`, and neither path may panic, integer-overflow
//! (in a debug build), index out of bounds, or try to allocate an
//! attacker-controlled pixel buffer the size of the claimed
//! `ceil(width / 8) * height`. `parse_wbmp` applies the default
//! `WbmpLimits`, so a hostile header is rejected before the decoder
//! touches its allocator. The return value is intentionally discarded.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::parse_wbmp;

fuzz_target!(|data: &[u8]| {
    let _ = parse_wbmp(data);
});
