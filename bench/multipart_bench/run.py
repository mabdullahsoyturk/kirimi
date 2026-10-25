"""Run defnull's multipart_bench with fast-multipart added as a contestant.

    python bench/multipart_bench/run.py [--profile fast] [--plots] [pattern ...]

Steps:

1. Clone https://github.com/defnull/multipart_bench at a pinned commit into
   bench/multipart_bench/work (reset on every run) and apply
   fast-multipart.patch, which adds the adapters and a README entry.
2. Create a virtual environment there with the other parsers and this
   repository's package (built from source, so Rust is required). Uses uv if
   available, otherwise venv and pip.
3. Run the benchmark and render the README, then print the result tables.

Patterns are passed on to multipart_bench's run.py, e.g. "upload-*" or
"*-fast_multipart_*". The Makefile of multipart_bench pins the benchmark to
one CPU core with real-time priority; that is Linux-only and not done here,
so expect more noise than in the published results.
"""

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

REPOSITORY = "https://github.com/defnull/multipart_bench"
COMMIT = "2f90d62043f5c921d8c90671f014e6aa72d8ebfb"

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[1]
PATCH = HERE / "fast-multipart.patch"

RUN_REQUIREMENTS = ["jinja2", "numpy", "scipy"]
PLOT_REQUIREMENTS = ["matplotlib"]
# requirements-parsers.txt of multipart_bench installs multipart from a local
# checkout of its author, so multipart is listed here instead.
PARSERS = [
    "multipart",
    "werkzeug",
    "django",
    "python-multipart",
    "streaming-form-data",
    "emmett-core",
]


def run(*args, cwd=None, stdout=None):
    print("+", " ".join(str(a) for a in args), flush=True)
    # render_readme.py prints the README; keep it UTF-8 on Windows consoles.
    env = dict(os.environ, PYTHONIOENCODING="utf-8")
    subprocess.run([str(a) for a in args], cwd=cwd, check=True, stdout=stdout, env=env)


def checkout(workdir: Path):
    if not (workdir / ".git").exists():
        run("git", "clone", "-c", "core.autocrlf=false", REPOSITORY, workdir)
    git = ["git", "-C", workdir, "-c", "core.autocrlf=false"]
    run(*git, "fetch", "--quiet", "origin")
    run(*git, "checkout", "--quiet", "--force", COMMIT)
    run(*git, "clean", "--quiet", "--force", "-d", "--exclude=.venv", "--exclude=var")
    run(*git, "apply", PATCH)
    (workdir / "var").mkdir(exist_ok=True)


def venv_python(workdir: Path) -> Path:
    scripts = "Scripts" if os.name == "nt" else "bin"
    return workdir / ".venv" / scripts / ("python.exe" if os.name == "nt" else "python")


def install(workdir: Path, python_version: str, plots: bool):
    python = venv_python(workdir)
    packages = RUN_REQUIREMENTS + PARSERS + (PLOT_REQUIREMENTS if plots else [])
    uv = shutil.which("uv")
    if uv:
        if not python.exists():
            run(uv, "venv", "--python", python_version, workdir / ".venv")
        run(uv, "pip", "install", "--python", python, *packages)
        run(uv, "pip", "install", "--python", python, "--reinstall-package", "fast-multipart", PROJECT)
    else:
        if not python.exists():
            run(sys.executable, "-m", "venv", workdir / ".venv")
        run(python, "-m", "pip", "install", "--upgrade", *packages)
        run(python, "-m", "pip", "install", "--force-reinstall", "--no-deps", PROJECT)
    return python


def print_tables(readme: Path):
    for line in readme.read_text(encoding="utf8").splitlines():
        if line.startswith(("### Scenario", "|")):
            print(line)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--workdir", type=Path, default=HERE / "work")
    parser.add_argument("--python", default="3.12", help="Python version for the venv (uv only)")
    parser.add_argument(
        "--profile",
        default="fast",
        choices=["fast", "default", "slow", "slower", "slowest"],
        help="multipart_bench stability profile",
    )
    parser.add_argument("--plots", action="store_true", help="Also render plots (matplotlib)")
    parser.add_argument("--skip-install", action="store_true", help="Reuse the existing venv")
    parser.add_argument("--setup-only", action="store_true", help="Prepare, but do not run")
    parser.add_argument(
        "--render-only", action="store_true", help="Only render the results of the last run"
    )
    parser.add_argument("patterns", nargs="*", help="Benchmark name patterns for run.py")
    args = parser.parse_args()

    workdir = args.workdir.resolve()
    if args.render_only:
        python = venv_python(workdir)
    else:
        checkout(workdir)
        python = venv_python(workdir) if args.skip_install else install(workdir, args.python, args.plots)
        if args.setup_only:
            return
        run(python, "run.py", "--profile", args.profile, *args.patterns, cwd=workdir)
    readme = workdir / "README.md"
    with open(readme, "wb") as fp:
        run(python, "render_readme.py", cwd=workdir, stdout=fp)
    if args.plots:
        run(python, "render_plots.py", cwd=workdir)

    print_tables(readme)
    print(f"\nFull report: {readme}")


if __name__ == "__main__":
    main()
