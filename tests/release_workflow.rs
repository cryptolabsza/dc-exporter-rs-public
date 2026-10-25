// TRACEWEAVER: file-role=public-pool-workflow-contract; req=REQ-PUB-004; trace=TRACE-PUB-004; ver=VER-PUB-004
// TRACEWEAVER: verifies=VER-PUB-004; req=REQ-PUB-004; trace=TRACE-PUB-004
const CALLER: &str = include_str!("../.github/workflows/build.yml");
const RECIPE: &str = include_str!("../.github/workflows/ci-pool-build-recipe.yml");
const POOL_BUILD: &str = include_str!("../scripts/ci/pool-build.sh");
const NATIVE_BUILD: &str = include_str!("../scripts/ci/build.sh");

#[test]
fn build_workflow_only_admits_trusted_branch_sources() {
    for workflow in [CALLER, RECIPE] {
        for forbidden in ["pull_request", "tags:", "secrets: inherit", "contents: write"] {
            assert!(!workflow.contains(forbidden), "{forbidden}");
        }
        assert!(workflow.contains("github.repository == 'cryptolabsza/dc-exporter-rs-public'"));
        assert!(workflow.contains("github.repository_id == '1416116330'"));
        assert!(workflow.contains("github.ref == 'refs/heads/dev'"));
        assert!(workflow.contains("github.ref == 'refs/heads/main'"));
        assert!(workflow.contains("github.ref_protected"));
    }
}

#[test]
fn source_caller_uses_immutable_qualified_recipe() {
    let prefix = "uses: cryptolabsza/dc-exporter-rs-public/.github/workflows/ci-pool-build-recipe.yml@";
    let pin = CALLER.lines().find_map(|line| line.trim().strip_prefix(prefix)).unwrap();
    assert_eq!(pin.len(), 40);
    assert!(pin.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(!CALLER.contains("runs-on:"));
    assert!(CALLER.contains("concurrency:"));
    assert!(CALLER.contains("cancel-in-progress: false"));
}

#[test]
fn build_uses_visibility_selected_pool_and_readonly_credentials() {
    for expected in ["ci-build-ephemeral", "ci-build-public-ephemeral", "ci-linux-cpu-v1-r1416116330",
        "toJSON(github.event.repository.private)", "github.event.repository.id == 1416116330",
        "contents: read", "persist-credentials: false", "timeout-minutes: 120"] {
        assert!(RECIPE.contains(expected), "{expected}");
    }
    for unsafe_fragment in ["apt-get", "sudo ", "secrets.", "runs-on: [self-hosted, linux]", "release-dev:", "gh release"] {
        assert!(!RECIPE.contains(unsafe_fragment), "{unsafe_fragment}");
    }
    for expected in ["--cap-drop=ALL", "--security-opt=no-new-privileges", "--cpus=2", "--memory=3g", "--pids-limit=256", "--user", "@sha256:"] {
        assert!(POOL_BUILD.contains(expected), "{expected}");
    }
}

#[test]
fn shared_build_retains_test_filters_and_hardware_free_packaging() {
    for script in [POOL_BUILD, NATIVE_BUILD] {
        assert!(script.contains("cargo test --release --locked --all-targets -- --skip test_collector_creation --skip test_timeout_behavior"));
        assert_eq!(script.matches("--skip ").count(), 2);
        for expected in ["--version", "--help", "--vram-backend", "git ls-files --error-unmatch Cargo.lock", "test_public_export.py", "mktemp -d", "trap "] {
            assert!(script.contains(expected), "{expected}");
        }
        assert!(!script.contains("--one-shot"));
        assert!(!script.contains("--ignored"));
    }
    assert!(NATIVE_BUILD.contains("preflight"));
    assert!(POOL_BUILD.contains("grep -q -- --tui"));
    assert!(POOL_BUILD.find("scripts/ci/rusd_abi.py").unwrap() < POOL_BUILD.find("cargo metadata").unwrap());
    assert!(POOL_BUILD.contains("cp dist/THIRD-PARTY-NOTICES.md"));
    assert!(POOL_BUILD.contains("usr/share/doc/dc-exporter-rs/copyright"));
}

#[test]
fn artifact_is_bound_to_source_run_attempt_and_manifest() {
    assert!(RECIPE.contains("dc-exporter-${{ github.repository_id }}-${{ github.run_id }}-${{ github.run_attempt }}-${{ github.sha }}"));
    assert!(RECIPE.contains("if-no-files-found: error"));
    assert!(RECIPE.contains("retention-days: 7"));
    assert!(RECIPE.contains("path: dist/*"));
    assert!(RECIPE.contains("path: build-evidence/*"));
    assert!(POOL_BUILD.contains("scripts/ci/pool_manifest.py"));
    assert!(POOL_BUILD.contains("scripts/ci/notice_inventory.py"));
}
