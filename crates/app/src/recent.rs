//! Recently analyzed repositories, kept beside the analysis caches in the
//! local application data directory. The list holds paths only.

use std::path::{Path, PathBuf};

const LIMIT: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecentRepository {
    pub path: PathBuf,
    pub revision: String,
}

impl RecentRepository {
    /// Folder name for display, falling back to the whole path.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

fn file(root: &Path) -> PathBuf {
    root.join("git-scoop").join("recent.json")
}

/// A missing or unreadable list starts empty; it only offers shortcuts.
pub fn load(root: &Path) -> Vec<RecentRepository> {
    std::fs::read(file(root))
        .ok()
        .and_then(|data| serde_json::from_slice::<Vec<RecentRepository>>(&data).ok())
        .map(|mut list| {
            list.truncate(LIMIT);
            list
        })
        .unwrap_or_default()
}

/// Move the entry to the front and write the list atomically.
pub fn remember(
    root: &Path,
    list: &mut Vec<RecentRepository>,
    entry: RecentRepository,
) -> std::io::Result<()> {
    list.retain(|item| item.path != entry.path);
    list.insert(0, entry);
    list.truncate(LIMIT);
    save(root, list)
}

pub fn forget(root: &Path, list: &mut Vec<RecentRepository>, path: &Path) -> std::io::Result<()> {
    list.retain(|item| item.path != path);
    save(root, list)
}

fn save(root: &Path, list: &[RecentRepository]) -> std::io::Result<()> {
    let target = file(root);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = target.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(list)?)?;
    std::fs::rename(temporary, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_most_recent_first_without_duplicates() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let mut list = load(root.path());
        assert!(list.is_empty());
        for index in 0..10 {
            remember(
                root.path(),
                &mut list,
                RecentRepository {
                    path: PathBuf::from(format!("repo-{index}")),
                    revision: "HEAD".into(),
                },
            )?;
        }
        remember(
            root.path(),
            &mut list,
            RecentRepository {
                path: PathBuf::from("repo-5"),
                revision: "main".into(),
            },
        )?;
        let loaded = load(root.path());
        assert_eq!(loaded, list);
        assert_eq!(loaded.len(), LIMIT);
        assert_eq!(loaded[0].path, PathBuf::from("repo-5"));
        assert_eq!(loaded[0].revision, "main");
        assert_eq!(
            loaded
                .iter()
                .filter(|item| item.path == Path::new("repo-5"))
                .count(),
            1
        );
        forget(root.path(), &mut list, Path::new("repo-5"))?;
        assert!(
            load(root.path())
                .iter()
                .all(|item| item.path != Path::new("repo-5"))
        );
        std::fs::write(file(root.path()), b"not json")?;
        assert!(load(root.path()).is_empty());
        Ok(())
    }
}
