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

pub fn run_install_skill(target_root: &Path, force: bool) -> anyhow::Result<ExitCode> {
    let agents_skill_dir = target_root.join(".agents/skills/async-doctor");
    let skill_file = agents_skill_dir.join("SKILL.md");
    let claude_skills_dir = target_root.join(".claude/skills");
    let claude_link = claude_skills_dir.join("async-doctor");

    let skill_exists = skill_file.is_file();
    let skill_differs = skill_exists
        && std::fs::read_to_string(&skill_file)
            .map(|existing| existing != BUNDLED_SKILL_MD)
            .unwrap_or(true);
    let claude_conflict = claude_path_conflicts(&claude_link);

    if !force && (skill_differs || claude_conflict) {
        if skill_differs {
            println!(
                "skip: `{}` already exists and differs from the bundled skill; re-run with --force to overwrite",
                skill_file.display()
            );
        }
        if claude_conflict {
            println!(
                "skip: `{}` already exists and is not the expected skill link; re-run with --force to replace (a real directory is never deleted — move it aside first)",
                claude_link.display()
            );
        }
        return Ok(ExitCode::from(EXIT_OK as u8));
    }

    std::fs::create_dir_all(&agents_skill_dir)
        .with_context(|| format!("failed to create `{}`", agents_skill_dir.display()))?;
    std::fs::write(&skill_file, BUNDLED_SKILL_MD)
        .with_context(|| format!("failed to write `{}`", skill_file.display()))?;

    std::fs::create_dir_all(&claude_skills_dir)
        .with_context(|| format!("failed to create `{}`", claude_skills_dir.display()))?;

    if is_real_directory(&claude_link) {
        println!(
            "installed skill: {}; skipped Claude link because `{}` is a real directory (move it aside and re-run to create the symlink — install-skill never deletes unrelated user files)",
            skill_file.display(),
            claude_link.display()
        );
        return Ok(ExitCode::from(EXIT_OK as u8));
    }

    remove_replaceable_claude_path(&claude_link)?;
    create_claude_link(&claude_link, &agents_skill_dir)?;

    println!(
        "installed skill: {} (linked from {})",
        skill_file.display(),
        claude_link.display()
    );
    Ok(ExitCode::from(EXIT_OK as u8))
}

fn is_real_directory(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    meta.is_dir() && !meta.file_type().is_symlink()
}

/// True when `.claude/skills/async-doctor` exists and is not already the
/// expected relative symlink (Unix) or an up-to-date directory copy (Windows).
fn claude_path_conflicts(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return std::fs::read_link(path)
            .map(|target| target != Path::new(REL_SYMLINK_TARGET))
            .unwrap_or(true);
    }
    if meta.is_file() {
        return true;
    }
    if !meta.is_dir() {
        return true;
    }
    #[cfg(unix)]
    {
        // Real directory (user content or leftover) — never auto-delete.
        true
    }
    #[cfg(not(unix))]
    {
        // Windows installs a directory copy; treat identical SKILL.md as OK.
        let existing = path.join("SKILL.md");
        std::fs::read_to_string(existing)
            .map(|c| c != BUNDLED_SKILL_MD)
            .unwrap_or(true)
    }
}

/// Remove a symlink or plain file at the Claude skill path. Never deletes a
/// real directory tree (user files).
fn remove_replaceable_claude_path(path: &Path) -> anyhow::Result<()> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() || meta.is_file() {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove existing `{}`", path.display()))?;
    } else if meta.is_dir() {
        anyhow::bail!(
            "`{}` is a real directory; move or remove it manually before installing",
            path.display()
        );
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
