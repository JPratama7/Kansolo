use serde::Serialize;
use std::path::PathBuf;

/// Metadata for a skill, parsed from `SKILL.md` frontmatter.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// Resolve the skills directory: env `ACP_SKILLS_DIR` > default
/// `~/.agents/skills`. The DB setting `acp_skills_dir` is NOT wired up
/// here — only the env var and the home-dir default are consulted.
fn skills_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ACP_SKILLS_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(home) = home_dir() {
        home.join(".agents/skills")
    } else {
        PathBuf::from(".agents/skills")
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Parse YAML-like frontmatter from a SKILL.md body.
/// Returns `(name, description, body_without_frontmatter)`.
fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>, String) {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() || lines[0].trim() != "---" {
        return (None, None, content.to_string());
    }
    let mut name = None;
    let mut description = None;
    let mut end_idx = None;
    for (i, line) in lines.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            end_idx = Some(i);
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "name" => name = Some(value.to_string()),
                "description" => description = Some(value.to_string()),
                _ => {}
            }
        }
    }
    match end_idx {
        Some(idx) => {
            let body = lines[idx + 1..].join("\n");
            (name, description, body)
        }
        None => (None, None, content.to_string()),
    }
}

/// List all skills in the skills directory, sorted by name.
pub fn list_skills() -> Vec<SkillManifest> {
    let dir = skills_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_file = path.join("SKILL.md");
        if !skill_file.exists() {
            continue;
        }
        let content = match std::fs::read_to_string(&skill_file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let (name, desc, _body) = parse_frontmatter(&content);
        let name = name.unwrap_or_else(|| entry.file_name().to_string_lossy().to_string());
        skills.push(SkillManifest {
            name,
            description: desc.unwrap_or_default(),
            path: skill_file,
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Load the body content of a skill by name (frontmatter stripped).
pub fn load_skill_content(name: &str) -> Option<String> {
    let path = skills_dir().join(name).join("SKILL.md");
    let content = std::fs::read_to_string(&path).ok()?;
    let (_name, _desc, body) = parse_frontmatter(&content);
    Some(body)
}

/// Load multiple skills by name, skipping missing ones.
pub fn load_skills(names: &[String]) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for name in names {
        match load_skill_content(name) {
            Some(content) => result.push((name.clone(), content)),
            None => continue,
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize env var access across parallel tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Holds the lock for the entire test duration.
    struct TestGuard {
        _guard: std::sync::MutexGuard<'static, ()>,
        dir: Option<PathBuf>,
    }
    impl Drop for TestGuard {
        fn drop(&mut self) {
            if let Some(ref dir) = self.dir {
                let _ = std::fs::remove_dir_all(dir);
            }
            std::env::remove_var("ACP_SKILLS_DIR");
        }
    }

    fn with_skills_dir() -> TestGuard {
        let guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("ACP_SKILLS_DIR", "/nonexistent/path/xyz");
        TestGuard {
            _guard: guard,
            dir: None,
        }
    }

    fn with_temp_skills_dir() -> (TestGuard, PathBuf) {
        let guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("tasker-skills-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ACP_SKILLS_DIR", dir.to_string_lossy().to_string());
        let g = TestGuard {
            _guard: guard,
            dir: Some(dir.clone()),
        };
        (g, dir)
    }

    #[test]
    fn missing_dir_returns_empty() {
        let _g = with_skills_dir();
        assert!(list_skills().is_empty());
    }

    #[test]
    fn missing_skill_file_returns_none() {
        let (_g, dir) = with_temp_skills_dir();
        std::fs::create_dir_all(dir.join("myskill")).unwrap();
        assert!(load_skill_content("myskill").is_none());
    }

    #[test]
    fn valid_frontmatter_parsed() {
        let (_g, dir) = with_temp_skills_dir();
        let skill_dir = dir.join("tdd");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: tdd\ndescription: Test-driven development\n---\n# TDD\nWrite tests first.",
        )
        .unwrap();
        let skills = list_skills();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "tdd");
        assert_eq!(skills[0].description, "Test-driven development");
        let content = load_skill_content("tdd").unwrap();
        assert!(content.contains("# TDD"));
        assert!(!content.contains("---"));
    }

    #[test]
    fn malformed_frontmatter_fallback() {
        let (_g, dir) = with_temp_skills_dir();
        let skill_dir = dir.join("broken");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: broken\nno closing frontmatter\nbody text",
        )
        .unwrap();
        let content = load_skill_content("broken").unwrap();
        assert!(content.contains("---"));
        assert!(content.contains("no closing"));
        let skills = list_skills();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "broken");
    }
}
