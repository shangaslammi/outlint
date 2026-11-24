//! Destination checks and same-directory atomic replacement.
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};
use tempfile::NamedTempFile;

pub(super) struct Destination<'a> {
    path: &'a Path,
    metadata: Metadata,
    pub source: String,
}

impl<'a> Destination<'a> {
    pub fn read(path: &'a Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "must be a regular file",
            ));
        }
        if metadata.permissions().readonly() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "is read-only",
            ));
        }
        // Check effective write access in preview and apply without truncating the file.
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let metadata = file.metadata()?;
        if hard_linked(&file, &metadata)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "has more than one hard link",
            ));
        }
        let mut source = String::new();
        file.read_to_string(&mut source)?;
        Ok(Self {
            path,
            metadata,
            source,
        })
    }

    pub fn replace(&self, text: &str) -> io::Result<()> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        let mut temp = NamedTempFile::new_in(parent.unwrap_or(Path::new(".")))?;
        temp.write_all(text.as_bytes())?;
        temp.as_file()
            .set_permissions(self.metadata.permissions())?;
        temp.into_temp_path().persist(self.path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn hard_linked(_: &File, metadata: &Metadata) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    Ok(metadata.nlink() > 1)
}

#[cfg(windows)]
fn hard_linked(file: &File, _: &Metadata) -> io::Result<bool> {
    Ok(winapi_util::file::information(file)?.number_of_links() > 1)
}
