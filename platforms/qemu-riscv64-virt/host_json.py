"""Strict JSON decoding for local host services."""

from http.client import HTTPException
import json

HTTP_ERRORS = (OSError, HTTPException, ValueError, RecursionError)


def decode_json(data: bytes | str) -> object:
    if isinstance(data, bytes):
        data = data.decode("utf-8")
    return json.loads(data, object_pairs_hook=unique_fields)


def unique_fields(pairs: list[tuple[str, object]]) -> dict:
    result = dict(pairs)
    if len(result) != len(pairs):
        raise ValueError("duplicate JSON field")
    return result
