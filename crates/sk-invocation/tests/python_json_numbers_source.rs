//! Source-first checks for the public exact JSON tree's numeric projection.
//! The Native conversion is pure; only the pinned Source runs a subprocess.

mod support;

use std::process::Command;

use sk_invocation::executor::PythonJsonValue;

#[test]
fn serde_numeric_tokens_preserve_python_integer_and_float_semantics() {
    let tokens = [
        "18446744073709551616",
        "1234567890123456789012345678901",
        "-18446744073709551617",
        "0",
        "-0",
        "-0.0",
        "10.0",
        "1e400",
        "-1e400",
        "1e-400",
        "-1e-400",
        "1e20",
        "0.00001",
    ];
    let temp = tempfile::tempdir().expect("isolated Source roots");
    let root = temp.path().canonicalize().expect("canonical root");
    for name in ["home", "config", "cache", "data", "state", "tmp"] {
        std::fs::create_dir(root.join(name)).expect("isolated directory");
    }
    let output = Command::new(support::python())
        .args([
            "-c",
            concat!(
                "import json,os,sys\n",
                "from pathlib import Path\n",
                "assert sys.version.split()[0]=='3.11.15',sys.version\n",
                "root=Path(os.environ['SK_NUMBER_ROOT'])\n",
                "assert Path.cwd()==root\n",
                "for key in ('HOME','TMPDIR','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XDG_STATE_HOME'):\n",
                " assert Path(os.environ[key]).parent==root,(key,os.environ[key])\n",
                "values=[json.dumps(json.loads(token)) for token in json.loads(sys.argv[1])]\n",
                "assert values[:3]==['18446744073709551616','1234567890123456789012345678901','-18446744073709551617'],values\n",
                "assert values[7:11]==['Infinity','-Infinity','0.0','-0.0'],values\n",
                "print(json.dumps(values))\n",
            ),
            &serde_json::to_string(&tokens).expect("fixture token list"),
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("TMPDIR", root.join("tmp"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("SK_NUMBER_ROOT", &root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .current_dir(&root)
        .output()
        .expect("pinned json oracle");
    assert!(
        output.status.success(),
        "Source must assert first: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source: Vec<String> = serde_json::from_slice(&output.stdout).expect("Source values");
    assert_eq!(source.len(), tokens.len());
    // Only after all Source execution/assertions: exercise the pure Native API.
    let mismatches: Vec<_> = tokens
        .iter()
        .zip(source)
        .filter_map(|(token, expected)| {
            let value: serde_json::Value = serde_json::from_str(token).expect("valid JSON token");
            let actual = PythonJsonValue::from_serde(&value).render(None);
            (actual != expected).then_some((token, expected, actual))
        })
        .collect();
    assert!(
        mismatches.is_empty(),
        "numeric data changed: {mismatches:?}"
    );
}
