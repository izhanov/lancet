//! `lt lang`: language plugins that make source code indexable.

use lancet_lib::lang::{self, GitHub};

use super::{Global, pretty_path};

#[derive(clap::Subcommand)]
pub enum Action {
    /// Install a tree-sitter grammar from GitHub: a short name (python), owner/repo,
    /// either with an optional @tag (latest release by default)
    Add {
        #[arg(required = true, value_name = "LANGUAGE")]
        specs: Vec<String>,
        /// Also index files with this extension (repeatable), e.g. --ext .rake
        #[arg(long = "ext", value_name = "EXT")]
        extensions: Vec<String>,
    },
    /// List installed languages
    List,
    /// Remove an installed language; its files drop out of the index on the next `lt index`
    Remove {
        #[arg(required = true, value_name = "LANGUAGE")]
        names: Vec<String>,
    },
}

pub async fn run(g: &Global, action: Action) -> anyhow::Result<()> {
    let dir = &g.languages;
    match action {
        Action::Add { specs, extensions } => {
            if !extensions.is_empty() && specs.len() > 1 {
                anyhow::bail!("--ext applies to one language at a time");
            }
            let gh = GitHub::default();
            for spec in &specs {
                for m in lang::add(dir, spec, &extensions, &gh).await? {
                    println!(
                        "installed {} ({}) from {}",
                        m.name,
                        m.extensions.join(" "),
                        m.source.as_deref().unwrap_or("?")
                    );
                }
            }
            println!("\nRun `lt index <path>` again to index files in these languages.");
        }
        Action::List => {
            let installed = lang::installed(dir)?;
            if installed.is_empty() {
                println!(
                    "no languages installed in {}; add one with `lt lang add <language>`",
                    pretty_path(&dir.to_string_lossy())
                );
            }
            for m in &installed {
                let source = m.source.as_deref().unwrap_or("hand-made");
                println!("{:<10} {:<24} {source}", m.name, m.extensions.join(" "));
            }
        }
        Action::Remove { names } => {
            for name in &names {
                if lang::remove(dir, name)? {
                    println!("removed {name}");
                } else {
                    println!("{name} is not installed");
                }
            }
        }
    }
    Ok(())
}
