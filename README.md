<div align="center">
  <h1>Copper</h1>
  <img src="https://github.com/user-attachments/assets/12ec45e9-4224-44ff-9e00-762a23dcf7b2" width="700" />
</div>

Copper is a lightweight, minimalist PipeWire volume control application built with Rust and egui. It provides a clean and efficient interface for managing audio devices and streams on Linux, and is feature-complete compared to pavucontrol.

## Features

- Manage audio outputs and inputs.
- Control playback and recording streams.
- Set default devices.
- Mute and volume control.
- Minimalist design with no heavy dependencies.

## Target Audience

Copper is specifically designed for users of tiling window managers (like i3, sway, Hyprland, or dwm) who prefer lightweight tools that integrate well into a minimalist workflow without requiring a full desktop environment.

## Building

### Prerequisites

You will need the PipeWire development headers installed on your system.

- **Arch Linux**: `sudo pacman -S pipewire pkgconf`
- **Fedora**: `sudo dnf install pipewire-devel pkgconf-pkg-config`
- **Ubuntu/Debian**: `sudo apt install libpipewire-0.3-dev pkg-config`

### Build from source

1. Clone the repository:
   ```bash
   git clone https://github.com/yourusername/copper.git
   cd copper
   ```

2. Build the release version:
   ```bash
   cargo build --release
   ```

3. The binary will be available at `target/release/copper`.
