//! Named-capture-group participation analysis for regex matchers.
//!
//! Reserved for the regex-capture phase. Nothing is implemented here yet; the
//! module exists so the boundary is settled before the work starts.
//!
//! This module will own the question of which named capture groups a regex can
//! leave unset on a successful match, so that a declared capture whose group
//! did not participate is distinguished from one that matched an empty string.
