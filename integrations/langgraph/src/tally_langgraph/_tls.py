"""Shared TLS trust configuration for outbound HTTPS requests.

Python builds that do not link to the platform certificate store (notably the
python.org installer on macOS) cannot validate any HTTPS certificate until the
process is given an explicit CA bundle. Bundling ``certifi`` and always trusting
its bundle -- the same approach ``requests``/``httpx`` take -- means delivery and
the onboarding handshake work regardless of how the host Python was installed.
"""

from __future__ import annotations

import ssl
from urllib.request import HTTPSHandler

import certifi


def https_handler() -> HTTPSHandler:
    context = ssl.create_default_context(cafile=certifi.where())
    return HTTPSHandler(context=context)
