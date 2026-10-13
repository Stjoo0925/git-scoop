use crate::{Commit, Event, Observation};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

pub(crate) fn create_destination(root: &Path) -> Result<()> {
    if let Some(parent) = root
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(root).with_context(|| {
        format!(
            "Could not create a new fixture destination: {}",
            root.display()
        )
    })?;
    Ok(())
}

pub(crate) fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(variable);
    }
    command
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("LC_ALL", "C")
        .env("GIT_AUTHOR_NAME", "Git Scoop Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Git Scoop Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .args([
            "--no-pager",
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.eol=lf",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "user.name=Git Scoop Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.file.allow=always",
        ])
        .arg("-c")
        .arg(format!(
            "core.hooksPath={}",
            root.join(".fixture-hooks").display()
        ));
    command
}

pub fn git_output(root: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = git_command(root)
        .args(arguments)
        .output()
        .context("Could not execute Git for a fixture")?;
    check_output(output)
}

pub(crate) fn check_output(output: Output) -> Result<Vec<u8>> {
    ensure!(
        output.status.success(),
        "Fixture Git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

pub(crate) struct FixtureRepository {
    pub root: PathBuf,
    timestamp: u64,
    commit_index: u64,
}

impl FixtureRepository {
    pub fn create(root: &Path, seed: u64) -> Result<Self> {
        let absolute_root = std::path::absolute(root)?;
        let root = absolute_root.as_path();
        ensure!(
            !root.exists(),
            "Refusing to overwrite an existing fixture destination"
        );
        create_destination(root)?;
        git_output(
            root,
            &["init", "--initial-branch=main", "--object-format=sha1"],
        )?;
        Ok(Self {
            root: root.to_owned(),
            timestamp: 1704067200 + seed,
            commit_index: 0,
        })
    }

    pub fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let target = self.root.join(path);
        ensure!(
            target.starts_with(&self.root)
                && !Path::new(path).is_absolute()
                && !path.contains(".."),
            "Invalid fixture path"
        );
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, data)?;
        Ok(())
    }

    pub fn remove(&self, path: &str) -> Result<()> {
        ensure!(
            !Path::new(path).is_absolute() && !path.contains(".."),
            "Invalid fixture path"
        );
        fs::remove_file(self.root.join(path))?;
        Ok(())
    }

    pub fn commit(
        &mut self,
        message: &str,
        mut observations: Vec<Observation>,
        events: Vec<Event>,
    ) -> Result<Commit> {
        git_output(&self.root, &["add", "--all"])?;
        let date = format!("@{} +0000", self.timestamp + self.commit_index * 60);
        let output = git_command(&self.root)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .args(["commit", "--quiet", "-m", message])
            .output()?;
        check_output(output)?;
        self.commit_index += 1;
        let oid = String::from_utf8(git_output(&self.root, &["rev-parse", "HEAD"])?)?
            .trim()
            .to_owned();
        let parents = String::from_utf8(git_output(
            &self.root,
            &["show", "-s", "--format=%P", "HEAD"],
        )?)?
        .split_whitespace()
        .map(str::to_owned)
        .collect();
        for observation in &mut observations {
            observation.commit.clone_from(&oid);
        }
        Ok(Commit {
            oid,
            parents,
            observations,
            boundary_events: events,
        })
    }

    pub fn merge(&mut self, branch: &str) -> Result<String> {
        let date = format!("@{} +0000", self.timestamp + self.commit_index * 60);
        let output = git_command(&self.root)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .args(["merge", "--no-ff", "--no-edit", branch])
            .output()?;
        check_output(output)?;
        self.commit_index += 1;
        Ok(
            String::from_utf8(git_output(&self.root, &["rev-parse", "HEAD"])?)?
                .trim()
                .to_owned(),
        )
    }
}
