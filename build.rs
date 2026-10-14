use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let mut version = None;
    for name in [
        "xxassxx-task-intake",
        "xxassxx-task-execution",
        "xxassxx-task-handoff",
    ] {
        let path = format!("skills/{name}/SKILL.md");
        println!("cargo:rerun-if-changed={path}");
        let text = fs::read_to_string(root.join(&path)).expect("bundled skill must exist");
        let (header, body) = text
            .strip_prefix("---\n")
            .expect("skill frontmatter")
            .split_once("\n---\n")
            .expect("skill body");
        assert!(
            header.lines().any(|l| l == format!("name: {name}")),
            "skill name mismatch: {path}"
        );
        assert!(
            header.lines().any(|l| l
                .strip_prefix("description:")
                .is_some_and(|d| !d.trim().is_empty())),
            "missing skill description: {path}"
        );
        assert!(!body.trim().is_empty(), "empty skill: {path}");
        let current = header
            .lines()
            .find_map(|l| l.strip_prefix("  version:"))
            .expect("skill version")
            .trim()
            .trim_matches('"')
            .to_owned();
        assert!(
            current.split('.').count() == 3
                && current
                    .split('.')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
            "invalid skill version"
        );
        if let Some(previous) = &version {
            assert_eq!(previous, &current, "skill bundle version mismatch");
        }
        version = Some(current);
    }
    println!(
        "cargo:rustc-env=XXASSXX_TASK_SKILL_VERSION={}",
        version.unwrap()
    );
}
