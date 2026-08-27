#!/usr/bin/env python3
"""Fast package-aware Rust verification for local/agent workflows."""

from __future__ import annotations

import json
import subprocess
import sys
from collections import defaultdict, deque
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class WorkspaceGraph:
    order: tuple[str, ...]
    roots: tuple[tuple[Path, str], ...]
    reverse: dict[str, tuple[str, ...]]

    @classmethod
    def load(cls) -> "WorkspaceGraph":
        metadata = json.loads(output(["cargo", "metadata", "--no-deps", "--format-version=1"]))
        members = set(metadata["workspace_members"])
        packages = [package for package in metadata["packages"] if package["id"] in members]
        order = tuple(package["name"] for package in packages)
        names = set(order)
        roots = []
        reverse: dict[str, list[str]] = defaultdict(list)

        for package in packages:
            name = package["name"]
            package_root = Path(package["manifest_path"]).parent.relative_to(ROOT)
            roots.append((package_root, name))
            for dependency in package["dependencies"]:
                dependency_name = dependency["name"]
                if dependency.get("path") and dependency_name in names:
                    reverse[dependency_name].append(name)

        roots.sort(key=lambda item: len(item[0].parts), reverse=True)
        return cls(
            order=order,
            roots=tuple(roots),
            reverse={name: tuple(dependents) for name, dependents in reverse.items()},
        )

    def validate(self, packages: list[str]) -> list[str]:
        unknown = sorted(set(packages) - set(self.order))
        if unknown:
            fail(f"unknown workspace package(s): {', '.join(unknown)}\navailable: {', '.join(self.order)}")
        requested = set(packages)
        return [package for package in self.order if package in requested]

    def owner(self, path: Path) -> str | None:
        for root, package in self.roots:
            if path == root or root in path.parents:
                return package
        return None

    def reverse_closure(self, packages: list[str]) -> list[str]:
        included = set(packages)
        queue = deque(packages)
        while queue:
            package = queue.popleft()
            for dependent in self.reverse.get(package, ()):
                if dependent not in included:
                    included.add(dependent)
                    queue.append(dependent)
        return [package for package in self.order if package in included]


@dataclass(frozen=True)
class Plan:
    changed: tuple[str, ...] = ()
    lint: tuple[str, ...] = ()
    full_reason: str | None = None
    message: str | None = None


def main() -> None:
    if len(sys.argv) < 2 or sys.argv[1] not in {"lint", "verify", "plan"}:
        fail("usage: scoped_verify.py <lint|verify|plan> [workspace-package ...]")

    command = sys.argv[1]
    packages = sys.argv[2:]
    graph = WorkspaceGraph.load()

    if command == "lint":
        if not packages:
            fail("lint requires at least one package; try `just lint denju`")
        run_clippy(graph.validate(packages), all_targets=False)
        return

    plan = make_plan(graph, packages)
    describe(plan)
    if command == "plan" or plan.message:
        return
    if plan.full_reason:
        run(["cargo", "xtask", "rust"])
        return

    run(["cargo", "fmt", "--all", "--check"])
    run_clippy(list(plan.lint), all_targets=True)
    run(["cargo", "test", *package_args(plan.changed)])


def make_plan(graph: WorkspaceGraph, explicit: list[str]) -> Plan:
    if explicit:
        changed = graph.validate(explicit)
    else:
        base, paths = changed_paths()
        if not paths:
            return Plan(message="no changes relative to origin/main; nothing to verify")
        for path in paths:
            reason = full_rust_reason(path)
            if reason:
                return Plan(full_reason=reason)
        changed_set = {owner for path in paths if (owner := graph.owner(path))}
        if not changed_set:
            return Plan(
                message=(
                    f"no Rust package changes detected relative to {base}; "
                    "use `just docs`, `just npm-check`, or `just full` for other surfaces"
                )
            )
        changed = [package for package in graph.order if package in changed_set]

    return Plan(changed=tuple(changed), lint=tuple(graph.reverse_closure(changed)))


def changed_paths() -> tuple[str, set[Path]]:
    try:
        base = output(["git", "merge-base", "HEAD", "origin/main"]).strip()
    except subprocess.CalledProcessError:
        base = output(["git", "rev-parse", "HEAD"]).strip()

    paths = {
        Path(line)
        for line in output(["git", "diff", "--name-only", base, "--"]).splitlines()
        if line.strip()
    }
    paths.update(
        Path(line)
        for line in output(["git", "ls-files", "--others", "--exclude-standard"]).splitlines()
        if line.strip()
    )
    return base, paths


def full_rust_reason(path: Path) -> str | None:
    text = path.as_posix()
    if text in {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml"}:
        return "workspace/toolchain configuration changed"
    if text.startswith(".cargo/"):
        return "Cargo configuration changed"
    if text.startswith(".sqlx/"):
        return "shared SQLx metadata changed"
    if text.startswith("spec/"):
        return "shared specification/fixture inputs changed"
    return None


def describe(plan: Plan) -> None:
    if plan.message:
        print(plan.message)
        return
    if plan.full_reason:
        print(f"scoped verification escalates to full Rust: {plan.full_reason}")
        return
    print(f"changed packages: {', '.join(plan.changed)}")
    if plan.lint != plan.changed:
        print(f"compile/lint closure: {', '.join(plan.lint)}")


def run_clippy(packages: list[str], *, all_targets: bool) -> None:
    command = ["cargo", "clippy", *package_args(packages)]
    if all_targets:
        command.append("--all-targets")
    command.extend(["--no-deps", "--", "-D", "warnings"])
    run(command)


def package_args(packages: tuple[str, ...] | list[str]) -> list[str]:
    args: list[str] = []
    for package in packages:
        args.extend(["-p", package])
    return args


def run(command: list[str]) -> None:
    print("+ " + " ".join(command), file=sys.stderr, flush=True)
    subprocess.run(command, cwd=ROOT, check=True)


def output(command: list[str]) -> str:
    return subprocess.check_output(command, cwd=ROOT, text=True, stderr=subprocess.PIPE)


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(2)


if __name__ == "__main__":
    main()
