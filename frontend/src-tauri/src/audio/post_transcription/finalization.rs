//! Native Stop proof. Finding an old file is not evidence of this save succeeding.
use std::path::{Path, PathBuf};

pub fn verified_audio<E>(
    folder: Option<&Path>,
    saved: &Result<Option<String>, E>,
) -> Option<PathBuf> {
    let path = saved.as_ref().ok()?.as_ref()?;
    let folder = folder?.canonicalize().ok()?;
    let path = Path::new(path).canonicalize().ok()?;
    let metadata = path.metadata().ok()?;
    (path.parent() == Some(folder.as_path()) && metadata.is_file() && metadata.len() > 0)
        .then_some(path)
}
