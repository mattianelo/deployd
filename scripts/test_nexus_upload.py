#!/usr/bin/env python3

from __future__ import annotations

import io
import json
import tempfile
import unittest
import urllib.error
from pathlib import Path
from typing import Any

import nexus_upload


class FakeResponse:
    def __init__(
        self,
        status: int,
        body: dict[str, Any] | None = None,
        headers: dict[str, str] | None = None,
    ) -> None:
        self.status = status
        self._body = json.dumps(body or {}).encode("utf-8")
        self.headers = headers or {}

    def read(self) -> bytes:
        return self._body

    def close(self) -> None:
        pass


class RecordingTransport:
    def __init__(self, responses: list[FakeResponse]) -> None:
        self.responses = responses
        self.requests: list[Any] = []

    def __call__(self, request: Any, *, timeout: int) -> FakeResponse:
        self.requests.append(request)
        if timeout != 300:
            raise AssertionError(f"Unexpected timeout: {timeout}")
        if not self.responses:
            raise AssertionError("Unexpected HTTP request")
        return self.responses.pop(0)


class NexusUploaderTests(unittest.TestCase):
    def test_small_artifact_uses_single_upload_session(self) -> None:
        transport = RecordingTransport(
            [
                FakeResponse(
                    201,
                    {
                        "data": {
                            "id": "upload-id",
                            "state": "created",
                            "presigned_url": "https://storage.example/upload",
                        }
                    },
                ),
                FakeResponse(200),
                FakeResponse(200),
                FakeResponse(200, {"data": {"id": "upload-id", "state": "created"}}),
                FakeResponse(200, {"data": {"id": "upload-id", "state": "available"}}),
                FakeResponse(201, {"data": {"version": {"id": "version-id"}}}),
            ]
        )
        uploader = nexus_upload.NexusUploader(
            "secret", open_url=transport, sleep=lambda _: None
        )

        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "Deployd.AppImage"
            artifact.write_bytes(b"appimage")
            version_id = uploader.publish(
                artifact, "file-id", "2.4.0", "Deployd 2.4.0", True
            )

        self.assertEqual(version_id, "version-id")
        self.assertEqual(
            [request.get_method() for request in transport.requests],
            ["POST", "PUT", "POST", "GET", "GET", "POST"],
        )
        self.assertEqual(transport.requests[0].full_url, f"{nexus_upload.API_BASE}/uploads")
        create_version = json.loads(transport.requests[-1].data)
        self.assertEqual(create_version["upload_id"], "upload-id")
        self.assertTrue(create_version["archive_existing_file"])

    def test_rejects_non_https_presigned_url(self) -> None:
        transport = RecordingTransport(
            [
                FakeResponse(
                    201,
                    {
                        "data": {
                            "id": "upload-id",
                            "state": "created",
                            "presigned_url": "http://storage.example/upload",
                        }
                    },
                )
            ]
        )
        uploader = nexus_upload.NexusUploader("secret", open_url=transport)

        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "Deployd.AppImage"
            artifact.write_bytes(b"appimage")
            with self.assertRaisesRegex(nexus_upload.UploadError, "non-HTTPS"):
                uploader.publish(artifact, "file-id", "2.4.0", "Deployd 2.4.0", True)

    def test_large_artifact_uses_multipart_upload_session(self) -> None:
        transport = RecordingTransport(
            [
                FakeResponse(
                    201,
                    {
                        "data": {
                            "id": "upload-id",
                            "state": "created",
                            "part_size_bytes": 4,
                            "part_presigned_urls": [
                                "https://storage.example/part-1",
                                "https://storage.example/part-2",
                            ],
                            "complete_presigned_url": "https://storage.example/complete",
                        }
                    },
                ),
                FakeResponse(200, headers={"ETag": '"etag-1"'}),
                FakeResponse(200, headers={"ETag": '"etag-2"'}),
                FakeResponse(200),
                FakeResponse(200, {"data": {"id": "upload-id", "state": "created"}}),
                FakeResponse(200, {"data": {"id": "upload-id", "state": "available"}}),
                FakeResponse(201, {"data": {"version": {"id": "version-id"}}}),
            ]
        )
        uploader = nexus_upload.NexusUploader(
            "secret", open_url=transport, sleep=lambda _: None
        )
        original_limit = nexus_upload.SINGLE_PART_LIMIT
        nexus_upload.SINGLE_PART_LIMIT = 1
        try:
            with tempfile.TemporaryDirectory() as directory:
                artifact = Path(directory) / "Deployd.AppImage"
                artifact.write_bytes(b"12345678")
                version_id = uploader.publish(
                    artifact, "file-id", "2.4.0", "Deployd 2.4.0", True
                )
        finally:
            nexus_upload.SINGLE_PART_LIMIT = original_limit

        self.assertEqual(version_id, "version-id")
        self.assertEqual(
            transport.requests[0].full_url,
            f"{nexus_upload.API_BASE}/uploads/multipart",
        )
        self.assertIn(b"<ETag>etag-1</ETag>", transport.requests[3].data)

    def test_reports_api_error_without_exposing_key(self) -> None:
        def reject(request: Any, *, timeout: int) -> Any:
            del request, timeout
            raise urllib.error.HTTPError(
                f"{nexus_upload.API_BASE}/uploads",
                403,
                "Forbidden",
                {},
                io.BytesIO(b"forbidden"),
            )

        uploader = nexus_upload.NexusUploader("super-secret", open_url=reject)
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "Deployd.AppImage"
            artifact.write_bytes(b"appimage")
            with self.assertRaises(nexus_upload.UploadError) as raised:
                uploader.publish(artifact, "file-id", "2.4.0", "Deployd 2.4.0", True)

        self.assertNotIn("super-secret", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
