#!/usr/bin/env python3
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


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
    require(
        'CI_PIPELINE_SOURCE == "push"' in pipeline and "allow_failure: false" in pipeline,
        "protected default-branch pushes cannot bootstrap a changed build image",
    )
    require(
        './scripts/ci-fossil.sh diff .ci-artifacts/fossil '
        '"$CI_MERGE_REQUEST_DIFF_BASE_SHA"' in pipeline,
        "merge requests do not run the Fossil diff gate against their target base",
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
    for local_source in ("AGENTS.md", ".agents/", ".codex/", ".plans/", "docs/"):
        require(local_source not in pipeline, f"CI references local-only input: {local_source}")

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
