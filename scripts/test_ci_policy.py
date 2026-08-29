#!/usr/bin/env python3
import unittest

from ci_policy import PolicyError, validate_pipeline_gating


STABLE_TAG_RULE = "'$CI_COMMIT_TAG =~ /^v\\d+\\.\\d+\\.\\d+$/'"


def pipeline(workflow_rules: str, shared_rules: str) -> str:
    return f"""stages: []

workflow:
  rules:
{workflow_rules}

variables:
  VALUE: value

.shared-pipeline:
  rules:
{shared_rules}

.rust-job:
  image: example
"""


class PipelineGatingTests(unittest.TestCase):
    def test_accepts_manual_scheduled_and_stable_release_sources(self) -> None:
        source = pipeline(
            f"    - if: {STABLE_TAG_RULE}\n"
            '    - if: \'$CI_PIPELINE_SOURCE == "web"\'\n'
            '    - if: \'$CI_PIPELINE_SOURCE == "schedule"\'\n'
            "    - when: never",
            '    - if: \'$CI_PIPELINE_SOURCE == "web"\'',
        )

        validate_pipeline_gating(source)

    def test_rejects_automatic_push_pipelines(self) -> None:
        source = pipeline(
            f"    - if: {STABLE_TAG_RULE}\n"
            '    - if: \'$CI_PIPELINE_SOURCE == "web"\'\n'
            '    - if: \'$CI_PIPELINE_SOURCE == "schedule"\'\n'
            '    - if: \'$CI_PIPELINE_SOURCE == "push"\'\n'
            "    - when: never",
            '    - if: \'$CI_PIPELINE_SOURCE == "web"\'',
        )

        with self.assertRaisesRegex(PolicyError, "automatic commit"):
            validate_pipeline_gating(source)

    def test_rejects_automatic_merge_request_validation(self) -> None:
        source = pipeline(
            f"    - if: {STABLE_TAG_RULE}\n"
            '    - if: \'$CI_PIPELINE_SOURCE == "web"\'\n'
            '    - if: \'$CI_PIPELINE_SOURCE == "schedule"\'\n'
            "    - when: never",
            '    - if: \'$CI_PIPELINE_SOURCE == "merge_request_event"\'',
        )

        with self.assertRaisesRegex(PolicyError, "manual preflights"):
            validate_pipeline_gating(source)


if __name__ == "__main__":
    unittest.main()
