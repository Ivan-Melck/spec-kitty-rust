use sk_glossary::{GlossaryError, GlossaryScope, load_seed_file, seed_path};
use std::fs;

#[test]
fn oversized_seed_is_refused_before_allocating_the_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed_path(GlossaryScope::TeamDomain, temp.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::File::create(&path)
        .unwrap()
        .set_len(16 * 1024 * 1024)
        .unwrap();
    assert!(matches!(
        load_seed_file(GlossaryScope::TeamDomain, temp.path()),
        Err(GlossaryError::BoundedRead { .. })
    ));
}

#[cfg(unix)]
#[test]
fn public_seed_loader_rejects_a_fifo_without_a_writer() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed_path(GlossaryScope::TeamDomain, temp.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(load_seed_file(GlossaryScope::TeamDomain, temp.path()));
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("loader must not block on FIFO");
    assert!(matches!(result, Err(GlossaryError::BoundedRead { .. })));
}
