# novafetch

A fast, lightweight fetch written in Rust that is aware of your distro.

`novafetch` was built as an alternative to fastfetch. It automatically themes its output to match your distribution's brand colors.

## Features

- 🎨 **Distro colors:** Automatically reads `ANSI_COLOR` from `/etc/os-release` so the OS line perfectly matches your distro's official brand color. Hardcoded fallbacks are included for missing values.
- 🖥️ **Wayland native:** Connects directly to the Wayland socket to reliably detect standalone compositors (niri, Sway, Hyprland, etc.) rather than relying on brittle environment variables.
- ⚙️ **Auto-configuration:** Generates a default `config.toml` in your `~/.config/novafetch/` directory, making it easy to toggle rows, adjust colors, and enable external command execution.
- 📦 **Package counts:** Built-in detection for `rpm`, `dpkg`, `pacman`, `apk`, `xbps`, `flatpak`, and `snap`.
- 🕹️ **Terminal detection:** Unmasks wrappers like `sudo`, `strace`, and `flatpak`, maps complex process names (e.g., `wezterm-gui` ⭢ `WezTerm`), and checks environment variables to accurately report the actual terminal emulator in use.

## Installation

Ensure you have Rust and Cargo installed, as well as `gcc`.

```bash
git clone https://github.com/JerrySM64/novafetch.git
cd novafetch
cargo build --release
sudo cp target/release/novafetch /usr/local/bin/
```

## Configuration

On the first run, `novafetch` generates a default configuration file at `~/.config/novafetch/config.toml`.

The file uses standard TOML syntax. You can disable specific modules, adjust the frame colors, and enable `allow_exec = true` to permit the execution of external commands for accurate package counts and detailed shell version strings.

Example `config.toml`:

```toml
# novafetch configuration

# Allow external commands for shell version, git info, packages, etc.
allow_exec = true

# The order of modules to display.
order = [
    "os", "kernel", "uptime", "os_age", "shell", "spacer",
    "host", "hostname", "cpu", "gpu", "memory", "spacer",
    "session", "de_wm", "terminal", "packages", "disks"
]

# Overall theme colors (RGB arrays)
[theme]
frame_rgb = [100, 100, 100]
header_rgb = [200, 200, 200]

# Disable a specific module
[modules.packages]
enabled = false
```

## License

Available under the MIT License.
