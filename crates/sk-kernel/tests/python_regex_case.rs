//! Source-first literal IGNORECASE facts, distinct from str.lower/casefold.

#[path = "support/oracle.rs"]
mod oracle;

use std::process::Command;

#[test]
fn regex_literal_case_keys_match_all_pinned_source_codepoints() {
    let root = tempfile::tempdir().expect("Source roots").keep();
    for directory in ["home", "tmp", "config", "cache", "data", "state"] {
        std::fs::create_dir(root.join(directory)).expect("isolated directory");
    }
    let result = Command::new(oracle::python())
        .args(["-c", r#"
import _sre,hashlib,json,os,re,struct,sys,unicodedata
from pathlib import Path
from re._casefix import _EXTRA_CASES
root=Path(os.environ['SK_REGEX_ROOT']);assert Path.cwd()==root
assert sys.version_info[:3]==(3,11,15)
assert unicodedata.unidata_version=='14.0.0'
for key in ('HOME','TMPDIR','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XDG_STATE_HOME'):
    assert Path(os.environ[key]).parent==root
keys=[]
for code in range(0x110000):
    lower=_sre.unicode_tolower(code)
    assert lower==ord(chr(code).lower()[0]),code
    keys.append(min((lower,*_EXTRA_CASES.get(lower,()))))
# Bind the classes to the actual regex matcher, not just an internal table.
for code,others in _EXTRA_CASES.items():
    variants={code,*others}
    for value in tuple(variants):
        upper=chr(value).upper()
        if len(upper)==1:variants.add(ord(upper))
    for a in variants:
        for b in variants:
            assert bool(re.fullmatch(re.escape(chr(a)),chr(b),re.I))==(keys[a]==keys[b]),(a,b)
for a,b,expected in [(ord('s'),0x17f,True),(ord('i'),0x131,True),(ord('i'),0x130,True),(0x3c3,0x3c2,True),(0x3bc,0xb5,True),(ord('a'),ord('b'),False),(0xd800,0xd801,False)]:
    assert bool(re.fullmatch(re.escape(chr(a)),chr(b),re.I))==expected,(a,b)
    assert (keys[a]==keys[b])==expected,(a,b)
raw=struct.pack('<%dI'%len(keys),*keys)
(root/'source-keys.bin').write_bytes(raw)
print(json.dumps({'count':len(keys),'extra_keys':len(_EXTRA_CASES),'sha256':hashlib.sha256(raw).hexdigest()}))
"#])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("TMPDIR", root.join("tmp"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("SK_REGEX_ROOT", &root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .current_dir(&root)
        .output()
        .expect("pinned Source regex oracle");
    std::fs::write(root.join("source.stdout"), &result.stdout).expect("preserve Source stdout");
    std::fs::write(root.join("source.stderr"), &result.stderr).expect("preserve Source stderr");
    assert!(
        result.status.success(),
        "Source failed before Native: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let observed: serde_json::Value = serde_json::from_slice(&result.stdout).expect("Source JSON");
    assert_eq!(observed["count"], 0x110000);
    assert_eq!(observed["extra_keys"], 50);
    let expected = std::fs::read(root.join("source-keys.bin")).expect("Source keys");
    assert_eq!(expected.len(), 0x110000 * 4);
    // All Source execution/assertions precede the pure Native consumer.
    for (code, bytes) in expected.as_chunks::<4>().0.iter().enumerate() {
        let expected = u32::from_le_bytes(*bytes);
        assert_eq!(
            sk_kernel::python_text::regex_ignore_case_key(code as u32),
            expected,
            "U+{code:04X}"
        );
    }
}
