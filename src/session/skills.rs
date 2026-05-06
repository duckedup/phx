use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    User,
    Project,
    Explicit,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub dir: PathBuf,
    pub skill_md: PathBuf,
    pub source: SkillSource,
}

fn find_skill_md(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.eq_ignore_ascii_case("skill.md")
            && entry.file_type().is_ok_and(|ft| ft.is_file())
        {
            return Some(entry.path());
        }
    }
    None
}

pub fn discover_in(base: &Path, source: SkillSource) -> Vec<Skill> {
    let skills_dir = base.join("skills");
    let Ok(entries) = std::fs::read_dir(&skills_dir) else {
        return vec![];
    };
    let mut skills = vec![];
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let dir = entry.path();
        if let Some(skill_md) = find_skill_md(&dir) {
            skills.push(Skill {
                name,
                dir,
                skill_md,
                source: source.clone(),
            });
        }
    }
    skills.sort_by_key(|a| a.name.to_lowercase());
    skills
}

pub fn discover_layered(
    user_dir: &Path,
    project_dir: Option<&Path>,
    explicit: Option<&Path>,
) -> Vec<Skill> {
    let mut all = discover_in(user_dir, SkillSource::User);
    if let Some(p) = project_dir {
        all.extend(discover_in(p, SkillSource::Project));
    }
    if let Some(e) = explicit {
        all.extend(discover_in(e, SkillSource::Explicit));
    }
    all
}

pub fn load_skill_prompt(skill: &Skill) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(&skill.skill_md)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn empty_when_no_skills_dir() {
        let dir = tempdir().unwrap();
        let skills = discover_in(dir.path(), SkillSource::User);
        assert!(skills.is_empty());
    }

    #[test]
    fn finds_sorted_skills() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("beta")).unwrap();
        std::fs::write(skills_dir.join("beta/skill.md"), "# Beta").unwrap();
        std::fs::create_dir_all(skills_dir.join("alpha")).unwrap();
        std::fs::write(skills_dir.join("alpha/skill.md"), "# Alpha").unwrap();

        let skills = discover_in(dir.path(), SkillSource::User);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "alpha");
        assert_eq!(skills[1].name, "beta");
    }

    #[test]
    fn skips_dirs_without_skill_md() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("valid")).unwrap();
        std::fs::write(skills_dir.join("valid/skill.md"), "# Valid").unwrap();
        std::fs::create_dir_all(skills_dir.join("empty")).unwrap();

        let skills = discover_in(dir.path(), SkillSource::User);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "valid");
    }

    #[test]
    fn case_insensitive_skill_md() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("upper")).unwrap();
        std::fs::write(skills_dir.join("upper/SKILL.MD"), "# Upper").unwrap();

        let skills = discover_in(dir.path(), SkillSource::User);
        assert_eq!(skills.len(), 1);
    }

    #[test]
    fn layered_discovery() {
        let user = tempdir().unwrap();
        let project = tempdir().unwrap();
        std::fs::create_dir_all(user.path().join("skills/a")).unwrap();
        std::fs::write(user.path().join("skills/a/skill.md"), "").unwrap();
        std::fs::create_dir_all(project.path().join("skills/b")).unwrap();
        std::fs::write(project.path().join("skills/b/skill.md"), "").unwrap();

        let skills = discover_layered(user.path(), Some(project.path()), None);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].source, SkillSource::User);
        assert_eq!(skills[1].source, SkillSource::Project);
    }

    #[test]
    fn load_skill_prompt_reads_content() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("test")).unwrap();
        std::fs::write(skills_dir.join("test/skill.md"), "# Test Skill\nDo things.").unwrap();

        let skills = discover_in(dir.path(), SkillSource::User);
        assert_eq!(skills.len(), 1);
        let content = load_skill_prompt(&skills[0]).unwrap();
        assert!(content.contains("Test Skill"));
    }
}
