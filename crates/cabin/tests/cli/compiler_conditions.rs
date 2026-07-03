//! `cfg(cc = ...)` / `cfg(cxx = ...)` / `*_version` condition
//! behavior through the real pipeline: layers gated on the detected
//! host compiler land in `compile_commands.json`, `cabin metadata`,
//! and the build-config fingerprint; non-matching conditions stay
//! inert.
//!
//! Self-consistent on any host: each test first asks `cabin
//! metadata` for the detected cxx identity, then generates a
//! manifest gated on exactly that identity - so the same assertions
//! hold on GCC / Clang / AppleClang / MSVC hosts and on every leg of
//! the CI compiler matrix.  Defines are used as the gated payload
//! because they are dialect-portable (`-D` and `/D` both embed the
//! name).

use super::*;

const CONDITION_MAIN_CC: &str = "int main() { return 0; }\n";

/// `cabin()` with the ambient `CC` / `CXX` selection restored.  The
/// shared helper scrubs them for hermeticity; these tests are
/// *about* the host compiler, and the CI compiler matrix pins the
/// compiler under test through exactly these variables (the
/// documented opt-back-in pattern from the testing rules).
fn cabin_with_host_toolchain() -> Command {
    let mut cmd = cabin();
    for key in ["CC", "CXX"] {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    cmd
}

fn write_condition_package(dir: &TempDir, target_table: &str) {
    let manifest = format!(
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.demo]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\n{target_table}"
    );
    dir.child("cabin.toml").write_str(&manifest).unwrap();
    dir.child("src/main.cc")
        .write_str(CONDITION_MAIN_CC)
        .unwrap();
}

/// Detected `(kind, major)` for the C++ compiler, read through
/// `cabin metadata` - the same source of truth the conditions
/// evaluate against.
fn detected_cxx(dir: &TempDir) -> (String, u64) {
    let assertion = cabin_with_host_toolchain()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let identity = &value["toolchain"]["detected"]["cxx"]["identity"];
    let kind = identity["kind"]
        .as_str()
        .expect("metadata reports a detected cxx kind")
        .to_owned();
    let version = identity["version"]
        .as_str()
        .expect("metadata reports a detected cxx version");
    let major: u64 = version
        .split('.')
        .next()
        .unwrap()
        .parse()
        .expect("numeric detected major version");
    (kind, major)
}

fn read_compile_commands(dir: &TempDir) -> String {
    fs::read_to_string(
        dir.path()
            .join("build")
            .join("dev")
            .join("compile_commands.json"),
    )
    .unwrap()
}

#[test]
fn matching_compiler_condition_contributes_flags() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_condition_package(&dir, "");
    let (kind, major) = detected_cxx(&dir);
    write_condition_package(
        &dir,
        &format!(
            "\n[target.'cfg(all(cxx = \"{kind}\", cxx_version = \"={major}\"))'.profile]\ndefines = [\"CABIN_MATCHED_COMPILER\"]\n"
        ),
    );
    cabin_with_host_toolchain()
        .current_dir(dir.path())
        .arg("build")
        .assert()
        .success();
    let cc = read_compile_commands(&dir);
    assert!(
        cc.contains("CABIN_MATCHED_COMPILER"),
        "matching compiler condition must contribute its defines: {cc}"
    );
}

#[test]
fn impossible_version_condition_stays_inert() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_condition_package(&dir, "");
    let (kind, _major) = detected_cxx(&dir);
    write_condition_package(
        &dir,
        &format!(
            "\n[target.'cfg(all(cxx = \"{kind}\", cxx_version = \">=9999\"))'.profile]\ndefines = [\"CABIN_IMPOSSIBLE_VERSION\"]\n"
        ),
    );
    cabin_with_host_toolchain()
        .current_dir(dir.path())
        .arg("build")
        .assert()
        .success();
    let cc = read_compile_commands(&dir);
    assert!(
        !cc.contains("CABIN_IMPOSSIBLE_VERSION"),
        "an unsatisfied version requirement must not contribute flags: {cc}"
    );
}

#[test]
fn negated_family_condition_stays_inert() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_condition_package(&dir, "");
    let (kind, _major) = detected_cxx(&dir);
    write_condition_package(
        &dir,
        &format!(
            "\n[target.'cfg(not(cxx = \"{kind}\"))'.profile]\ndefines = [\"CABIN_NEGATED_FAMILY\"]\n"
        ),
    );
    cabin_with_host_toolchain()
        .current_dir(dir.path())
        .arg("build")
        .assert()
        .success();
    let cc = read_compile_commands(&dir);
    assert!(!cc.contains("CABIN_NEGATED_FAMILY"));
}

#[test]
fn metadata_reports_compiler_gated_flags_per_package() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_condition_package(&dir, "");
    let (kind, _major) = detected_cxx(&dir);
    write_condition_package(
        &dir,
        &format!(
            "\n[target.'cfg(cxx = \"{kind}\")'.profile]\ndefines = [\"CABIN_METADATA_GATED\"]\n"
        ),
    );
    let assertion = cabin_with_host_toolchain()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let defines = value["toolchain"]["build_flags_per_package"]["demo"]["defines"]
        .as_array()
        .expect("demo build flags present");
    assert!(
        defines.iter().any(|d| d == "CABIN_METADATA_GATED"),
        "gated define must appear in metadata flags: {defines:?}"
    );
}

#[test]
fn compiler_condition_flips_build_config_fingerprint() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_condition_package(&dir, "");
    let (kind, _major) = detected_cxx(&dir);

    let fingerprint_with = |condition: &str| -> String {
        write_condition_package(
            &dir,
            &format!("\n[target.'{condition}'.profile]\ndefines = [\"CABIN_FP_PROBE\"]\n"),
        );
        let assertion = cabin_with_host_toolchain()
            .current_dir(dir.path())
            .args(["explain", "build-config", "demo", "--format", "json"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        value["configuration"]["fingerprint"]
            .as_str()
            .expect("build-config explanation carries a fingerprint")
            .to_owned()
    };

    let matching = fingerprint_with(&format!("cfg(cxx = \"{kind}\")"));
    let inert = fingerprint_with(&format!(
        "cfg(all(cxx = \"{kind}\", cxx_version = \">=9999\"))"
    ));
    assert_ne!(
        matching, inert,
        "a flipped compiler condition must move the build fingerprint"
    );
}

#[test]
fn compiler_condition_on_dependency_table_is_rejected() {
    // Pure manifest validation: no toolchain required.
    let dir = TempDir::new().unwrap();
    write_condition_package(
        &dir,
        "\n[target.'cfg(cxx = \"clang\")'.dependencies]\nfmt = \"^10\"\n",
    );
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "may only gate a `.profile` flag table",
        ));
}

#[test]
fn unknown_compiler_family_value_is_rejected_at_parse_time() {
    let dir = TempDir::new().unwrap();
    write_condition_package(
        &dir,
        "\n[target.'cfg(cxx = \"clang++\")'.profile]\ncxxflags = [\"-stdlib=libc++\"]\n",
    );
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown compiler family"))
        .stderr(predicate::str::contains("clang++"));
}
