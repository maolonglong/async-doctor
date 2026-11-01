//! `async-doctor install-skill` subcommand.
//!
//! Installs the bundled `skills/async-doctor/SKILL.md` into a target project's
//! `.agents/skills/async-doctor/SKILL.md` and creates a relative symlink at
//! `.claude/skills/async-doctor -> ../../.agents/skills/async-doctor`.

use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;
use async_doctor_core::EXIT_OK;

const BUNDLED_SKILL_MD: &str = include_str!("../../../skills/async-doctor/SKILL.md");
const REL_SYMLINK_TARGET: &str = "../../.agents/skills/async-doctor";

pub fn run_install_skill(target_root: &Path) -> anyhow::Result<ExitCode> {
    let agents_skill_dir = target_root.join(".agents/skills/async-doctor");
    std::fs::create_dir_all(&agents_skill_dir)
        .with_context(|| format!("failed to create `{}`", agents_skill_dir.display()))?;

    let skill_file = agents_skill_dir.join("SKILL.md");
    std::fs::write(&skill_file, BUNDLED_SKILL_MD)
        .with_context(|| format!("failed to write `{}`", skill_file.display()))?;

    let claude_skills_dir = target_root.join(".claude/skills");
    std::fs::create_dir_all(&claude_skills_dir)
        .with_context(|| format!("failed to create `{}`", claude_skills_dir.display()))?;

    let claude_link = claude_skills_dir.join("async-doctor");
    remove_existing_link_or_dir(&claude_link)?;
    create_claude_link(&claude_link, &agents_skill_dir)?;

    println!(
        "installed skill: {} (linked from {})",
        skill_file.display(),
        claude_link.display()
    );
    Ok(ExitCode::from(EXIT_OK as u8))
}

fn remove_existing_link_or_dir(path: &Path) -> anyhow::Result<()> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() || meta.is_file() {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove existing `{}`", path.display()))?;
    } else if meta.is_dir() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove existing directory `{}`", path.display()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_claude_link(claude_link: &Path, _agents_skill_dir: &Path) -> anyhow::Result<()> {
    std::os::unix::fs::symlink(REL_SYMLINK_TARGET, claude_link).with_context(|| {
        format!(
            "failed to symlink `{}` -> `{REL_SYMLINK_TARGET}`",
            claude_link.display()
        )
    })
}

#[cfg(not(unix))]
fn create_claude_link(claude_link: &Path, agents_skill_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(claude_link)
        .with_context(|| format!("failed to create `{}`", claude_link.display()))?;
    std::fs::copy(
        agents_skill_dir.join("SKILL.md"),
        claude_link.join("SKILL.md"),
    )
    .with_context(|| format!("failed to copy SKILL.md into `{}`", claude_link.display()))?;
    Ok(())
}
