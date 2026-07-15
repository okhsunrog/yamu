use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn sibling(destination: &Path, role: &str, extension: Option<&str>) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let nonce = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = format!(".yamu-{}-{nonce}-{role}", std::process::id());
    if let Some(extension) = extension {
        name.push('.');
        name.push_str(extension);
    }
    parent.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_name_does_not_inherit_destination_name() {
        let destination = Path::new("music").join(format!("{}.flac", "long title ".repeat(40)));
        let temporary = sibling(&destination, "normalized", Some("flac"));
        let name = temporary.file_name().unwrap().to_string_lossy();

        assert_eq!(temporary.parent(), Some(Path::new("music")));
        assert!(name.starts_with(".yamu-"));
        assert!(name.ends_with("-normalized.flac"));
        assert!(!name.contains("long title"));
        assert!(name.len() < 80);
    }
}
