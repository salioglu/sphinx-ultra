#!/usr/bin/env python3
"""Run one guarded, deterministic Sphinx oracle build in a child process."""

from __future__ import annotations

import argparse
import itertools
import subprocess
import sys
import uuid
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))


def reject_network(*_args, **_kwargs):
    raise RuntimeError("network disabled by html oracle")


def install_shims() -> None:
    import socket

    socket.socket.connect = reject_network
    socket.create_connection = reject_network
    socket.getaddrinfo = reject_network
    uuid_counter = itertools.count(1)
    uuid.uuid4 = lambda: uuid.UUID(int=next(uuid_counter), version=4)


def _git(needs_root: Path, *arguments: str) -> str:
    command = ["git", "-c", f"safe.directory={needs_root}", "-C", str(needs_root), *arguments]
    result = subprocess.run(command, check=True, capture_output=True, text=True)
    return result.stdout.strip()


def verify_needs_checkout(needs_root: Path) -> None:
    from tools.gen_html_oracle import validate_needs_metadata

    root = Path(needs_root).resolve()
    source_root = root / "packages" / "sphinx-needs" / "src"
    sys.path.insert(0, str(source_root))
    sys.modules.pop("sphinx_needs", None)
    import sphinx_needs

    validate_needs_metadata(
        root,
        module_file=Path(sphinx_needs.__file__),
        module_version=sphinx_needs.__version__,
        commit=_git(root, "rev-parse", "HEAD"),
        tree=_git(root, "rev-parse", "HEAD:packages/sphinx-needs"),
        status=_git(root, "status", "--porcelain", "--", "packages/sphinx-needs"),
    )


def expected_versions(profile: str) -> tuple[str, str]:
    if profile == "core":
        return "9.1.0", "0.22.4"
    if profile == "local_needs":
        return "9.1.0", "0.21.2"
    raise RuntimeError(f"unknown oracle profile: {profile}")


def verify_profile_versions(profile: str) -> None:
    expected_sphinx, expected_docutils = expected_versions(profile)
    import docutils
    import sphinx

    if sphinx.__version__ != expected_sphinx:
        raise RuntimeError(f"Sphinx {sphinx.__version__!r} != {expected_sphinx!r}")
    if docutils.__version__ != expected_docutils:
        raise RuntimeError(f"Docutils {docutils.__version__!r} != {expected_docutils!r}")


def qualified_exception_type(exception: BaseException) -> str:
    exception_class = type(exception)
    return f"{exception_class.__module__}.{exception_class.__qualname__}"


def install_exception_handler(exception_file: Path) -> None:
    import sphinx._cli.util.errors

    original_handle_exception = sphinx._cli.util.errors.handle_exception

    def recording_handle_exception(exception: BaseException, *args, **kwargs):
        exception_file.write_text(
            qualified_exception_type(exception) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        return original_handle_exception(exception, *args, **kwargs)

    sphinx._cli.util.errors.handle_exception = recording_handle_exception


def build(args: argparse.Namespace) -> int:
    install_shims()
    if args.profile == "local_needs":
        if args.needs_root is None:
            raise RuntimeError("--needs-root is required for local_needs")
        verify_needs_checkout(args.needs_root)
    verify_profile_versions(args.profile)

    args.exception_file.parent.mkdir(parents=True, exist_ok=True)
    args.exception_file.unlink(missing_ok=True)
    install_exception_handler(args.exception_file)

    from sphinx.cmd.build import main

    args.warnings_file.parent.mkdir(parents=True, exist_ok=True)
    args.outputdir.mkdir(parents=True, exist_ok=True)
    args.doctree_dir.mkdir(parents=True, exist_ok=True)
    argv = [
        "-q",
        "-w",
        str(args.warnings_file),
        "-b",
        args.builder,
    ]
    if args.profile == "local_needs":
        argv.extend(["-D", "needs_reproducible_json=1"])
    argv.extend(
        [
            "-d",
            str(args.doctree_dir),
            str(args.sourcedir),
            str(args.outputdir),
        ]
    )
    try:
        result = main(argv)
    except SystemExit as exc:
        return int(exc.code or 0)
    return int(result or 0)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", required=True, choices=("core", "local_needs"))
    parser.add_argument("--sourcedir", type=Path, required=True)
    parser.add_argument("--outputdir", type=Path, required=True)
    parser.add_argument("--doctree-dir", type=Path, required=True)
    parser.add_argument("--builder", required=True)
    parser.add_argument("--warnings-file", type=Path, required=True)
    parser.add_argument("--exception-file", type=Path, required=True)
    parser.add_argument("--needs-root", type=Path)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    return build(parse_args(argv))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"{type(exc).__name__}: {exc}", file=sys.stderr)
        raise
