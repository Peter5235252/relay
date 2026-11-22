#!/usr/bin/env python3
"""Build and install Relay on Linux, macOS, and Windows 11. Requires Python 3.10+."""
from __future__ import annotations
import argparse
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CARGO_BIN = Path.home() / ".cargo" / "bin"
PACKAGES = {
    "apt": """build-essential pkg-config cmake clang libssl-dev libfontconfig1-dev
        libfreetype6-dev libx11-dev libx11-xcb-dev libxcb1-dev libxcb-render0-dev
        libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libxkbcommon-x11-dev
        libwayland-dev libegl1-mesa-dev libvulkan-dev libasound2-dev libglib2.0-dev
        libgtk-3-dev""".split(),
    "pacman": """base-devel pkgconf cmake clang openssl fontconfig freetype2 libx11
        libxcb libxkbcommon libxkbcommon-x11 wayland mesa vulkan-headers
        vulkan-icd-loader alsa-lib glib2 gtk3""".split(),
    "xbps": """base-devel pkg-config cmake clang openssl-devel fontconfig-devel
        freetype-devel libX11-devel libxcb-devel libxkbcommon-devel wayland-devel
        MesaLib-devel libglvnd-devel Vulkan-Headers vulkan-loader-devel
        alsa-lib-devel libglib-devel gtk+3-devel""".split(),
    "dnf": """gcc gcc-c++ make pkgconf-pkg-config cmake clang openssl-devel
        fontconfig-devel freetype-devel libX11-devel libX11-xcb-devel libxcb-devel
        libxkbcommon-devel libxkbcommon-x11-devel wayland-devel mesa-libEGL-devel
        vulkan-headers vulkan-loader-devel alsa-lib-devel glib2-devel gtk3-devel""".split(),
    "zypper": """gcc gcc-c++ make pkg-config cmake clang libopenssl-devel
        fontconfig-devel freetype2-devel libX11-devel libxcb-devel libxkbcommon-devel
        wayland-devel Mesa-libEGL-devel vulkan-devel alsa-devel glib2-devel gtk3-devel""".split(),
    "apk": """build-base pkgconf cmake clang openssl-dev fontconfig-dev freetype-dev
        libx11-dev libxcb-dev libxkbcommon-dev wayland-dev mesa-dev vulkan-headers
        vulkan-loader alsa-lib-dev glib-dev gtk+3.0-dev""".split(),
    "emerge": """sys-devel/gcc sys-devel/make dev-util/pkgconf dev-util/cmake
        sys-devel/clang dev-libs/openssl media-libs/fontconfig media-libs/freetype
        x11-libs/libX11 x11-libs/libxcb x11-libs/libxkbcommon dev-libs/wayland
        media-libs/mesa dev-util/vulkan-headers media-libs/alsa-lib dev-libs/glib
        x11-libs/gtk+""".split(),
}

def run(args: list[str], *, cwd: Path = ROOT) -> None:
    print("+", subprocess.list2cmdline(args), flush=True)
    subprocess.run(args, cwd=cwd, check=True)

def os_release() -> dict[str, str]:
    result: dict[str, str] = {}
    path = Path("/etc/os-release")
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            if line and not line.startswith("#") and "=" in line:
                key, value = line.split("=", 1)
                result[key] = value.strip().strip('"').strip("'").lower()
    return result

def linux_manager() -> str | None:
    info = os_release()
    ids = {info.get("ID", ""), *info.get("ID_LIKE", "").split()}
    if ids & {"debian", "ubuntu", "linuxmint", "pop", "elementary", "kali", "raspbian"}: return "apt"
    if ids & {"arch", "manjaro", "endeavouros", "garuda"}: return "pacman"
    if "void" in ids: return "xbps"
    if ids & {"fedora", "rhel", "centos", "rocky", "almalinux", "ol"}: return "dnf"
    if ids & {"opensuse", "opensuse-leap", "opensuse-tumbleweed", "sles", "sled"}: return "zypper"
    if "alpine" in ids: return "apk"
    if "gentoo" in ids: return "emerge"
    return None

def privileged(args: list[str]) -> None:
    if hasattr(os, "geteuid") and os.geteuid() == 0:
        run(args)
    elif shutil.which("sudo"):
        run(["sudo", *args])
    elif shutil.which("doas"):
        run(["doas", *args])
    else:
        raise RuntimeError("Installing system packages needs root access. Configure sudo/doas or install the packages manually.")

def install_linux_deps(skip: bool) -> None:
    manager = linux_manager()
    if not skip and manager is None:
        name = os_release().get("PRETTY_NAME", "this Linux distribution")
        raise RuntimeError(f"Automatic dependency installation is not configured for {name}. Install equivalent GPUI development packages, then rerun with --skip-system-deps.")
    if not skip:
        packages = PACKAGES[manager]
        if manager == "apt":
            privileged(["apt-get", "update"])
            privileged(["apt-get", "install", "-y", *packages])
        elif manager == "pacman":
            privileged(["pacman", "-S", "--needed", "--noconfirm", *packages])
        elif manager == "xbps":
            privileged(["xbps-install", "-S"])
            privileged(["xbps-install", "-y", *packages])
        elif manager == "dnf":
            privileged(["dnf" if shutil.which("dnf") else "yum", "install", "-y", *packages])
        elif manager == "zypper":
            privileged(["zypper", "--non-interactive", "install", "--no-recommends", *packages])
        elif manager == "apk":
            privileged(["apk", "add", *packages])
        else:
            privileged(["emerge", "--noreplace", *packages])
    for tool in ("pkg-config", "cmake", "clang"):
        if not shutil.which(tool):
            raise RuntimeError(f"Required build tool '{tool}' is missing. Install it and rerun.")

def install_macos_deps(skip: bool) -> None:
    if not shutil.which("xcode-select") or subprocess.run(["xcode-select", "-p"], capture_output=True).returncode:
        raise RuntimeError("Install Xcode Command Line Tools first with: xcode-select --install")
    if not shutil.which("brew"):
        raise RuntimeError("Homebrew is required to install CMake and pkg-config. Install it from https://brew.sh, then rerun.")
    if not skip:
        run(["brew", "install", "cmake", "pkg-config", "fontconfig"])
    for tool in ("cmake", "pkg-config", "clang", "fc-list"):
        if not shutil.which(tool):
            raise RuntimeError(f"Required build tool '{tool}' is missing.")

def is_windows_11() -> bool:
    return os.name == "nt" and sys.getwindowsversion().build >= 22000

def find_vs() -> Path | None:
    root = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"))
    vswhere = root / "Microsoft Visual Studio" / "Installer" / "vswhere.exe"
    if not vswhere.exists():
        return None
    component = "Microsoft.VisualStudio.Component.VC.Tools.ARM64" if platform.machine().lower() in {"arm64", "aarch64"} else "Microsoft.VisualStudio.Component.VC.Tools.x86.x64"
    result = subprocess.run([str(vswhere), "-latest", "-products", "*", "-requires", component, "-property", "installationPath"], capture_output=True, text=True)
    return Path(result.stdout.strip()) if result.returncode == 0 and result.stdout.strip() else None

def install_windows_deps(skip: bool) -> Path:
    if not is_windows_11():
        raise RuntimeError("The source installer supports Windows 11 only. Windows 10 is not supported.")
    vs = find_vs()
    if vs is None:
        if skip:
            raise RuntimeError("Visual Studio C++ Build Tools are missing.")
        if not shutil.which("winget"):
            raise RuntimeError("Install Visual Studio 2022 Build Tools with the Desktop development with C++ workload.")
        extra = " --add Microsoft.VisualStudio.Component.VC.Tools.ARM64" if platform.machine().lower() in {"arm64", "aarch64"} else ""
        override = "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --add Microsoft.VisualStudio.Component.Windows11SDK.26100" + extra
        run(["winget", "install", "--id", "Microsoft.VisualStudio.2022.BuildTools", "--exact", "--silent", "--accept-source-agreements", "--accept-package-agreements", "--override", override])
        vs = find_vs()
        if vs is None:
            raise RuntimeError("Visual Studio Build Tools installation did not expose the required MSVC toolchain.")
    if not shutil.which("cmake"):
        if skip:
            raise RuntimeError("CMake is missing.")
        run(["winget", "install", "--id", "Kitware.CMake", "--exact", "--silent", "--accept-source-agreements", "--accept-package-agreements"])
        cmake_bin = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "CMake" / "bin"
        os.environ["PATH"] = str(cmake_bin) + os.pathsep + os.environ.get("PATH", "")
    if not shutil.which("cmake"):
        raise RuntimeError("CMake is missing. Install it, open a new terminal, and rerun.")
    return vs

def rustup_host() -> str:
    arch = "aarch64" if platform.machine().lower() in {"aarch64", "arm64"} else "x86_64"
    if os.name == "nt": return f"{arch}-pc-windows-msvc"
    if sys.platform == "darwin": return f"{arch}-apple-darwin"
    if sys.platform.startswith("linux"): return f"{arch}-unknown-linux-gnu"
    raise RuntimeError(f"Unsupported platform: {sys.platform}")

def ensure_rustup() -> None:
    os.environ["PATH"] = str(CARGO_BIN) + os.pathsep + os.environ.get("PATH", "")
    if shutil.which("rustup"):
        return
    suffix = ".exe" if os.name == "nt" else ""
    url = f"https://static.rust-lang.org/rustup/dist/{rustup_host()}/rustup-init{suffix}"
    destination = Path(tempfile.gettempdir()) / f"relay-rustup-init{suffix}"
    print("Downloading the official Rustup installer...", flush=True)
    with urllib.request.urlopen(url, timeout=60) as response, destination.open("wb") as out:
        shutil.copyfileobj(response, out)
    if os.name != "nt":
        destination.chmod(0o755)
    run([str(destination), "-y", "--default-toolchain", "none", "--no-modify-path"])
    os.environ["PATH"] = str(CARGO_BIN) + os.pathsep + os.environ.get("PATH", "")
    if not shutil.which("rustup"):
        raise RuntimeError("Rustup was installed but is not available in PATH. Open a new terminal and rerun.")

def build(vs: Path | None, debug: bool) -> None:
    cargo = shutil.which("cargo") or str(CARGO_BIN / ("cargo.exe" if os.name == "nt" else "cargo"))
    args = ["build", "--locked", "-p", "relay-ui", "-p", "relay-cli"] if debug else ["relay-build"]
    if os.name == "nt":
        devcmd = vs / "Common7" / "Tools" / "VsDevCmd.bat"
        arch = "arm64" if platform.machine().lower() in {"arm64", "aarch64"} else "x64"
        command = f'call "{devcmd}" -arch={arch} -host_arch={arch} && "{cargo}" ' + " ".join(args)
        run(["cmd.exe", "/d", "/s", "/c", command])
    else:
        run([cargo, *args])

def build_output(name: str, debug: bool) -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return ROOT / "target" / ("debug" if debug else "release") / f"{name}{suffix}"

def copy_binaries(destination: Path, debug: bool) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    for name in ("relay", "relay-cli", "relay-clear"):
        source = build_output(name, debug)
        if not source.is_file():
            raise RuntimeError(f"Expected build output was not created: {source}")
        target = destination / source.name
        shutil.copy2(source, target)
        if os.name != "nt":
            target.chmod(0o755)

def install_linux(debug: bool) -> None:
    home = Path.home()
    bins = home / ".local" / "bin"
    copy_binaries(bins, debug)
    applications = home / ".local" / "share" / "applications"
    applications.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / "packaging" / "io.github.csemanpeter.Relay.desktop", applications)
    shutil.copytree(ROOT / "packaging" / "icons" / "hicolor", home / ".local" / "share" / "icons" / "hicolor", dirs_exist_ok=True)
    print(f"Installed Relay to {bins}. Make sure ~/.local/bin is on PATH.")

def install_macos(debug: bool) -> None:
    home = Path.home()
    app = home / "Applications" / "Relay.app"
    contents = app / "Contents"
    binaries = contents / "MacOS"
    copy_binaries(binaries, debug)
    (contents / "PkgInfo").write_bytes(b"APPL????")
    version = "0.0.0"
    for line in (ROOT / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("version = "):
            version = line.split('"')[1]
            break
    plist = f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>relay</string>
<key>CFBundleIdentifier</key><string>io.github.csemanpeter.Relay</string>
<key>CFBundleName</key><string>Relay</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
"""
    contents.mkdir(parents=True, exist_ok=True)
    (contents / "Info.plist").write_text(plist, encoding="utf-8")
    copy_binaries(home / ".local" / "bin", debug)
    print(f"Installed Relay to {app}; CLI tools are in ~/.local/bin.")

def install_windows(debug: bool) -> None:
    local = Path(os.environ.get("LOCALAPPDATA", str(Path.home() / "AppData" / "Local")))
    app = local / "Programs" / "Relay"
    copy_binaries(app, debug)
    menu = Path(os.environ.get("APPDATA", str(Path.home() / "AppData" / "Roaming"))) / "Microsoft" / "Windows" / "Start Menu" / "Programs" / "Relay"
    menu.mkdir(parents=True, exist_ok=True)
    ps = f"$w=New-Object -ComObject WScript.Shell; $s=$w.CreateShortcut('{menu / 'Relay.lnk'}'); $s.TargetPath='{app / 'relay.exe'}'; $s.WorkingDirectory='{app}'; $s.Save()"
    subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", ps], check=True)
    import winreg
    key = winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment", 0, winreg.KEY_READ | winreg.KEY_WRITE)
    try:
        try:
            old, kind = winreg.QueryValueEx(key, "Path")
        except FileNotFoundError:
            old, kind = "", winreg.REG_EXPAND_SZ
        parts = [p for p in old.split(";") if p]
        if str(app).rstrip("\\").lower() not in [p.rstrip("\\").lower() for p in parts]:
            winreg.SetValueEx(key, "Path", 0, kind, ";".join(parts + [str(app)]))
    finally:
        winreg.CloseKey(key)
    print(f"Installed Relay to {app}. Open Relay from the Start menu or use relay-cli in a new terminal.")

def main() -> int:
    parser = argparse.ArgumentParser(description="Install build dependencies, build Relay desktop and CLI, and install them for the current user.")
    parser.add_argument("--skip-system-deps", action="store_true", help="Do not install OS packages; use when dependencies are already installed.")
    parser.add_argument("--no-install", action="store_true", help="Build the applications without installing them.")
    parser.add_argument("--debug", action="store_true", help="Use the faster Cargo debug profile instead of release.")
    args = parser.parse_args()
    if sys.platform.startswith("linux"):
        install_linux_deps(args.skip_system_deps)
        vs = None
    elif sys.platform == "darwin":
        install_macos_deps(args.skip_system_deps)
        vs = None
    elif os.name == "nt":
        vs = install_windows_deps(args.skip_system_deps)
    else:
        raise RuntimeError("Relay source installer supports Linux, macOS, and Windows 11.")
    ensure_rustup()
    print("Building Relay desktop app and CLI...", flush=True)
    build(vs, args.debug)
    if not args.no_install:
        if sys.platform.startswith("linux"): install_linux(args.debug)
        elif sys.platform == "darwin": install_macos(args.debug)
        else: install_windows(args.debug)
    print("Relay build completed successfully.")
    return 0

if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except subprocess.CalledProcessError as error:
        print(f"Command failed with exit code {error.returncode}.", file=sys.stderr)
        raise SystemExit(error.returncode or 1)
    except (OSError, RuntimeError, urllib.error.URLError) as error:
        print(f"Error: {error}", file=sys.stderr)
        raise SystemExit(1)
