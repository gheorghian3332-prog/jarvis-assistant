"""Makes the cuBLAS / cuDNN DLLs from the ``nvidia-*-cu12`` wheels loadable by CTranslate2."""

from __future__ import annotations

import os
import sys
from pathlib import Path

_added: list[object] = []


def nvidia_bin_dirs() -> list[Path]:
    """Returns ``site-packages/nvidia/*/bin`` directories that exist."""
    found: list[Path] = []
    for entry in sys.path:
        if not entry:
            continue
        root = Path(entry) / "nvidia"
        if not root.is_dir():
            continue
        for package in sorted(root.iterdir()):
            bin_dir = package / "bin"
            if bin_dir.is_dir() and bin_dir not in found:
                found.append(bin_dir)
    return found


def add_dll_directories() -> list[Path]:
    """Registers the NVIDIA wheel DLL directories; returns the directories added."""
    directories = nvidia_bin_dirs()
    for directory in directories:
        if hasattr(os, "add_dll_directory"):
            _added.append(os.add_dll_directory(str(directory)))
        os.environ["PATH"] = str(directory) + os.pathsep + os.environ.get("PATH", "")
    return directories
