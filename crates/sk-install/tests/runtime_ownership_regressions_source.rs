//! Source-first adversarial owner/lock/config regressions from independent I1.
//! Linux/Unix execution evidence, not a general platform or ownership verdict.
#![cfg(unix)]
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

const CASES: &[&str] = &[
    "lock-replace",
    "lock-mtime",
    "prune",
    "version",
    "unknown",
    "uppercase-sha",
    "symlink-mode",
    "umask",
    "agent",
    "empty-unreadable",
];
const VICTIM: &[u8] = b"FOREIGN DATA MUST SURVIVE\n";

fn environment(command: &mut Command, root: &Path, repo: &Path) {
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("SK_RUNTIME_ROOT", root)
        .current_dir(root);
    for (key, name) in environment_paths() {
        command.env(key, root.join(name));
    }
}

fn environment_paths() -> [(&'static str, &'static str); 9] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]
}

fn sandbox(root: &Path) {
    fs::create_dir_all(root).unwrap();
    for (_, name) in environment_paths() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
}

fn native_case(case: &str, root: &Path) {
    use sk_install::migrations::completion_modern::global_runtime::ensure_runtime_with;
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in environment_paths() {
        assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
    }
    let user = root.join("home");
    let home = user.join(".kittify");
    let ensure = || ensure_runtime_with(&home, &user, "4.0.0rc8");
    ensure().unwrap().expect("package source present");
    let cache = home.join("cache");
    let lock = cache.join(".update.lock");
    let inventory = cache.join("runtime_bootstrap-assets.json");
    let version = cache.join("version.lock");
    match case {
        "lock-replace" => {
            let victim = root.join("foreign-victim");
            fs::write(&victim, VICTIM).unwrap();
            fs::write(
                home.join("missions/software-dev/mission.yaml"),
                "operator edit\n",
            )
            .unwrap();
            let watched = lock.clone();
            let saved = cache.join("held-original");
            let target = victim.clone();
            let saved_for_thread = saved.clone();
            let thread = std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while std::time::Instant::now() < deadline {
                    if fs::metadata(&watched).is_ok_and(|metadata| metadata.len() > 0) {
                        fs::rename(&watched, &saved_for_thread).unwrap();
                        symlink(&target, &watched).unwrap();
                        return true;
                    }
                    std::thread::yield_now();
                }
                false
            });
            let result = ensure();
            assert!(
                thread.join().unwrap(),
                "fault-injection premise must be reached"
            );
            assert_eq!(
                fs::read(&victim).unwrap(),
                VICTIM,
                "never truncate the foreign inode"
            );
            assert!(
                fs::symlink_metadata(&lock)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert!(
                result.is_err(),
                "Source recheck refuses a non-file owner lock"
            );
            assert_eq!(
                fs::read(saved).unwrap(),
                b"",
                "release clears the held descriptor"
            );
        }
        "lock-mtime" => {
            let before = fs::metadata(&lock).unwrap().modified().unwrap();
            fs::write(
                home.join("missions/software-dev/mission.yaml"),
                "operator edit\n",
            )
            .unwrap();
            ensure().unwrap();
            let after = fs::metadata(&lock).unwrap().modified().unwrap();
            assert_ne!(
                before, after,
                "effectful release has its observed clock, not a restored mtime"
            );
            assert_eq!(fs::read(&lock).unwrap(), b"");
            assert_eq!(
                fs::metadata(&lock).unwrap().permissions().mode() & 0o7777,
                0o644
            );
            let warm = fs::metadata(&lock).unwrap().modified().unwrap();
            ensure().unwrap();
            assert_eq!(
                warm,
                fs::metadata(&lock).unwrap().modified().unwrap(),
                "warm pass never acquires the lock"
            );
        }
        "prune" => {
            let mut data: serde_json::Value =
                serde_json::from_slice(&fs::read(&inventory).unwrap()).unwrap();
            let entries = data["entries"].as_object_mut().unwrap();
            let old = home.join("missions/software-dev/olddir");
            fs::create_dir_all(&old).unwrap();
            fs::write(old.join("recorded"), "recorded\n").unwrap();
            fs::write(old.join("operator"), "keep\n").unwrap();
            let prefix = "home/.kittify/missions/software-dev/olddir";
            entries.insert(prefix.to_owned(), serde_json::json!({"kind":"directory","mode":493,"sha256":null,"target":null,"mtime_ns":null}));
            entries.insert(format!("{prefix}/recorded"), serde_json::json!({"kind":"file","mode":420,"sha256":sk_install::manifest::content_hash(b"recorded\n"),"target":null,"mtime_ns":null}));
            fs::write(&inventory, serde_json::to_vec(&data).unwrap()).unwrap();
            ensure().unwrap();
            assert!(old.is_dir());
            assert_eq!(fs::read(old.join("operator")).unwrap(), b"keep\n");
            assert!(!old.join("recorded").exists());
        }
        "version" => {
            fs::write(&version, "4.0.0rc8\n").unwrap();
            ensure().unwrap();
            assert_eq!(fs::read(version).unwrap(), b"4.0.0rc8");
        }
        "unknown" | "uppercase-sha" | "symlink-mode" => {
            let mut data: serde_json::Value =
                serde_json::from_slice(&fs::read(&inventory).unwrap()).unwrap();
            let node = &mut data["entries"]["home/.kittify/missions/software-dev/mission.yaml"];
            match case {
                "unknown" => node["unexpected"] = serde_json::json!("foreign"),
                "uppercase-sha" => node["sha256"] = serde_json::json!("A".repeat(64)),
                "symlink-mode" => {
                    *node = serde_json::json!({"kind":"symlink","mode":null,"sha256":null,"target":"x","mtime_ns":null});
                }
                _ => unreachable!(),
            }
            let bytes = serde_json::to_vec(&data).unwrap();
            fs::write(&inventory, &bytes).unwrap();
            assert!(ensure().is_err());
            assert_eq!(fs::read(inventory).unwrap(), bytes);
        }
        "umask" => {
            assert_eq!(
                fs::metadata(&cache).unwrap().permissions().mode() & 0o7777,
                0o755
            );
            for path in [inventory, version, lock] {
                assert_eq!(
                    fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
                    0o644,
                    "{}",
                    path.display()
                );
            }
        }
        "agent" => {
            let project = root.join("project");
            fs::create_dir_all(project.join(".kittify")).unwrap();
            for key in ["CLAUDE", "Claude", " claude "] {
                fs::write(
                    project.join(".kittify/config.yaml"),
                    format!("agents:\n  available: [{key:?}]\n"),
                )
                .unwrap();
                assert!(
                    sk_install::migrations::completion_modern::support::load_agent_config(&project)
                        .is_err()
                );
            }
            assert!(
                sk_install::agent_surface(" CLAUDE ").is_ok(),
                "Native CLI aliases remain a distinct capability"
            );
        }
        "empty-unreadable" => {
            let legacy = root.join("legacy");
            fs::create_dir_all(legacy.join("hidden")).unwrap();
            fs::set_permissions(legacy.join("hidden"), fs::Permissions::from_mode(0o0)).unwrap();
            assert!(
                sk_install::preserve::Prover::EmptyTree
                    .prove(&legacy, root)
                    .is_some()
            );
        }
        _ => unreachable!(),
    }
}

const SOURCE: &str = r#"
import os, sys, json, hashlib, threading, time
from pathlib import Path
root = Path(os.environ['SK_RUNTIME_ROOT'])
assert Path.cwd() == root and sys.version_info[:3] == (3,11,15)
for key,name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert Path(os.environ[key]) == root/name
from specify_cli.runtime.bootstrap import ensure_runtime, get_kittify_home
home = get_kittify_home()
assert home == root/'home/.kittify'
ensure_runtime()
cache=home/'cache'; lock=cache/'.update.lock'; inventory=cache/'runtime_bootstrap-assets.json'; version=cache/'version.lock'
case=sys.argv[1]
if case=='lock-replace':
    victim=root/'foreign-victim'; victim.write_bytes(b'FOREIGN DATA MUST SURVIVE\n')
    (home/'missions/software-dev/mission.yaml').write_text('operator edit\n')
    saved=cache/'held-original'; hit=[]
    def replace():
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            try:
                if lock.stat().st_size:
                    lock.rename(saved); lock.symlink_to(victim); hit.append(True); return
            except FileNotFoundError: pass
            time.sleep(0)
    thread=threading.Thread(target=replace); thread.start()
    try: ensure_runtime(); outcome='ok'
    except Exception as exc: outcome=type(exc).__name__
    thread.join()
    assert hit and victim.read_bytes()==b'FOREIGN DATA MUST SURVIVE\n' and lock.is_symlink()
    assert outcome!='ok' and saved.read_bytes()==b''
elif case=='lock-mtime':
    before=lock.stat().st_mtime_ns
    (home/'missions/software-dev/mission.yaml').write_text('operator edit\n')
    ensure_runtime(); after=lock.stat().st_mtime_ns
    assert before!=after and lock.read_bytes()==b'' and (lock.stat().st_mode & 0o7777)==0o644
    ensure_runtime(); assert lock.stat().st_mtime_ns==after
elif case=='prune':
    data=json.loads(inventory.read_text()); entries=data['entries']; prefix='home/.kittify/missions/software-dev/olddir'
    entries[prefix]=dict(kind='directory',mode=0o755,sha256=None,target=None,mtime_ns=None)
    entries[prefix+'/recorded']=dict(kind='file',mode=0o644,sha256=hashlib.sha256(b'recorded\n').hexdigest(),target=None,mtime_ns=None)
    inventory.write_text(json.dumps(data))
    old=home/'missions/software-dev/olddir'; old.mkdir(); (old/'recorded').write_bytes(b'recorded\n'); (old/'operator').write_bytes(b'keep\n')
    ensure_runtime(); assert old.is_dir() and (old/'operator').read_bytes()==b'keep\n' and not (old/'recorded').exists()
elif case=='version':
    version.write_bytes(b'4.0.0rc8\n'); ensure_runtime(); assert version.read_bytes()==b'4.0.0rc8'
elif case in ('unknown','uppercase-sha','symlink-mode'):
    data=json.loads(inventory.read_text()); node=data['entries']['home/.kittify/missions/software-dev/mission.yaml']
    if case=='unknown': node['unexpected']='foreign'
    elif case=='uppercase-sha': node['sha256']='A'*64
    else: data['entries']['home/.kittify/missions/software-dev/mission.yaml']=dict(kind='symlink',mode=None,sha256=None,target='x',mtime_ns=None)
    raw=json.dumps(data).encode(); inventory.write_bytes(raw)
    try: ensure_runtime(); refused=False
    except Exception: refused=True
    assert refused and inventory.read_bytes()==raw
elif case=='umask':
    assert (cache.stat().st_mode & 0o7777)==0o755
    for path in (inventory,version,lock): assert (path.stat().st_mode & 0o7777)==0o644, path
elif case=='agent':
    from specify_cli.core.agent_config import load_agent_config, AgentConfigError
    project=root/'project'; (project/'.kittify').mkdir(parents=True)
    for key in ('CLAUDE','Claude',' claude '):
        (project/'.kittify/config.yaml').write_text('agents:\n  available: ['+json.dumps(key)+']\n')
        try: load_agent_config(project); refused=False
        except AgentConfigError: refused=True
        assert refused, key
elif case=='empty-unreadable':
    from specify_cli.upgrade.migrations.m_4_0_0rc6_charter_pack_cutover import _EmptyTreeProver
    legacy=root/'legacy'; (legacy/'hidden').mkdir(parents=True); (legacy/'hidden').chmod(0)
    assert _EmptyTreeProver().prove(legacy,root) is not None
else: raise AssertionError(case)
print(json.dumps({'case':case,'asserted':True}))
"#;

#[test]
fn runtime_owned_regressions() {
    if let Ok(case) = std::env::var("SK_RUNTIME_NATIVE_CASE") {
        native_case(
            &case,
            &PathBuf::from(std::env::var_os("SK_RUNTIME_ROOT").unwrap()),
        );
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    for case in CASES {
        let source_root = root.join(case).join("source");
        sandbox(&source_root);
        let native_root = root.join(case).join("native");
        sandbox(&native_root);
        let python = oracle::python();
        let mut source = Command::new("/bin/bash");
        source
            .args(["-c", "umask 077; exec \"$@\"", "bash"])
            .arg(&python)
            .args(["-c", SOURCE, case]);
        environment(&mut source, &source_root, &repo);
        let output = source.output().unwrap();
        fs::write(source_root.join("stdout"), &output.stdout).unwrap();
        fs::write(source_root.join("stderr"), &output.stderr).unwrap();
        assert!(
            output.status.success(),
            "Source {case} must assert FIRST: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut native = Command::new("/bin/bash");
        native
            .args(["-c", "umask 077; exec \"$@\"", "bash"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", "runtime_owned_regressions", "--nocapture"])
            .env("SK_RUNTIME_NATIVE_CASE", case);
        environment(&mut native, &native_root, &repo);
        // environment() clears inherited variables, so declare the child mode last.
        native.env("SK_RUNTIME_NATIVE_CASE", case);
        let output = native.output().unwrap();
        fs::write(native_root.join("stdout"), &output.stdout).unwrap();
        fs::write(native_root.join("stderr"), &output.stderr).unwrap();
        assert!(
            output.status.success(),
            "Native {case}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
