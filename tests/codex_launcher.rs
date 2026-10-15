mod common;
use common::Team;
use std::{os::unix::fs::symlink, path::Path};
use xxassxx::codex_launcher;
#[test]
fn manual_codex_uses_exact_directory_without_shell_or_automated_permission_flags() {
    let team = Team::new();
    let s = team.store("a");
    let root = team.dir.path().join("a/work");
    s.allow_directory(&root).unwrap();
    let sub = root.join("space and $(touch not-a-command)");
    std::fs::create_dir(&sub).unwrap();
    let command = codex_launcher::command(&s, &sub).unwrap();
    let command = command.as_std();
    assert_eq!(
        command.get_current_dir(),
        Some(sub.canonicalize().unwrap().as_path())
    );
    assert_eq!(command.get_args().count(), 0);
    assert!(codex_launcher::directory(&s, team.dir.path()).is_err());
    symlink(team.dir.path().join("b/work"), root.join("outside")).unwrap();
    assert!(codex_launcher::directory(&s, &root.join("outside")).is_err());
    let c = rusqlite::Connection::open(&s.path).unwrap();
    xxassxx::access_mode::set(&s, xxassxx::access_mode::Mode::FullAccess).unwrap();
    assert!(codex_launcher::directory(&s, Path::new("/")).is_err());
    c.execute("DELETE FROM file_roots", []).unwrap();
    assert!(codex_launcher::command(&s, &sub).is_err());
    assert_eq!(
        c.query_row("SELECT count(*) FROM app_tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
