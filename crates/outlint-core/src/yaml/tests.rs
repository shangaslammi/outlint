//! Canonicalization of YAML scalars, tested against the shared conversion.

use proptest::prelude::*;

use super::scalar::{canonical_float, canonical_integer, parse_frontmatter_scalar};
use crate::{CanonicalFloat, CanonicalInteger, FrontmatterScalar};

#[test]
fn yaml_core_scalars_support_arbitrary_magnitude_without_signed_nan() {
    assert_eq!(
        parse_frontmatter_scalar("1e100000000000000000000000000000000000000"),
        FrontmatterScalar::Float(CanonicalFloat(
            "1e100000000000000000000000000000000000000".into()
        ))
    );
    assert_eq!(
        parse_frontmatter_scalar("-0xffffffffffffffffffffffffffffffff"),
        FrontmatterScalar::Integer(CanonicalInteger(
            "-340282366920938463463374607431768211455".into()
        ))
    );
    assert_eq!(
        parse_frontmatter_scalar("-.nan"),
        FrontmatterScalar::String("-.nan".into())
    );
    assert_eq!(
        parse_frontmatter_scalar("+.NaN"),
        FrontmatterScalar::String("+.NaN".into())
    );
}

proptest! {
    #[test]
    fn canonical_integer_normalization_is_idempotent(value in any::<i64>()) {
        let source = if value >= 0 {
            format!("+000{value}")
        } else {
            format!("-000{}", value.unsigned_abs())
        };
        let canonical = canonical_integer(&source).expect("generated decimal is valid");
        prop_assert_eq!(canonical.as_str(), value.to_string());
        let repeated = canonical_integer(&canonical);
        prop_assert_eq!(repeated.as_deref(), Some(canonical.as_str()));
    }

    #[test]
    fn canonical_float_normalization_is_idempotent(
        coefficient in any::<i64>(),
        exponent in any::<i16>(),
    ) {
        let source = format!("{coefficient}e{exponent}");
        let canonical = canonical_float(&source).expect("generated decimal float is valid");
        let repeated = canonical_float(&canonical);
        prop_assert_eq!(repeated.as_deref(), Some(canonical.as_str()));
    }
}
