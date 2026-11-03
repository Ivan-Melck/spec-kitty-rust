//! `4.0.0rc8_retire_stale_mission_template_forks`: repair old template forks.
//!
//! Two shipped migrations copied older documentation/research template forks
//! into projects. A later runtime sweep could move those forks into
//! `.kittify/overrides/missions/<type>/templates/`, where they outrank the
//! current pack copy. This migration repairs what those writers left:
//!
//! - `.kittify/overrides/missions/<type>/templates/`: a proven stale copy is
//!   removed, so the canonical pack tier resolves again;
//! - `.kittify/missions/<type>/templates/`: a proven stale copy is replaced
//!   with the pack copy, which also keeps the old `0.9.2` migration quiet.
//!
//! Proof is a SHA-256 of the file's bytes (CRLF folded to LF) against every
//! version the fork ever shipped. A file whose hash is not in the set is the
//! operator's edit: it is kept and named in a warning. A file already equal to
//! the pack copy is left alone silently. Directories are never removed, and a
//! symlinked path is refused.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;

const MISSION_TYPES: [&str; 2] = ["documentation", "research"];
const TEMPLATES: &str = "templates";
const LEGACY_TREE: [&str; 2] = [".kittify", "missions"];
const OVERRIDE_TREE: [&str; 3] = [".kittify", "overrides", "missions"];

const KNOWN_FORK_SHA256: [(&str, &[&str]); 15] = [
    (
        "documentation/divio/explanation-template.md",
        &[
            "62b594f31bee903b722a236056731bd18a88d9e8000118e87f756ed3015767e7",
            "fe810f3091a941c11f32df0ff0ede27446607468b3e9e373fac03b7b4af45c7a",
        ],
    ),
    (
        "documentation/divio/howto-template.md",
        &[
            "3e28e151173b325fff82a74cc9776d396d9b5fed09877561b2a6725ae1d5554e",
            "bad57e533f97024b369970e3712cf6fff6d57bfba75578d5bf70fcef9de44671",
        ],
    ),
    (
        "documentation/divio/reference-template.md",
        &[
            "33ad6e668847415a3813e145c8ffccb615e3e54effb67ba5d86e337154f2eb06",
            "883d08403f7915bf37633e7d25be8b47ff7225fa5354e9ee176c8e32fd39adf6",
        ],
    ),
    (
        "documentation/divio/tutorial-template.md",
        &[
            "64ad64956ddd2bb7b5eaa9e19682ca543445a098825789d17c1254d9ecfdb662",
            "e43f2cf966f9147b8e0d4d8729a944b398b3e50002ebede90dbb4789c8db4056",
        ],
    ),
    (
        "documentation/plan-template.md",
        &[
            "02088b60642aae5354f98b38ea12759189b7378ca1410ab976909b555553482c",
            "30dd3ad12352b4526684442a75a3645a44d918cff2ae8412707a9458bf035db3",
            "3109c5bc2ed07970ac8c644d755a12e37b4b8e6ef9ee107b69e36571b584ac75",
            "402fec9c26e87df39d78264915dda5db6a60e3d529390ddc989a3469daf6ec88",
            "8dc2d6dd32045dd2bc8c56eb4982cb367925352a6893d0c045b37cea13457d5e",
        ],
    ),
    (
        "documentation/release-template.md",
        &[
            "856b61fe0e7735a3c7103e1bcf242f5064326ed971edbb5aa3b5f6c4f04cf437",
            "d452ca449cb04d7678e1524773fbfb2d68d85f561a6ccbf2138ca025443ae73d",
        ],
    ),
    (
        "documentation/spec-template.md",
        &[
            "98cebd2cfab3d35d5bcb3bcd34083fe8e02be52e08fd3543e67fad5d0e19083c",
            "ba82d8cc3c38e50d6fe0e1abb78b1b32ef0e62c3b61af2ba9fc15d52db1b1132",
            "cfbf3d095fd3225d0f426e25582ffa60058fe1a63e57f34581ef3cb507cff37a",
            "d43fff182ae5ecbcdbab743734824c0795aa5af9488b4ee4a136626fbb591961",
        ],
    ),
    (
        "documentation/task-prompt-template.md",
        &[
            "0e05bb4136f36d0f91b992853bd4e47b5271894650efef94f5faa2933deb7eba",
            "16a6b08b4a1bf77ee73c58c6cce80ef9219e1cefc814d514e868ed06e5391cde",
            "203e3ec2ae63d3760a2a67ed280b4d9a23d2b2018c92852ed2ce46a4fd3a7db8",
            "2e17673d167bf13d6799602eca403f90856f913c6ed2f017690e07d81356e272",
            "448b57f73e4fd36bce06bee4a05ad398f747fe53d3e92512d7806cf00959a3a1",
            "6ff27f35f0b5cc1a8b787f73d2f35142d7a36913cf52ebdc7ce42d81657a9548",
            "9820e114509156e1495f5f5c721b373c9950c72d66250c17907d342d5bfce153",
            "98d3f8e0e3628592a7dd3b4645c3bbe7ad7f13396746a82fc1033e095086b0c7",
            "a24794f28e2dd7af0f0310522e19800136654b91082a71071aea624a6cd7390c",
            "c82e2c1f33cb363d31ea5728b790da8b73a6ed92b38b4d768b3afef37d2d52b1",
            "c8b0c33a02b4f37e27576757e8e49fa7a4b383819338f56379fd73010e3779e3",
            "d764d53db598d8c6f2992c422c77c3ad2df417dcc37d323a041f0027403ce410",
            "dffb50232bb076034beea174d588c581668b9f9c6cd3cda6903f95b75a2525ea",
            "e17fc3d339f8c8708af81cb9e94eb7e32d7ccf68c4a0b0599044c31678659d38",
            "e535e6269a2d58f98f868f55368ae37385686de84d0021ce205b57c02d66dcde",
            "eb46020554c7840845e2233c8f2c7c4ebdce009db2d2fed095026b29a1502c6e",
        ],
    ),
    (
        "documentation/tasks-template.md",
        &[
            "09a68ff734f0158aaa8a9832f775fc53932a171f10656c74f0d186e6e4e18ab1",
            "9735d55c770ef79c073614eaf5dee0f2011ccdc5cb1616e76ad937b65798efbd",
            "c593367877686a5e312ca05ad550dc52b33f8407433df52b81415fef5cbc9ce2",
            "d993e239b42ee75f86d5e4fdac26a0c06e1e84325478ec168637b166a82f97ea",
            "eaa08544c484100e6f4448370ad80face005e644ffb0754f09d10af7c1678cae",
        ],
    ),
    (
        "research/data-model-template.md",
        &[
            "d9115c00810c62874f1b456e3811827a14a94e2d8f770f30906ea41fc697d0c5",
            "fdcb09e829e2e967c34c20c4ae722b9ca00636c3be41c830293152e667d843a6",
        ],
    ),
    (
        "research/plan-template.md",
        &[
            "15f0c4bdd8b1cbccf9f032e849491925e9f50acdc8b0cd68602baf472377fa2b",
            "3850d398e77d85550218d07c7dfc02d2365ee2031fe276ed44245d557719bb4c",
            "3c6bc2be02d879b3397e075a7d3770bcca23fcdf15e54cd77d08ae5176aec857",
            "e8ce4ea2b3158c9daf9f4cfbf5d65377466a6972a51e9782fde86f8becef1a34",
        ],
    ),
    (
        "research/research-template.md",
        &[
            "3104d75be3c6112572c332c1362bfe8a8370b77a60da85304ec0e66ca384edbd",
            "582fe34c36d8cd779c07670a6b580ee3b9a10ba88e0ff60437026d9e8dfaaefc",
            "cd0deebe9e0fc0a4132416f59e95f530786fde482af008d7c1b5b3a35e18613a",
        ],
    ),
    (
        "research/spec-template.md",
        &[
            "16c845b94c48c67506e0a729ac5608357e971cb4078476e6211b862ba8168eae",
            "d9489b6847736a80b375cec09b708664e4c1229a880c8e346802f769e89b6bce",
            "f97386004b06661d8b1d6a20ab0040d3b27f109187e828158be78ba003f79b72",
        ],
    ),
    (
        "research/task-prompt-template.md",
        &[
            "16516fec8152f991be0d37b1b072bf38ab6849c87edc5b69edacb90c44b9276a",
            "171b795651f75fcca4a08a62e58cb1fc8efd5948e4368c690b870f389924d39b",
            "2d911d14001e1646729c0c88fa452e6459a2673aa6b2e03b4d14d7164cb09d99",
            "455a90e561f6020608395765f8bc851d4c7bff6350038efe0b549f5d953af9e6",
            "51e1745f1e8541ebc58cde8a9317fb0305c3da4261f4be267d5cd76068a7f951",
            "59accb807169400538f9a6c193df8799c09c8d36985b9597a42341005daf7830",
            "71ef1e31437fc2f6fa02709134026b8e6e63e8ed5de78bfb95f7c4e323ebf53e",
            "797c66a88747731291a5a37d38d03f8f65f950bd4f33c3bbf1e9134ae43ec161",
            "7dc1e8357b532b13fbd4b642ee46233abe35f14ccf2d0988bf6a774f2ee539a4",
            "a427393ed8dea5fcfd06918dbe45801456e84c084777c4f1547eb08211e64a21",
            "b60c2df2da0a74c51196209263125550140364194da82cac5662db2044862e5a",
            "e75ed3a1ad46528796deccf100b2bd59334471ec4ac456fcbe323ee1d4734051",
            "edc13271ae7d8dfc0d2445dc036e761ae82469712ae670a1e727f7202d1a81c2",
        ],
    ),
    (
        "research/tasks-template.md",
        &[
            "18499583e8724f3a9585bfdbdc7f40d9672f9a45ebb8448bc8e9c26c1db7c40f",
            "237b0954d3a87bc7f073c9a0037f231a068cec7db3e493f08c19de3e84c5689a",
            "4d22bc713080654d4d956809ce36d78d33c5b30557a94e1702acea6b92194cf4",
            "8fd0354620d3dc8ba6e2c78d3ac076adcb3e88d1440bad93eac44c65891c5e8e",
        ],
    ),
];

fn known_hashes(key: &str) -> Option<&'static [&'static str]> {
    KNOWN_FORK_SHA256
        .iter()
        .find_map(|(candidate, hashes)| (*candidate == key).then_some(*hashes))
}

fn fingerprint(content: &[u8]) -> String {
    // The source folds CRLF to LF before hashing, so a Windows checkout is not
    // an edit. Fold the pair as a unit: drop the CR, keep the LF.
    let mut folded = Vec::with_capacity(content.len());
    let mut index = 0;
    while index < content.len() {
        if content[index] == b'\r' && content.get(index + 1) == Some(&b'\n') {
            folded.push(b'\n');
            index += 2;
        } else {
            folded.push(content[index]);
            index += 1;
        }
    }
    format!("{:x}", Sha256::digest(folded))
}

fn relative(path: &Path, project_root: &Path) -> String {
    path.strip_prefix(project_root)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

/// Warning for a fork-named file that differs from every shipped version.
pub fn kept_warning(relative_path: &str) -> String {
    format!(
        "Kept {relative_path}: it differs from every version Spec Kitty shipped, so it is treated as your edit. Delete it to use the template Spec Kitty ships now."
    )
}

/// Warning for a symlinked or non-directory path on the scanned tree.
pub fn skipped_tree_warning(relative_path: &str) -> String {
    format!(
        "Stale mission template cleanup skipped {relative_path}: it is a symlink or not a directory; nothing there was changed"
    )
}

#[derive(Clone, Debug)]
struct Candidate {
    path: PathBuf,
    key: String,
    in_overrides: bool,
}

fn real_dir(
    project_root: &Path,
    parts: &[&str],
) -> Result<(Option<PathBuf>, Option<String>), InstallError> {
    let mut current = project_root.to_path_buf();
    for part in parts {
        current.push(part);
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((None, None));
            }
            Err(source) => return Err(InstallError::io("inspect template tree", &current, source)),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Ok((None, Some(relative(&current, project_root))));
        }
    }
    Ok((Some(current), None))
}

fn scan(project_root: &Path) -> Result<(Vec<Candidate>, Vec<String>), InstallError> {
    let mut candidates = Vec::new();
    let mut refused = Vec::new();
    for (tree, in_overrides) in [(&LEGACY_TREE[..], false), (&OVERRIDE_TREE[..], true)] {
        for mission_type in MISSION_TYPES {
            for (key, _) in KNOWN_FORK_SHA256 {
                if !key.starts_with(&format!("{mission_type}/")) {
                    continue;
                }
                let name_parts: Vec<&str> = key.split('/').skip(1).collect();
                let (last, parent_parts) = name_parts.split_last().expect("key has a name");
                let mut parts: Vec<&str> = tree.to_vec();
                parts.push(mission_type);
                parts.push(TEMPLATES);
                parts.extend(parent_parts.iter().copied());
                let (parent, refused_rel) = real_dir(project_root, &parts)?;
                if let Some(refused_rel) = refused_rel
                    && !refused.contains(&refused_rel)
                {
                    refused.push(refused_rel);
                }
                let Some(parent) = parent else {
                    continue;
                };
                let path = parent.join(last);
                if path.exists() || path.symlink_metadata().is_ok() {
                    candidates.push(Candidate {
                        path,
                        key: (*key).to_string(),
                        in_overrides,
                    });
                }
            }
        }
    }
    Ok((candidates, refused))
}

fn pack_copy(pack_root: Option<&Path>, key: &str) -> Option<PathBuf> {
    let pack_root = pack_root?;
    let (mission_type, rest) = key.split_once('/')?;
    let path = pack_root.join(mission_type).join(TEMPLATES).join(rest);
    path.is_file().then_some(path)
}

fn is_current(candidate: &Candidate, pack: Option<&Path>) -> bool {
    let Some(pack) = pack else {
        return false;
    };
    if candidate
        .path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        || !candidate.path.is_file()
    {
        return false;
    }
    match (std::fs::read(&candidate.path), std::fs::read(pack)) {
        (Ok(left), Ok(right)) => fingerprint(&left) == fingerprint(&right),
        _ => false,
    }
}

fn pending(
    candidates: &[Candidate],
    pack_root: Option<&Path>,
) -> Vec<(Candidate, Option<PathBuf>)> {
    candidates
        .iter()
        .filter_map(|candidate| {
            let pack = pack_copy(pack_root, &candidate.key);
            (!is_current(candidate, pack.as_deref())).then(|| (candidate.clone(), pack))
        })
        .collect()
}

fn proved(candidate: &Candidate) -> bool {
    if candidate
        .path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        || !candidate.path.is_file()
    {
        return false;
    }
    let Some(known) = known_hashes(&candidate.key) else {
        return false;
    };
    match std::fs::read(&candidate.path) {
        Ok(bytes) => known.contains(&fingerprint(&bytes).as_str()),
        Err(_) => false,
    }
}

fn repair(
    project_root: &Path,
    candidate: &Candidate,
    pack: Option<&Path>,
    dry_run: bool,
) -> Result<String, InstallError> {
    let rel = relative(&candidate.path, project_root);
    if candidate.in_overrides || pack.is_none() {
        if !proved(candidate) {
            return Ok(kept_warning(&rel));
        }
        if dry_run {
            return Ok(format!(
                "Would remove package-owned {rel} (proof: canonical_content)"
            ));
        }
        std::fs::remove_file(&candidate.path)
            .map_err(|source| InstallError::io("remove stale template", &candidate.path, source))?;
        return Ok(format!(
            "Removed package-owned {rel} (proof: canonical_content)"
        ));
    }
    if !proved(candidate) {
        return Ok(kept_warning(&rel));
    }
    if dry_run {
        return Ok(format!(
            "Would replace stale {rel} with the shipped template"
        ));
    }
    let pack = pack.expect("pack is Some in this branch");
    let bytes = std::fs::read(pack)
        .map_err(|source| InstallError::io("read shipped template", pack, source))?;
    sk_kernel::atomic_write(&candidate.path, &bytes).map_err(|error| {
        InstallError::io(
            "replace stale template",
            &candidate.path,
            std::io::Error::other(error),
        )
    })?;
    Ok(format!("Replaced stale {rel} with the shipped template"))
}

fn apply_inner(
    project_root: &Path,
    pack_root: Option<&Path>,
    dry_run: bool,
) -> Result<MigrationOutcome, InstallError> {
    let (candidates, refused) = scan(project_root)?;
    let mut changes = Vec::new();
    let mut warnings: Vec<String> = refused
        .iter()
        .map(|rel| skipped_tree_warning(rel))
        .collect();
    let mut errors = Vec::new();
    for (candidate, pack) in pending(&candidates, pack_root) {
        match repair(project_root, &candidate, pack.as_deref(), dry_run) {
            Ok(outcome) if outcome.starts_with("Kept ") => warnings.push(outcome),
            Ok(outcome) => changes.push(outcome),
            Err(error) => errors.push(format!(
                "Could not repair {}: {error}",
                relative(&candidate.path, project_root)
            )),
        }
    }
    if errors.is_empty() {
        Ok(MigrationOutcome {
            changed: !changes.is_empty(),
            changes,
            warnings,
        })
    } else {
        Err(InstallError::UpgradeRefused {
            detail: errors.join("; "),
        })
    }
}

fn shipped_asset_root() -> Result<PathBuf, InstallError> {
    sk_kernel::get_package_asset_root().map_err(|error| InstallError::TemplateTreeMissing {
        path: PathBuf::from("<package mission assets>"),
        detail: error.to_string(),
    })
}

/// Detection: a candidate differs from its pack copy, or a scanned path is refused.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    let (candidates, refused) = scan(project_root)?;
    if !refused.is_empty() {
        return Ok(true);
    }
    let pack_root = shipped_asset_root();
    match pack_root {
        Ok(pack_root) => Ok(!pending(&candidates, Some(&pack_root)).is_empty()),
        Err(_) => Ok(!candidates.is_empty()),
    }
}

/// `can_apply`: the shipped mission templates must be locatable.
pub fn can_apply(_project_root: &Path) -> Result<Option<String>, InstallError> {
    match shipped_asset_root() {
        Ok(_) => Ok(None),
        Err(error) => Ok(Some(format!(
            "Could not locate the shipped mission templates: {error}"
        ))),
    }
}

/// Apply using the canonical shipped mission assets.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let pack_root = shipped_asset_root()?;
    apply_inner(project_root, Some(&pack_root), false)
}

/// Apply against an explicit mission-assets root (tests and dependency-injected callers).
pub fn apply_with_pack_root(
    project_root: &Path,
    pack_root: &Path,
    dry_run: bool,
) -> Result<MigrationOutcome, InstallError> {
    apply_inner(project_root, Some(pack_root), dry_run)
}

/// Detection against an explicit mission-assets root.
pub fn detect_with_pack_root(
    project_root: &Path,
    pack_root: Option<&Path>,
) -> Result<bool, InstallError> {
    let (candidates, refused) = scan(project_root)?;
    if !refused.is_empty() {
        return Ok(true);
    }
    Ok(!pending(&candidates, pack_root).is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_fingerprint_is_a_known_fork_hash() {
        let bytes = include_bytes!(
            "../../tests/fixtures/migrations/forks/research/task-prompt-template.md"
        );
        let hash = fingerprint(bytes);
        assert_eq!(
            hash,
            "455a90e561f6020608395765f8bc851d4c7bff6350038efe0b549f5d953af9e6"
        );
        let crlf: Vec<u8> = bytes
            .iter()
            .flat_map(|byte| {
                if *byte == b'\n' {
                    vec![b'\r', b'\n']
                } else {
                    vec![*byte]
                }
            })
            .collect();
        assert_eq!(fingerprint(&crlf), hash);
    }
}
