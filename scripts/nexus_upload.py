#!/usr/bin/env python3
"""Publish a release artifact as a new version of an existing Nexus Mods file."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Callable
from xml.sax.saxutils import escape


API_BASE = "https://api.nexusmods.com/v3"
USER_AGENT = "Deployd-release-uploader/1.0"


class UploadError(RuntimeError):
    """Raised when Nexus rejects or cannot complete an upload."""


class NexusUploader:
    def __init__(
        self,
        api_key: str,
        *,
        open_url: Callable[..., Any] = urllib.request.urlopen,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        if not api_key:
            raise UploadError("NEXUSMODS_API_KEY is required")
        self._api_key = api_key
        self._open_url = open_url
        self._sleep = sleep

    def publish(
        self,
        artifact: Path,
        file_id: str,
        version: str,
        display_name: str,
        archive_existing: bool,
    ) -> str:
        if not artifact.is_file():
            raise UploadError(f"Release artifact does not exist: {artifact}")
        if not file_id:
            raise UploadError("Nexus file ID is required")
        if not version:
            raise UploadError("Release version is required")

        size = artifact.stat().st_size
        upload_id = self._upload_multipart(artifact, size)

        self._api_json("POST", f"/uploads/{upload_id}/finalise", expected_status=200)
        self._wait_until_available(upload_id)

        response = self._api_json(
            "POST",
            f"/mod-files/{urllib.parse.quote(file_id, safe='')}/versions",
            {
                "upload_id": upload_id,
                "name": display_name,
                "version": version,
                "file_category": "main",
                "archive_existing_file": archive_existing,
            },
            expected_status=201,
        )
        version_id = self._required(response, "data", "version", "id")
        if not isinstance(version_id, str):
            raise UploadError("Nexus returned a non-string file version ID")
        return version_id

    def _upload_multipart(self, artifact: Path, size: int) -> str:
        response = self._api_json(
            "POST",
            "/uploads/multipart",
            {"filename": artifact.name, "size_bytes": size},
            expected_status=201,
        )
        upload_id = self._upload_id(response)
        data = self._required(response, "data")
        if not isinstance(data, dict):
            raise UploadError("Nexus returned invalid multipart upload data")

        part_size = data.get("part_size_bytes")
        part_urls = data.get("part_presigned_urls")
        complete_url = data.get("complete_presigned_url")
        if not isinstance(part_size, int) or part_size <= 0:
            raise UploadError("Nexus returned an invalid multipart part size")
        if not isinstance(part_urls, list) or not all(
            isinstance(url, str) for url in part_urls
        ):
            raise UploadError("Nexus returned invalid multipart part URLs")
        if not isinstance(complete_url, str):
            raise UploadError("Nexus returned an invalid multipart completion URL")

        etags: list[str] = []
        with artifact.open("rb") as artifact_file:
            for part_number, part_url in enumerate(part_urls, start=1):
                part = artifact_file.read(part_size)
                if not part:
                    raise UploadError(
                        "Nexus supplied more multipart URLs than the artifact requires"
                    )
                etags.append(self._put_part(part_url, part, part_number))
            if artifact_file.read(1):
                raise UploadError(
                    "Nexus supplied too few multipart URLs for the artifact"
                )

        parts_xml = "".join(
            "<Part>"
            f"<PartNumber>{part_number}</PartNumber>"
            f"<ETag>{escape(etag)}</ETag>"
            "</Part>"
            for part_number, etag in enumerate(etags, start=1)
        )
        completion_xml = f"<CompleteMultipartUpload>{parts_xml}</CompleteMultipartUpload>"
        completion_response = self._presigned_request(
            "POST",
            complete_url,
            completion_xml.encode("utf-8"),
            {"Content-Type": "application/xml"},
        )
        completion_response.close()
        return upload_id

    def _put_part(self, url: str, part: bytes, part_number: int) -> str:
        response = self._presigned_request(
            "PUT",
            url,
            part,
            {
                "Content-Type": "application/octet-stream",
                "Content-Length": str(len(part)),
            },
        )
        etag = response.headers.get("ETag")
        response.close()
        if not etag:
            raise UploadError(f"Nexus upload part {part_number} returned no ETag")
        return etag.strip('"')

    def _presigned_request(
        self, method: str, url: str, body: bytes, headers: dict[str, str]
    ) -> Any:
        self._validate_presigned_url(url)
        request = urllib.request.Request(
            url, data=body, headers=headers, method=method
        )
        return self._send(request, f"complete {method} request to upload storage")

    def _wait_until_available(self, upload_id: str) -> None:
        for attempt in range(60):
            response = self._api_json(
                "GET", f"/uploads/{upload_id}", expected_status=200
            )
            state = self._required(response, "data", "state")
            if state == "available":
                return
            if state != "created":
                raise UploadError(f"Nexus returned unknown upload state: {state!r}")
            self._sleep(min(2 * (1.5**attempt), 30))
        raise UploadError("Nexus did not make the upload available before timeout")

    def _api_json(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        *,
        expected_status: int,
    ) -> dict[str, Any]:
        encoded_body = None if body is None else json.dumps(body).encode("utf-8")
        request = urllib.request.Request(
            f"{API_BASE}{path}",
            data=encoded_body,
            headers={
                "Accept": "application/json",
                "Content-Type": "application/json",
                "apikey": self._api_key,
                "User-Agent": USER_AGENT,
            },
            method=method,
        )
        response = self._send(request, f"call Nexus API {method} {path}")
        try:
            if response.status != expected_status:
                raise UploadError(
                    f"Nexus API {method} {path} returned HTTP {response.status}; "
                    f"expected {expected_status}"
                )
            try:
                decoded = json.loads(response.read())
            except (json.JSONDecodeError, UnicodeDecodeError) as error:
                raise UploadError(
                    f"Nexus API {method} {path} returned invalid JSON"
                ) from error
        finally:
            response.close()
        if not isinstance(decoded, dict):
            raise UploadError(f"Nexus API {method} {path} returned invalid data")
        return decoded

    def _send(self, request: urllib.request.Request, operation: str) -> Any:
        try:
            return self._open_url(request, timeout=300)
        except urllib.error.HTTPError as error:
            try:
                detail = error.read().decode("utf-8", errors="replace")[:2000]
            finally:
                error.close()
            raise UploadError(
                f"Failed to {operation}: HTTP {error.code}: {detail}"
            ) from error
        except urllib.error.URLError as error:
            raise UploadError(f"Failed to {operation}: {error.reason}") from error

    @staticmethod
    def _validate_presigned_url(url: str) -> None:
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme != "https" or not parsed.netloc:
            raise UploadError("Nexus returned an invalid non-HTTPS upload URL")

    @staticmethod
    def _required(document: dict[str, Any], *path: str) -> Any:
        value: Any = document
        for key in path:
            if not isinstance(value, dict) or key not in value:
                joined_path = ".".join(path)
                raise UploadError(f"Nexus response is missing {joined_path}")
            value = value[key]
        return value

    def _upload_id(self, response: dict[str, Any]) -> str:
        upload_id = self._required(response, "data", "id")
        if not isinstance(upload_id, str):
            raise UploadError("Nexus returned a non-string upload ID")
        return upload_id


def parse_args(arguments: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--file", required=True, type=Path, dest="artifact")
    parser.add_argument("--file-id", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--display-name", required=True)
    parser.add_argument("--archive-existing", action="store_true")
    return parser.parse_args(arguments)


def main(arguments: list[str]) -> int:
    args = parse_args(arguments)
    try:
        uploader = NexusUploader(os.environ.get("NEXUSMODS_API_KEY", ""))
        version_id = uploader.publish(
            args.artifact,
            args.file_id,
            args.version,
            args.display_name,
            args.archive_existing,
        )
    except (OSError, UploadError) as error:
        print(f"Nexus upload failed: {error}", file=sys.stderr)
        return 1

    print(f"Published Nexus file version {version_id}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
