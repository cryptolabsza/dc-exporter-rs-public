const BUILD_WORKFLOW: &str = include_str!("../.github/workflows/build.yml");

#[test]
fn build_workflow_only_admits_trusted_branch_sources() {
    assert!(!BUILD_WORKFLOW.contains("pull_request:"));
    assert!(!BUILD_WORKFLOW.contains("tags:"));
    assert!(BUILD_WORKFLOW.contains("github.repository == 'cryptolabsza/dc-exporter-rs-public'"));
    assert!(BUILD_WORKFLOW.contains("github.ref == 'refs/heads/dev'"));
    assert!(BUILD_WORKFLOW.contains("github.ref == 'refs/heads/main'"));
}

#[test]
fn build_uses_dedicated_native_nonroot_pool_and_readonly_credentials() {
    assert!(BUILD_WORKFLOW.contains("group: dc-exporter-build"));
    assert!(BUILD_WORKFLOW.contains("labels: dc-exporter-build"));
    assert!(BUILD_WORKFLOW.contains("contents: read"));
    assert!(BUILD_WORKFLOW.contains("persist-credentials: false"));
    assert!(!BUILD_WORKFLOW.contains("contents: write"));
    for unsafe_fragment in ["container:", "apt-get", "sudo ", "curl ", "secrets.", "runs-on: [self-hosted, linux]", "release-dev:", "gh release"] {
        assert!(!BUILD_WORKFLOW.contains(unsafe_fragment), "{unsafe_fragment}");
    }
}

#[test]
fn native_job_has_timeout_concurrency_fresh_attempt_paths_and_no_gpu_smoke() {
    assert!(BUILD_WORKFLOW.contains("timeout-minutes:"));
    assert!(BUILD_WORKFLOW.contains("concurrency:"));
    assert!(BUILD_WORKFLOW.contains("github.run_attempt"));
    assert!(BUILD_WORKFLOW.contains("scripts/ci/build.sh"));
    let script = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/ci/build.sh")).unwrap();
    assert!(script.contains("cargo test --release --locked --all-targets -- --skip test_collector_creation --skip test_timeout_behavior"));
    assert_eq!(script.matches("--skip ").count(), 2);
    assert!(script.contains("--version"));
    assert!(script.contains("--help"));
    assert!(script.contains("--vram-backend"));
    assert!(!script.contains("--one-shot"));
    assert!(!script.contains("--ignored"));
    assert!(script.contains("preflight"));
    assert!(script.contains("git ls-files --error-unmatch Cargo.lock"));
    assert!(script.contains("test_public_export.py"));
    assert!(script.contains("mktemp -d"));
    assert!(script.contains("trap "));
}

#[test]
fn artifact_is_bound_to_source_run_attempt_and_manifest() {
    assert!(BUILD_WORKFLOW.contains("dc-exporter-${{ github.repository_id }}-${{ github.run_id }}-${{ github.run_attempt }}-${{ github.sha }}"));
    assert!(BUILD_WORKFLOW.contains("if-no-files-found: error"));
    assert!(BUILD_WORKFLOW.contains("retention-days: 7"));
    assert!(BUILD_WORKFLOW.contains("steps.build.outputs.artifact_dir"));
}
