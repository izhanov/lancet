//! `lt skill`: the Claude Code skill that teaches AI agents to search with
//! Lancet. It is embedded in the binary, so installing needs no copy of the
//! repository.

use std::path::PathBuf;

use anyhow::Context;

/// The skill's directory name under `~/.claude/skills/`.
pub const NAME: &str = "lancet-search";

pub const CONTENT: &str = include_str!("../../skill/SKILL.md");

#[derive(clap::Subcommand)]
pub enum Action {
    /// Install the skill into ~/.claude/skills (overwrites a previous version)
    Install,
    /// Remove the installed skill
    Uninstall,
    /// Print the skill to stdout
    Show,
}

pub fn run(action: Action) -> anyhow::Result<()> {
    match action {
        Action::Install => {
            let path = dir()?.join("SKILL.md");
            std::fs::create_dir_all(path.parent().expect("SKILL.md has a parent"))
                .and_then(|()| std::fs::write(&path, CONTENT))
                .with_context(|| format!("install the skill into {}", path.display()))?;
            println!("skill installed: {}", path.display());
            println!("\nIt loads automatically in new Claude Code sessions. To make agents");
            println!("reach for it proactively, add a line to your ~/.claude/CLAUDE.md:");
            println!(
                "\n  Before tasks involving my notes, docs or style guides, use the {NAME} skill."
            );
        }
        Action::Uninstall => {
            let dir = dir()?;
            match std::fs::remove_dir_all(&dir) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(e).with_context(|| format!("remove {}", dir.display()));
                }
                _ => println!("skill removed: {}", dir.display()),
            }
        }
        Action::Show => print!("{CONTENT}"),
    }
    Ok(())
}

pub fn installed() -> bool {
    dir().is_ok_and(|dir| dir.join("SKILL.md").is_file())
}

/// `~/.claude/skills/lancet-search`.
fn dir() -> anyhow::Result<PathBuf> {
    let home = std::env::home_dir().context("cannot locate the home directory")?;
    Ok(home.join(".claude").join("skills").join(NAME))
}
