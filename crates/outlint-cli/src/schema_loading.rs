//! Filesystem-backed schema reading and linked JSON Schema preloading.
//!
//! The core library is filesystem-agnostic: it consumes schema text and
//! already-loaded JSON Schema resources. This module is the CLI's filesystem
//! boundary, walking the `$ref` graph of a linked frontmatter schema and
//! handing the collected resources to `outlint-core`.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use outlint_core::{
    json_schema_external_references, linked_frontmatter_schema_path, load_schema_with_resources,
    InvalidSchema, JsonSchemaResourceContents, JsonSchemaResourceInput, LinkedJsonSchemaInput,
    LoadedSchema, SourceLabel,
};

pub(crate) fn read_and_load_schema(
    path: &Path,
    display: &str,
) -> Result<Result<LoadedSchema, InvalidSchema>, String> {
    let source = read_utf8_file(path, "schema")?;
    let external = linked_frontmatter_schema_path(&source)
        .map(|declared| preload_linked_json_schema(path, &declared))
        .transpose()?;
    Ok(load_schema_with_resources(
        &source,
        Some(SourceLabel(display.to_owned())),
        external,
    ))
}

const LOGICAL_JSON_SCHEMA_ORIGIN: &str = "https://outlint.invalid";

fn preload_linked_json_schema(
    schema_path: &Path,
    declared: &str,
) -> Result<LinkedJsonSchemaInput, String> {
    let schema_path = lexical_absolute(schema_path)?;
    let root_path = if Path::new(declared).is_absolute() {
        PathBuf::from(declared)
    } else {
        schema_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(declared)
    };
    let root_actual_uri = path_file_uri(&root_path)?;
    let root_logical_uri = logical_json_schema_uri(&root_actual_uri)?;
    let mut queue = VecDeque::from([(root_actual_uri, root_logical_uri.clone(), root_path)]);
    let mut visited = HashSet::new();
    let mut cached = HashMap::<PathBuf, Arc<str>>::new();
    let mut resources = Vec::new();

    while let Some((actual_uri, logical_uri, lexical_path)) = queue.pop_front() {
        if !visited.insert(logical_uri.clone()) {
            continue;
        }
        let canonical = fs::canonicalize(&lexical_path).unwrap_or_else(|_| lexical_path.clone());
        let contents = if let Some(text) = cached.get(&canonical) {
            JsonSchemaResourceContents::Loaded(Arc::clone(text))
        } else {
            match read_utf8_file(&lexical_path, "linked JSON Schema") {
                Ok(source) => {
                    let text = Arc::<str>::from(source);
                    cached.insert(canonical, Arc::clone(&text));
                    JsonSchemaResourceContents::Loaded(text)
                }
                Err(message) => JsonSchemaResourceContents::ReadFailure(message),
            }
        };
        let references = match &contents {
            JsonSchemaResourceContents::Loaded(text) => {
                json_schema_external_references(text, &actual_uri, &logical_uri).ok()
            }
            JsonSchemaResourceContents::ReadFailure(_) => None,
        };
        resources.push(JsonSchemaResourceInput {
            uri: logical_uri,
            label: Some(SourceLabel(path_display(&lexical_path))),
            contents,
        });
        let Some(references) = references else {
            continue;
        };
        for reference in references {
            let Some(path) = file_uri_path(&reference.physical_uri) else {
                continue;
            };
            queue.push_back((reference.physical_uri, reference.logical_uri, path));
        }
    }

    Ok(LinkedJsonSchemaInput {
        root_uri: root_logical_uri,
        resources,
    })
}

fn logical_json_schema_uri(file_uri: &str) -> Result<String, String> {
    // Preserve the complete lexical path so URI-relative references mirror
    // filesystem-relative references without aliasing distinct files.
    let path = file_uri
        .strip_prefix("file://")
        .ok_or_else(|| format!("JSON Schema path URI `{file_uri}` is not a file URI"))?;
    Ok(format!("{LOGICAL_JSON_SCHEMA_ORIGIN}{path}"))
}

fn lexical_absolute(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| format!("cannot determine current directory: {error}"))
    }
}

fn path_file_uri(path: &Path) -> Result<String, String> {
    let display = path
        .to_str()
        .ok_or_else(|| format!("path '{}' is not valid UTF-8", path_display(path)))?
        .replace('\\', "/");
    let mut uri = if display.starts_with('/') {
        "file://".to_owned()
    } else {
        "file:///".to_owned()
    };
    for byte in display.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            uri.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(uri, "%{byte:02X}").map_err(|error| error.to_string())?;
        }
    }
    Ok(uri)
}

fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let remainder = uri.strip_prefix("file://")?;
    let encoded = if remainder.starts_with('/') {
        remainder
    } else {
        let authority_end = remainder.find('/')?;
        let authority = remainder.get(..authority_end)?;
        if !authority.eq_ignore_ascii_case("localhost") {
            return None;
        }
        remainder.get(authority_end..)?
    };
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        if encoded.as_bytes().get(index) == Some(&b'%') {
            let hex = encoded.get(index + 1..index + 3)?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            bytes.push(*encoded.as_bytes().get(index)?);
            index += 1;
        }
    }
    String::from_utf8(bytes).ok().map(uri_decoded_path)
}

/// A file URI spells a Windows drive path with a slash before the drive
/// (`file:///C:/dir/x.json`); decoding must drop that slash or the resulting
/// `/C:/dir/x.json` is not a path Windows can read. On Unix `/C:/dir` is an
/// ordinary path and is preserved as-is.
fn uri_decoded_path(decoded: String) -> PathBuf {
    if cfg!(windows) {
        let bytes = decoded.as_bytes();
        if bytes.first() == Some(&b'/')
            && bytes.get(1).is_some_and(u8::is_ascii_alphabetic)
            && bytes.get(2) == Some(&b':')
            && matches!(bytes.get(3), None | Some(b'/'))
        {
            return PathBuf::from(&decoded[1..]);
        }
    }
    PathBuf::from(decoded)
}

/// Renders a path for diagnostics, labels, and error messages. Output paths
/// are canonical forward-slash on every platform; native separators are for
/// filesystem access only. On Unix a backslash is an ordinary name byte, so
/// this only rewrites separators where `\` cannot appear inside a name.
fn path_display(path: &Path) -> String {
    let text = path.display().to_string();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text
    }
}

pub(crate) fn read_utf8_file(path: &Path, kind: &str) -> Result<String, String> {
    let shown = path_display(path);
    let metadata =
        fs::metadata(path).map_err(|error| format!("cannot inspect {kind} '{shown}': {error}"))?;
    if metadata.is_dir() {
        return Err(format!(
            "{kind} '{shown}' is a directory; pass individual files instead"
        ));
    }
    let bytes = fs::read(path).map_err(|error| format!("cannot read {kind} '{shown}': {error}"))?;
    decode_utf8(bytes).map_err(|error| format!("{kind} '{shown}': {error}"))
}

pub(crate) fn read_stdin_utf8() -> Result<String, String> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read standard input: {error}"))?;
    decode_utf8(bytes).map_err(|error| format!("standard input: {error}"))
}

fn decode_utf8(mut bytes: Vec<u8>) -> Result<String, String> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        bytes.drain(..3);
    }
    String::from_utf8(bytes).map_err(|_| "input is not valid UTF-8".to_owned())
}

pub(crate) fn discover_schema(document: &Path) -> Result<PathBuf, String> {
    let absolute = if document.is_absolute() {
        document.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| format!("cannot determine current directory: {error}"))?
            .join(document)
    };
    // Candidate file names, most specific first: the document's stem (its
    // file name with the final extension removed) with `.outlint.yml`
    // appended, then the directory default. Spec section 11.2.
    let mut names: Vec<std::ffi::OsString> = Vec::new();
    if let Some(stem) = absolute.file_stem() {
        let mut name = stem.to_os_string();
        name.push(".outlint.yml");
        names.push(name);
    }
    names.push(std::ffi::OsString::from(".outlint.yml"));
    let mut directory = absolute.parent();
    while let Some(candidate_directory) = directory {
        for name in &names {
            let candidate = candidate_directory.join(name);
            // Only a regular file participates (spec section 11.2): a
            // directory named like a schema is skipped as if absent.
            match std::fs::metadata(&candidate) {
                Ok(metadata) if metadata.is_file() => return Ok(candidate),
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) => {}
                Err(error) => {
                    return Err(format!(
                        "cannot inspect schema candidate '{}': {error}",
                        path_display(&candidate)
                    ));
                }
            }
        }
        directory = candidate_directory.parent();
    }
    let expected = names
        .iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" or ");
    Err(format!(
        "no {expected} found for Markdown input '{}'",
        path_display(document)
    ))
}

pub(crate) fn display_path(path: &Path) -> String {
    let relative = env::current_dir()
        .ok()
        .and_then(|current| path.strip_prefix(current).ok())
        .filter(|path| !path.as_os_str().is_empty());
    path_display(relative.unwrap_or(path))
}

#[cfg(test)]
mod tests {
    use super::{decode_utf8, file_uri_path};
    use std::path::Path;

    #[test]
    fn accepts_and_removes_utf8_bom() {
        assert_eq!(
            decode_utf8(b"\xef\xbb\xbfhello".to_vec()),
            Ok("hello".into())
        );
    }

    #[test]
    fn file_uri_paths_accept_only_local_authorities() {
        assert_eq!(
            file_uri_path("file:///schemas/defs.json").as_deref(),
            Some(Path::new("/schemas/defs.json"))
        );
        assert_eq!(
            file_uri_path("file://LOCALHOST/schemas/defs.json").as_deref(),
            Some(Path::new("/schemas/defs.json"))
        );
        assert_eq!(file_uri_path("file://attacker.invalid/defs.json"), None);
        assert_eq!(file_uri_path("file://localhost.evil/defs.json"), None);
        assert_eq!(file_uri_path("file://localhost@evil/defs.json"), None);
        assert_eq!(file_uri_path("file://local%68ost/defs.json"), None);
    }
}
