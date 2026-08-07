use serde::Serialize;

/// One validation finding. Serialized form is a stable public interface;
/// see spec/outlint-spec.md section 6.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    /// Stable diagnostic id, e.g. "missing-section", "one_of".
    pub id: String,
    /// Header path in the document, e.g. "Overview > Goals".
    pub path: String,
    /// 1-based source line of the relevant header, if known.
    pub line: Option<u32>,
    /// Human-readable message.
    pub message: String,
}
