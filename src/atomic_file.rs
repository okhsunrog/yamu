use std::{io, path::Path};

/// Publishes a fully written sibling temporary file with one filesystem
/// operation. The no-replace form fails if the destination appeared while the
/// caller was producing the temporary file.
pub(crate) fn persist(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    let temporary = tempfile::TempPath::try_from_path(source.to_owned())?;
    let result = if replace {
        temporary.persist(destination)
    } else {
        temporary.persist_noclobber(destination)
    };
    result.map_err(|error| error.error)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn no_clobber_preserves_an_existing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"existing").unwrap();

        let error = persist(&source, &destination, false).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(destination).unwrap(), b"existing");
    }

    #[test]
    fn replace_atomically_publishes_the_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"existing").unwrap();

        persist(&source, &destination, true).unwrap();

        assert_eq!(fs::read(destination).unwrap(), b"new");
        assert!(!source.exists());
    }
}
