use anyhow::{Result, bail};
use git_scoop_fixtures::{generate_benchmark, generate_suite};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let Some(destination) = arguments.next() else {
        bail!("Usage: git-scoop-fixtures <new-output-directory> [--benchmark]");
    };
    let benchmark = match arguments.next() {
        Some(argument) if argument == "--benchmark" => true,
        Some(_) => bail!("Unknown option. Use --benchmark to create the performance fixture."),
        None => false,
    };
    if arguments.next().is_some() {
        bail!("Too many arguments.");
    }
    let destination = PathBuf::from(destination);
    if benchmark {
        let manifest = generate_benchmark(&destination)?;
        println!(
            "Created benchmark: {} commits, {} files, {} source bytes",
            manifest.commits, manifest.files, manifest.source_bytes
        );
    } else {
        let suite = generate_suite(&destination)?;
        println!(
            "Created {} datasets at {}",
            suite.datasets.len(),
            destination.display()
        );
    }
    Ok(())
}
