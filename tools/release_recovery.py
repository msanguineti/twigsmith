#!/usr/bin/env python3
"""Compute the next release step from fresh provider observations.

This pure function performs no I/O, locking, publication or source eligibility
checks. Its caller must authenticate observations, serialize invocations, verify
approved inputs and re-observe after every attempted operation, including timeouts.
"""
import re


def next_step(expected_assets, expected_npm, observed):
    """Return one operation or a stop reason; inputs are exact SHA-256 strings.

    observed contains: exclusive (bool), release (absent/draft/published/unknown),
    assets (filename -> sha256, or None if unknown), npm (absent, unknown, or
    exact sha256), consumption_verified (bool), and site_promoted (bool).
    An absent release has an empty assets mapping. A caller must obtain fresh,
    complete observations and guarantee exclusivity externally for this advice
    to be actionable. Unknown state always stops.
    """
    if observed.get('exclusive') is not True:
        return 'stop: exclusivity not established'
    release = observed.get('release')
    assets = observed.get('assets')
    npm = observed.get('npm')
    channel = observed.get('channel')
    if release not in ('absent', 'draft', 'published') or not isinstance(assets, dict) or npm is None or npm == 'unknown' or channel not in ('expected', 'other'):
        return 'stop: remote state unknown'
    if (not expected_assets or not isinstance(expected_npm, str)
            or not re.fullmatch(r'[0-9a-f]{64}', expected_npm)
            or any(not isinstance(name, str) or not name or '/' in name
                   or not isinstance(digest, str) or not re.fullmatch(r'[0-9a-f]{64}', digest)
                   for name, digest in expected_assets.items())):
        return 'stop: incomplete approved identity'
    if release == 'absent' and assets:
        return 'stop: inconsistent release observation'
    # Unexpected assets are not permitted in the release allowlist either.
    if any(name not in expected_assets or expected_assets[name] != digest for name, digest in assets.items()):
        return 'stop: conflicting release asset'
    if npm != 'absent' and npm != expected_npm:
        return 'stop: conflicting npm version'
    if release == 'absent':
        if npm != 'absent':
            return 'stop: npm exists without release'
        return 'create draft'
    missing = sorted(set(expected_assets) - set(assets))
    if missing:
        if release == 'published':
            return 'stop: published release missing assets'
        return 'upload ' + missing[0]
    if release == 'draft':
        if npm != 'absent':
            return 'stop: npm exists before release publication'
        return 'publish release'
    if npm == 'absent':
        return 'publish npm'
    if observed.get('consumption_verified') is not True:
        return 'verify public consumption'
    if channel != 'expected':
        return 'promote channel'
    if observed.get('site_promoted') is not True:
        return 'promote site'
    return 'complete'
