# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later
"""Checks that the nix-darwin module's launchd job matches the standalone job.

The program arguments differ, so this compares every other key, and checks that
the module's job runs the packaged binary.
"""

import plistlib
import sys

standalone_path, module_path, program = sys.argv[1:]

with open(standalone_path, "rb") as file:
    standalone = plistlib.load(file)

with open(module_path, "rb") as file:
    module = plistlib.load(file)

arguments = module.pop("ProgramArguments")
standalone.pop("ProgramArguments")

if module != standalone:
    sys.exit(
        "the module's job differs from the standalone job:\n"
        f"module:     {module}\n"
        f"standalone: {standalone}"
    )

if not arguments[-1].endswith(f"exec {program}"):
    sys.exit(f"the module's job runs {arguments}, not {program}")
