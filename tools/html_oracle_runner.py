#!/usr/bin/env python3
"""Run one guarded, deterministic Sphinx oracle build in a child process.

The runner reports to the generator through a JSON result file:

* ``exception_type``: qualified type of the exception that ended the build,
  including crashes Sphinx itself does not handle, or null when Sphinx
  returned normally.
* ``sphinx_error``: whether that exception is a ``sphinx.errors.SphinxError``;
  the generator files those as build errors and everything else as a
  reference crash.
* ``network_attempted``: whether anything tried to open a network connection.
  Such a case is excluded because its output depends on the network.
* ``setup_error``: set when the runner's own preconditions failed before the
  build started; the generator treats the run as fatal.
"""

from __future__ import annotations

import argparse
import itertools
import json
import sys
import uuid
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

SETUP_FAILURE_EXIT_CODE = 3
NETWORK_ATTEMPTS: list[str] = []


def reject_network(*_args, **_kwargs):
    NETWORK_ATTEMPTS.append("socket")
    raise RuntimeError("network disabled by html oracle")


def install_shims() -> None:
    import socket

    socket.socket.connect = reject_network
    socket.create_connection = reject_network
    socket.getaddrinfo = reject_network
    uuid_counter = itertools.count(1)
    uuid.uuid4 = lambda: uuid.UUID(int=next(uuid_counter), version=4)


def use_needs_checkout(needs_root: Path) -> None:
    """Import sphinx_needs from the pinned checkout instead of the venv wheel.

    The git state of the checkout is verified once by the generator; the
    child only verifies that the import resolves to the pinned source.
    """
    from tools.gen_html_oracle import validate_needs_module

    root = Path(needs_root).resolve()
    source_root = root / "packages" / "sphinx-needs" / "src"
    sys.path.insert(0, str(source_root))
    sys.modules.pop("sphinx_needs", None)
    import sphinx_needs

    validate_needs_module(
        root,
        module_file=Path(sphinx_needs.__file__),
        module_version=sphinx_needs.__version__,
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


class RunnerResult:
    def __init__(self) -> None:
        self.exception_type: str | None = None
        self.sphinx_error = False
        self.setup_error: str | None = None

    def record(self, exception: BaseException) -> None:
        from sphinx.errors import SphinxError

        self.exception_type = qualified_exception_type(exception)
        self.sphinx_error = isinstance(exception, SphinxError)

    def write(self, path: Path) -> None:
        payload = {
            "exception_type": self.exception_type,
            "sphinx_error": self.sphinx_error,
            "network_attempted": bool(NETWORK_ATTEMPTS),
            "setup_error": self.setup_error,
        }
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            json.dumps(payload, sort_keys=True) + "\n",
            encoding="utf-8",
            newline="\n",
        )


def install_search_root_normalizer(source_root: Path) -> None:
    """Index ``<SRCDIR>`` in place of the absolute source directory.

    A kept system message can quote the absolute source path in page text,
    and Sphinx would then index that path's components as search terms,
    which ties the reference to the machine that built it.  The page itself
    is normalized to ``<SRCDIR>`` by the generator's text policy, so the
    index is built from the same spelling.  The differential harness applies
    the matching normalization to Ultra's index.
    """
    import sphinx.search
    from tools.gen_html_oracle import root_pattern

    pattern = root_pattern(source_root)
    original_init = sphinx.search.IndexBuilder.__init__

    def init_with_normalized_split(self, *args, **kwargs):
        original_init(self, *args, **kwargs)
        original_split = self.lang.split

        def split(text: str) -> list[str]:
            normalized = pattern.sub(b"<SRCDIR>", text.encode("utf-8", "surrogatepass"))
            return original_split(normalized.decode("utf-8", "surrogatepass"))

        self.lang.split = split

    sphinx.search.IndexBuilder.__init__ = init_with_normalized_split


def install_buildinfo_normalizer() -> None:
    """Hash ``<SITE_PACKAGES>`` in place of the environment's package path.

    ``sphinx.ext.graphviz`` (loaded by sphinx-needs) appends the absolute
    path of a stylesheet below Sphinx's own package directory to
    ``html_static_path``, so the ``.buildinfo`` config hash of every such
    project would otherwise depend on where the profile's virtualenv lives.
    Only the hash input is rewritten; the build itself is unchanged.
    """
    import sphinx
    import sphinx.builders.html._build_info as build_info
    from tools.gen_html_oracle import root_pattern

    pattern = root_pattern(Path(sphinx.__file__).resolve().parent.parent)
    original_stable_hash = build_info.stable_hash

    def normalize(value):
        if isinstance(value, str):
            return pattern.sub(b"<SITE_PACKAGES>", value.encode("utf-8", "surrogatepass")).decode(
                "utf-8", "surrogatepass"
            )
        if isinstance(value, dict):
            return {key: normalize(item) for key, item in value.items()}
        if isinstance(value, list):
            return [normalize(item) for item in value]
        if isinstance(value, tuple):
            return tuple(normalize(item) for item in value)
        return value

    def stable_hash(obj):
        return original_stable_hash(normalize(obj))

    build_info.stable_hash = stable_hash


def install_exception_handler(result: RunnerResult) -> None:
    """Record the exception Sphinx reports before its own handler runs."""
    import sphinx._cli.util.errors

    original_handle_exception = sphinx._cli.util.errors.handle_exception

    def recording_handle_exception(exception: BaseException, *args, **kwargs):
        result.record(exception)
        return original_handle_exception(exception, *args, **kwargs)

    sphinx._cli.util.errors.handle_exception = recording_handle_exception


def exit_code_of(system_exit: SystemExit) -> int:
    code = system_exit.code
    if code is None:
        return 0
    if isinstance(code, int):
        return code
    print(code, file=sys.stderr)
    return 1


def build(args: argparse.Namespace) -> int:
    result = RunnerResult()
    args.result_file.unlink(missing_ok=True)
    try:
        install_shims()
        if args.profile == "local_needs":
            if args.needs_root is None:
                raise RuntimeError("--needs-root is required for local_needs")
            use_needs_checkout(args.needs_root)
        verify_profile_versions(args.profile)
        install_exception_handler(result)
        install_search_root_normalizer(args.sourcedir)
        install_buildinfo_normalizer()
        from sphinx.cmd.build import main
    except Exception as exc:
        result.setup_error = f"{type(exc).__name__}: {exc}"
        result.write(args.result_file)
        print(result.setup_error, file=sys.stderr)
        return SETUP_FAILURE_EXIT_CODE

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
        exit_code = int(main(argv) or 0)
    except SystemExit as exc:
        exit_code = exit_code_of(exc)
        if exit_code != 0:
            result.record(exc)
    except BaseException as exc:  # noqa: BLE001 - crashes Sphinx does not handle
        result.record(exc)
        print(f"{qualified_exception_type(exc)}: {exc}", file=sys.stderr)
        exit_code = 2
    result.write(args.result_file)
    return exit_code


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", required=True, choices=("core", "local_needs"))
    parser.add_argument("--sourcedir", type=Path, required=True)
    parser.add_argument("--outputdir", type=Path, required=True)
    parser.add_argument("--doctree-dir", type=Path, required=True)
    parser.add_argument("--builder", required=True)
    parser.add_argument("--warnings-file", type=Path, required=True)
    parser.add_argument("--result-file", type=Path, required=True)
    parser.add_argument("--needs-root", type=Path)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    return build(parse_args(argv))


if __name__ == "__main__":
    raise SystemExit(main())
