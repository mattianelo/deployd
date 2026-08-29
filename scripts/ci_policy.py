#!/usr/bin/env python3
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
STABLE_TAG_RULE = "'$CI_COMMIT_TAG =~ /^v\\d+\\.\\d+\\.\\d+$/'"


class PolicyError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise PolicyError(message)


def shell_value(source: str, name: str) -> str:
    match = re.search(rf'^{re.escape(name)}="([^"]+)"$', source, re.MULTILINE)
    if match is None:
        raise PolicyError(f"missing shell pin: {name}")
    return match.group(1)


def validate_pipeline_gating(pipeline: str) -> None:
    workflow = pipeline.split("\nworkflow:\n", maxsplit=1)[1].split(
        "\nvariables:\n", maxsplit=1
    )[0]
    require(
        STABLE_TAG_RULE in workflow
        and '$CI_PIPELINE_SOURCE == "web"' in workflow
        and '$CI_PIPELINE_SOURCE == "schedule"' in workflow
        and "    - when: never" in workflow,
        "GitLab pipelines are not restricted to manual, scheduled, and release runs",
    )
    require(
        'CI_PIPELINE_SOURCE == "push"' not in workflow
        and 'CI_PIPELINE_SOURCE == "merge_request_event"' not in workflow,
        "GitLab creates automatic commit or merge-request pipelines",
    )
    shared_rules = pipeline.split("\n.shared-pipeline:\n", maxsplit=1)[1].split(
        "\n.rust-job:\n", maxsplit=1
    )[0]
    require(
        shared_rules.count('$CI_PIPELINE_SOURCE == "web"') == 1
        and "merge_request_event" not in shared_rules
        and 'CI_PIPELINE_SOURCE == "push"' not in shared_rules,
        "full GitLab validation is not restricted to manual preflights",
    )


def validate() -> None:
    with (ROOT / "rust-toolchain.toml").open("rb") as stream:
        rust_version = tomllib.load(stream)["toolchain"]["channel"]
    with (ROOT / "fossil.toml").open("rb") as stream:
        fossil = tomllib.load(stream)

    pins_source = (ROOT / "packaging" / "appimage" / "mcp-versions.sh").read_text()
    fossil_version = shell_value(pins_source, "FOSSIL_MCP_VERSION")
    rust_analyzer_version = shell_value(pins_source, "RUST_ANALYZER_MCP_VERSION")
    for name in (
        "FOSSIL_MCP_SHA256",
        "RUST_ANALYZER_MCP_SHA256",
        "RUST_ANALYZER_MCP_LOCK_SHA256",
    ):
        require(
            re.fullmatch(r"[0-9a-f]{64}", shell_value(pins_source, name)) is not None,
            f"{name} is not a SHA-256 value",
        )

    pipeline = (ROOT / ".gitlab-ci.yml").read_text()
    validate_pipeline_gating(pipeline)
    revision_match = re.search(
        r'^\s*BUILD_ENV_IMAGE_REVISION: "(v[1-9][0-9]*)"$', pipeline, re.MULTILINE
    )
    require(revision_match is not None, "build image revision is missing or invalid")
    expected_tag = f"rust-{rust_version}-fossil-{fossil_version}-{revision_match.group(1)}"
    require(
        pipeline.index("  - build-env") < pipeline.index("  - validate"),
        "build image bootstrap must run before jobs that consume the image",
    )
    require(
        f'BUILD_ENV_IMAGE_TAG: "{expected_tag}"' in pipeline,
        f"build image tag must be {expected_tag}",
    )
    require(
        f'BUILD_ENV_PUBLISH_TAG: "{expected_tag}"' in pipeline,
        f"default build image publish tag must be {expected_tag}",
    )
    require("image: $BUILD_ENV_IMAGE" in pipeline, "CI jobs do not use the pinned image variable")
    require(
        "image: $CI_REGISTRY_IMAGE/build-env:latest" not in pipeline,
        "a CI job consumes the mutable build image alias",
    )
    require(
        re.search(r"^\s+HOME:", pipeline, re.MULTILINE) is None,
        "job-level HOME leaks into the GitLab checkout helper",
    )
    require(
        "- export HOME=/home/ubuntu" in pipeline,
        "non-root HOME is not established after checkout",
    )
    require(
        re.search(r"ci-ownership\.sh.*CARGO_(?:HOME|TARGET_DIR)", pipeline) is None,
        "CI applies ownership policy to runner-managed cache roots",
    )
    build_env_job = pipeline.split("\nbuild-env-image:\n", maxsplit=1)[1].split(
        "\nbuild-appimage:\n", maxsplit=1
    )[0]
    require(
        '$CI_PIPELINE_SOURCE == "web"' in build_env_job
        and 'CI_PIPELINE_SOURCE == "push"' not in build_env_job
        and "when: manual" not in build_env_job,
        "manual preflights do not automatically verify the immutable build image",
    )
    require(
        'git fetch origin "$CI_DEFAULT_BRANCH"' in pipeline
        and 'git merge-base "$CI_COMMIT_SHA" FETCH_HEAD' in pipeline
        and './scripts/ci-fossil.sh diff .ci-artifacts/fossil "$base_sha"' in pipeline,
        "manual preflights do not run the Fossil diff gate against the default branch",
    )
    fossil_ci = (ROOT / "scripts" / "ci-fossil.sh").read_text()
    require(
        "--max-dead-code 0" in fossil_ci,
        "Fossil diff gate does not block new dead code",
    )
    require(
        "--min-confidence high" in fossil_ci,
        "Fossil diff gate does not use the calibrated confidence threshold",
    )
    require(
        "--max-scaffolding 0" in fossil_ci and "--fail-on-scaffolding" in fossil_ci,
        "Fossil diff gate does not block new scaffolding",
    )
    require(
        "--max-clones 4294967295" in fossil_ci,
        "Fossil diff gate blocks clone growth before ratchet approval",
    )
    fossil_diff_job = pipeline.split("\nfossil-diff:\n", maxsplit=1)[1].split(
        "\ndependency-audit:\n", maxsplit=1
    )[0]
    require("allow_failure: false" in fossil_diff_job, "Fossil diff gate is non-blocking")
    require(
        ".ci-artifacts/fossil/fossil-diff.json" in fossil_diff_job,
        "Fossil diff report is not retained for clone-growth review",
    )
    file_size_source = (ROOT / "scripts" / "ci_file_size.py").read_text()
    require(
        "MAX_RUST_LINES = 1_500" in file_size_source,
        "Rust file-size policy does not enforce the 1,500-line threshold",
    )
    require(
        "python3 scripts/test_ci_file_size.py" in pipeline
        and "python3 scripts/ci_file_size.py" in pipeline,
        "CI does not test and enforce the Rust file-size policy",
    )
    require(
        pipeline.count(STABLE_TAG_RULE) == 5,
        "GitLab release jobs do not share the exact stable SemVer tag rule",
    )
    require(
        'python3 scripts/release_metadata.py --tag "$CI_COMMIT_TAG"' in pipeline,
        "GitLab does not validate tagged release metadata",
    )
    require(
        "- job: release-metadata" in pipeline,
        "AppImage builds do not depend on release metadata validation",
    )
    build_appimage_job = pipeline.split("\nbuild-appimage:\n", maxsplit=1)[1].split(
        "\nupload-nexus:\n", maxsplit=1
    )[0]
    require(
        '$CI_PIPELINE_SOURCE == "web"' in build_appimage_job
        and "when: manual" not in build_appimage_job,
        "manual preflights do not automatically build the AppImage",
    )
    require(
        "python3 scripts/test_release_metadata.py" in pipeline,
        "CI does not run release metadata regression tests",
    )
    pages_job = pipeline.split("\npages:\n", maxsplit=1)[1]
    require(
        "when: manual" in pages_job and "allow_failure: true" in pages_job,
        "manual preflights are blocked by optional Pages publication",
    )
    for secret_name in ("SNAPCRAFT_STORE_CREDENTIALS", "SNAP_STORE_LOGIN"):
        require(
            secret_name not in pipeline,
            f"GitLab must not receive the GitHub Snap secret: {secret_name}",
        )
    for local_source in ("AGENTS.md", ".agents/", ".codex/", ".plans/", "docs/"):
        require(local_source not in pipeline, f"CI references local-only input: {local_source}")

    github_workflow = (ROOT / ".github" / "workflows" / "release-snap.yml").read_text()
    for action in (
        "actions/checkout@11d5960a326750d5838078e36cf38b85af677262",
        "canonical/action-build@6d723b848ffb875da54b8fa7a8fe060e6c3f55a7",
        "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02",
        "canonical/action-publish@895633656038d69bfc68efdccc0964053d60ad2f",
    ):
        require(action in github_workflow, f"GitHub workflow action pin drifted: {action}")
    require(
        '      - "v[0-9]+.[0-9]+.[0-9]+"' in github_workflow
        and "  workflow_dispatch:" in github_workflow,
        "GitHub workflow does not separate stable tags from manual preflights",
    )
    require(
        "permissions:\n  contents: read" in github_workflow,
        "GitHub workflow permissions are not read-only",
    )
    require(
        "group: snap-store-production" in github_workflow
        and "cancel-in-progress: false" in github_workflow,
        "Snap Store publication concurrency is not protected",
    )
    require(
        github_workflow.count("runs-on: ubuntu-24.04") == 3
        and "timeout-minutes: 5" in github_workflow
        and github_workflow.count("timeout-minutes: 90") == 2,
        "GitHub Snap jobs do not use the approved runner and timeouts",
    )
    require(
        github_workflow.count("retention-days: 90") == 2,
        "GitHub Snap artifacts are not retained for 90 days",
    )
    require(
        "if: github.event_name == 'workflow_dispatch'" in github_workflow
        and "if: needs.classify.outputs.release == 'true'" in github_workflow,
        "manual builds and stable publication are not isolated",
    )
    preflight_job = github_workflow.split("\n  preflight:\n", maxsplit=1)[1].split(
        "\n  release:\n", maxsplit=1
    )[0]
    require(
        "action-publish" not in preflight_job
        and "SNAPCRAFT_STORE_CREDENTIALS" not in preflight_job
        and "SNAP_STORE_LOGIN" not in preflight_job,
        "manual Snap preflight can access publication credentials",
    )
    require(
        "environment: snap-store-production" in github_workflow
        and "SNAPCRAFT_STORE_CREDENTIALS: ${{ secrets.SNAP_STORE_LOGIN }}" in github_workflow
        and "release: stable" in github_workflow,
        "Snap publication is not restricted to the stable production environment",
    )

    require(fossil["dead_code"]["min_confidence"] == "high", "Fossil dead code must be high confidence")
    require(fossil["dead_code"]["include_tests"] is False, "Fossil must exclude test-only dead code")
    require(fossil["ci"]["fail_on_scaffolding"] is False, "observation mode must not block on scaffolding")

    sandbox = (ROOT / "scripts" / "mcp" / "fossil-sandbox.sh").read_text()
    require('/workspace/fossil.toml' in sandbox, "local Fossil MCP does not use root fossil.toml")
    require("docs/phase-2/fossil.toml" not in sandbox, "local Fossil MCP uses obsolete config")

    snap = (ROOT / "snap" / "snapcraft.yaml").read_text()
    require("confinement: strict" in snap, "Snap confinement is not strict")
    require(
        "cargo build --locked --release --features loot,libarchive-fallback" in snap,
        "Snap feature composition drifted",
    )
    require(
        "  rust-deps:\n" in snap and "    plugin: nil\n" in snap,
        "Snap does not provide the Rust plugin's required dependency part",
    )
    require(
        "    after: [rust-deps]\n" in snap and "    rust-channel: 'none'\n" in snap,
        "Snap Rust part does not consume the dedicated toolchain part",
    )
    require(
        "      - rustup\n" in snap
        and "      rustup set profile minimal\n" in snap
        and f"      rustup default {rust_version}\n" in snap
        and f"      - RUSTUP_TOOLCHAIN: '{rust_version}'\n" in snap,
        "hosted Snap builds do not provision the repository Rust toolchain",
    )
    require(
        "rustup toolchain install" not in snap,
        "Ubuntu's packaged rustup must use the default-toolchain flow",
    )
    require(
        "build-snaps: [rustup]" not in snap,
        "Snap build uses the incompatible core26-based rustup snap",
    )
    require("      - home" in snap and "      - network" in snap, "Snap required plugs drifted")

    dockerfile = (ROOT / "packaging" / "appimage" / "Dockerfile").read_text()
    require("USER ubuntu" in dockerfile, "build image does not select the non-root user")
    require(
        "install -d -o ubuntu -g ubuntu /home/ubuntu /build" in dockerfile,
        "build image does not create the non-root HOME explicitly",
    )
    require("rust-analyzer rust-src" in dockerfile, "build image omits rust-src")
    require("install-fossil.sh" in dockerfile, "build image omits Fossil")
    require("install-rust-analyzer-mcp.sh" in dockerfile, "build image omits rust-analyzer MCP")

    executable_files = (
        "check.sh",
        "scripts/rust-command.sh",
        "scripts/ci-environment.sh",
        "scripts/ci-ownership.sh",
        "scripts/ci-ownership-smoke.sh",
        "scripts/ci-fossil.sh",
        "scripts/ci_instruction_drift.py",
        "scripts/ci-freshness.sh",
        "scripts/ci-inventory-tests.sh",
        "scripts/ci-mcp-smoke.sh",
        "scripts/ci-rust-analyzer-mcp.sh",
        "scripts/ci-fossil-mcp.sh",
        "scripts/test_ci_environment.sh",
        "packaging/appimage/install-fossil.sh",
        "packaging/appimage/install-rust-analyzer-mcp.sh",
        "packaging/appimage/provision-mcp.sh",
    )
    for relative in executable_files:
        require((ROOT / relative).stat().st_mode & 0o111 != 0, f"script is not executable: {relative}")

    print(
        "policy validation passed: "
        f"Rust {rust_version}, Fossil {fossil_version}, rust-analyzer MCP {rust_analyzer_version}"
    )


def main() -> int:
    try:
        validate()
    except (KeyError, OSError, tomllib.TOMLDecodeError, PolicyError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
