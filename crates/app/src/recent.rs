//! Recently analyzed and saved repositories, kept beside the analysis caches
//! in the local application data directory. The list holds paths only.

use std::path::{Path, PathBuf};

/// Unsaved entries kept; saved entries never age out.
const LIMIT: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecentRepository {
    pub path: PathBuf,
    pub revision: String,
    /// Unix seconds of the last successful analysis; absent in older lists.
    #[serde(default)]
    pub opened_at: Option<i64>,
    /// Saved entries stay listed until the user unsaves or removes them.
    #[serde(default)]
    pub saved: bool,
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
            trim(&mut list);
            list
        })
        .unwrap_or_default()
}

/// Move the entry to the front, keeping its saved state, and write the list
/// atomically.
pub fn remember(
    root: &Path,
    list: &mut Vec<RecentRepository>,
    mut entry: RecentRepository,
) -> std::io::Result<()> {
    entry.saved |= list
        .iter()
        .any(|item| item.path == entry.path && item.saved);
    list.retain(|item| item.path != entry.path);
    list.insert(0, entry);
    trim(list);
    save(root, list)
}

/// Save or unsave an entry. An unsaved entry stays only while it is among
/// the most recent ones.
pub fn set_saved(
    root: &Path,
    list: &mut Vec<RecentRepository>,
    path: &Path,
    saved: bool,
) -> std::io::Result<()> {
    for item in list.iter_mut().filter(|item| item.path == path) {
        item.saved = saved;
    }
    trim(list);
    save(root, list)
}

pub fn forget(root: &Path, list: &mut Vec<RecentRepository>, path: &Path) -> std::io::Result<()> {
    list.retain(|item| item.path != path);
    save(root, list)
}

/// Keep every saved entry and the most recent unsaved ones, in order.
fn trim(list: &mut Vec<RecentRepository>) {
    let mut unsaved = 0;
    list.retain(|item| {
        if item.saved {
            return true;
        }
        unsaved += 1;
        unsaved <= LIMIT
    });
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
                    opened_at: Some(index),
                    saved: false,
                },
            )?;
        }
        remember(
            root.path(),
            &mut list,
            RecentRepository {
                path: PathBuf::from("repo-5"),
                revision: "main".into(),
                opened_at: Some(100),
                saved: false,
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
        // Lists written before `opened_at` existed still load.
        std::fs::write(
            file(root.path()),
            br#"[{"path":"legacy","revision":"HEAD"}]"#,
        )?;
        let legacy = load(root.path());
        assert_eq!(legacy[0].opened_at, None);
        assert!(!legacy[0].saved);
        std::fs::write(file(root.path()), b"not json")?;
        assert!(load(root.path()).is_empty());
        Ok(())
    }

    #[test]
    fn saved_entries_survive_newer_ones() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let mut list = Vec::new();
        let entry = |name: &str| RecentRepository {
            path: PathBuf::from(name),
            revision: "HEAD".into(),
            opened_at: None,
            saved: false,
        };
        remember(root.path(), &mut list, entry("kept"))?;
        set_saved(root.path(), &mut list, Path::new("kept"), true)?;
        for index in 0..LIMIT + 2 {
            remember(root.path(), &mut list, entry(&format!("repo-{index}")))?;
        }
        let loaded = load(root.path());
        assert_eq!(loaded.len(), LIMIT + 1);
        assert!(
            loaded
                .last()
                .is_some_and(|item| item.path == Path::new("kept") && item.saved)
        );
        // Opening a saved repository again keeps it saved.
        remember(root.path(), &mut list, entry("kept"))?;
        assert!(list[0].saved);
        // Unsaving drops it back into the recent window, where it still fits.
        set_saved(root.path(), &mut list, Path::new("kept"), false)?;
        assert_eq!(list.len(), LIMIT);
        assert!(list.iter().all(|item| !item.saved));
        assert_eq!(load(root.path()), list);
        Ok(())
    }
}
