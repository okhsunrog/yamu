use std::{io, path::Path};

pub fn persist(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    let temporary = tempfile::TempPath::try_from_path(source.to_owned())?;
    let result = if replace {
        temporary.persist(destination)
    } else {
        temporary.persist_noclobber(destination)
    };
    result.map_err(|error| error.error)
}
